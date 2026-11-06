//! Shared lowering lossiness types and their rendering.
//!
//! Lowerers return per-field [`LossyField`] entries; callers hand them to
//! `DiagnosticCollector::record_lossy_fields`, which buffers one [`LossRecord`] each, and
//! `drain` calls [`render`] once. Nothing else turns a loss into a line.
//!
//! **Warn only when the user can act on it or it changes behaviour that matters; everything
//! else is `--verbose`.** `Dropped` is the one class that prints by default, one line per item
//! and target: the user wrote something Mars discards and can fix it. A site that cannot meet
//! that is `Quiet { why }` (the native file has no carrier for it, or it loses nothing),
//! `Approximate`, or `MeridianOnly` (a launch-time field); those print per item under
//! `--verbose`. Quiet needs a reason, so an unclassified site is loud.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCollector, DiagnosticLevel, LossinessMode,
};

/// A field that was dropped or only approximately lowered in the native artifact.
#[derive(Debug, Clone)]
pub struct LossyField {
    pub field: String,
    pub target: String,
    pub classification: Lossiness,
}

/// Lossiness classification for a single field in a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lossiness {
    Approximate {
        note: &'static str,
    },
    /// A field the native artifact does not carry that the user wrote and can act on, or whose
    /// loss changes behaviour that matters. Printed by default.
    Dropped,
    /// A field the native artifact does not carry and nothing the user can fix: `why` says why
    /// it loses nothing, or that no native carrier exists and meridian reports or enforces it
    /// at launch. Shown only under `--verbose`.
    Quiet {
        why: &'static str,
    },
    /// A launch-time field meridian applies at spawn. Shown only under `--verbose`.
    MeridianOnly,
}

/// A secondary artifact emitted alongside the primary lowered file (e.g. Codex `openai.yaml`).
#[derive(Debug, Clone)]
pub struct LoweredSibling {
    /// Path relative to the skill directory root (e.g. `openai.yaml`).
    pub rel_path: String,
    pub bytes: Vec<u8>,
}

/// Output from a single lowering pass.
pub struct LoweredOutput {
    /// Serialized bytes for the native artifact.
    pub bytes: Vec<u8>,
    /// Lossiness findings for fields that were dropped or approximated.
    pub lossy_fields: Vec<LossyField>,
    /// Extra files written next to the primary artifact (skills only today).
    pub siblings: Vec<LoweredSibling>,
}

pub(crate) fn target_label(target: &str) -> String {
    format!(".{}", target.to_lowercase())
}

/// Under `--verbose`, note once per target that project `[settings]` keys reach no native file
/// of it: the settings belong to the project, so the per-Agent lines do not carry them. Quiet
/// by default: the target has no carrier, and `meridian spawn --dry-run` reports the gap.
pub fn emit_project_settings_dropped_warning(
    keys: &[&str],
    target: &str,
    diag: &mut DiagnosticCollector,
) {
    if !diag.lossiness_verbose() {
        return;
    }
    let (list, verb) = match keys {
        [] => return,
        [one] => ((*one).to_string(), "is"),
        _ => (keys.join(", "), "are"),
    };
    diag.warn_with_category(
        "project-setting-dropped",
        format!(
            "project {list} {verb} not projected to {}; native {target} agents keep {target}'s own network default",
            target_label(target)
        ),
        DiagnosticCategory::Lossiness,
    );
}

/// One lossy field of one lowered item, as the collector buffers it until `drain`.
#[derive(Debug, Clone)]
pub(crate) struct LossRecord {
    /// `"agent"` or `"skill"`.
    pub item_kind: &'static str,
    pub item_name: String,
    pub field: LossyField,
}

fn diagnostic(level: DiagnosticLevel, code: &'static str, message: String) -> Diagnostic {
    Diagnostic {
        level,
        code: code.into(),
        message,
        context: None,
        category: Some(DiagnosticCategory::Lossiness),
    }
}

/// Render the buffered records for `mode`. This is the only place lossiness becomes lines.
///
/// `Surface` prints `Dropped` fields per item and target. `Verbose` adds quiet, approximate and
/// launch-time fields.
pub(crate) fn render(records: &[LossRecord], mode: LossinessMode) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    render_per_item(records, mode.verbose(), &mut out);
    if mode.verbose() {
        render_meridian_only(records, &mut out);
    }
    out
}

#[derive(Default)]
struct ItemTargetFields<'a> {
    dropped: Vec<&'a LossyField>,
    approximate: Vec<(String, &'static str)>,
}

fn render_per_item(records: &[LossRecord], verbose: bool, out: &mut Vec<Diagnostic>) {
    let mut grouped: BTreeMap<(&str, &str, &str), ItemTargetFields> = BTreeMap::new();
    for record in records {
        let key = (
            record.item_kind,
            record.item_name.as_str(),
            record.field.target.as_str(),
        );
        match record.field.classification {
            Lossiness::Dropped => {
                grouped.entry(key).or_default().dropped.push(&record.field);
            }
            Lossiness::Quiet { .. } if verbose => {
                grouped.entry(key).or_default().dropped.push(&record.field);
            }
            Lossiness::Approximate { note } if verbose => grouped
                .entry(key)
                .or_default()
                .approximate
                .push((record.field.field.clone(), note)),
            Lossiness::Quiet { .. } | Lossiness::Approximate { .. } | Lossiness::MeridianOnly => {}
        }
    }
    for ((kind, name, target), mut fields) in grouped {
        let label = target_label(target);
        if !fields.dropped.is_empty() {
            let mut names: Vec<String> = fields
                .dropped
                .iter()
                .map(|f| match f.classification {
                    Lossiness::Quiet { why } => format!("{} — {why}", f.field),
                    _ => f.field.clone(),
                })
                .collect();
            names.sort_unstable();
            names.dedup();
            let (count, noun) = counted(names.len());
            out.push(diagnostic(
                DiagnosticLevel::Warning,
                if kind == "skill" {
                    "skill-field-dropped"
                } else {
                    "agent-field-dropped"
                },
                format!(
                    "{kind} `{name}`: {count} {noun} dropped for {label} ({})",
                    names.join(", ")
                ),
            ));
        }
        if !fields.approximate.is_empty() {
            fields.approximate.sort_unstable();
            fields.approximate.dedup();
            let (count, noun) = counted(fields.approximate.len());
            let details = fields
                .approximate
                .iter()
                .map(|(field, note)| format!("{field}: {note}"))
                .collect::<Vec<_>>()
                .join("; ");
            out.push(diagnostic(
                DiagnosticLevel::Warning,
                if kind == "skill" {
                    "skill-field-approximate"
                } else {
                    "agent-field-approximate"
                },
                format!(
                    "{kind} `{name}`: {count} {noun} approximately mapped for {label} ({details})"
                ),
            ));
        }
    }
}

fn counted(count: usize) -> (usize, &'static str) {
    (count, if count == 1 { "field" } else { "fields" })
}

fn render_meridian_only(records: &[LossRecord], out: &mut Vec<Diagnostic>) {
    let mut grouped: BTreeMap<(&str, &str, &str), BTreeSet<&str>> = BTreeMap::new();
    for record in records {
        if record.field.classification == Lossiness::MeridianOnly {
            grouped
                .entry((
                    record.item_kind,
                    record.item_name.as_str(),
                    record.field.target.as_str(),
                ))
                .or_default()
                .insert(record.field.field.as_str());
        }
    }
    if grouped.is_empty() {
        return;
    }
    for ((kind, name, target), fields) in grouped {
        let (count, noun) = counted(fields.len());
        out.push(diagnostic(
            DiagnosticLevel::Warning,
            if kind == "skill" {
                "skill-field-meridian-only"
            } else {
                "agent-field-meridian-only"
            },
            format!(
                "{kind} `{name}`: {count} {noun} not lowered (meridian-only) for {} ({})",
                target_label(target),
                fields.into_iter().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(field: &str, target: &str, classification: Lossiness) -> LossyField {
        LossyField {
            field: field.into(),
            target: target.into(),
            classification,
        }
    }

    fn run(mode: LossinessMode, record: impl FnOnce(&mut DiagnosticCollector)) -> Vec<Diagnostic> {
        let mut diag = DiagnosticCollector::with_lossiness_mode(mode);
        record(&mut diag);
        diag.drain()
    }

    #[test]
    fn surface_prints_only_dropped_fields_and_verbose_prints_all() {
        let record = |d: &mut DiagnosticCollector| {
            let lossy = vec![
                field(
                    "harness-overrides.codex.sandbox_mode",
                    "Codex",
                    Lossiness::Dropped,
                ),
                field("tools (deny edit)", "Codex", Lossiness::Quiet { why: "w" }),
                field("effort", "Codex", Lossiness::Approximate { note: "n" }),
                field("approval", "Codex", Lossiness::MeridianOnly),
            ];
            d.record_lossy_fields("agent", "coder", &lossy);
            // Recording twice must not repeat the line.
            d.record_lossy_fields("agent", "coder", &lossy[..1]);
        };
        let surface = run(LossinessMode::Surface, record);
        assert_eq!(surface.len(), 1);
        assert_eq!(surface[0].code, "agent-field-dropped");
        assert_eq!(
            surface[0].message,
            "agent `coder`: 1 field dropped for .codex (harness-overrides.codex.sandbox_mode)"
        );

        let verbose = run(LossinessMode::Verbose, record);
        let codes: Vec<_> = verbose.iter().map(|d| d.code.as_ref()).collect();
        assert_eq!(
            codes,
            [
                "agent-field-dropped",
                "agent-field-approximate",
                "agent-field-meridian-only"
            ]
        );
        assert!(verbose[0].message.contains("tools (deny edit) — w"));
        assert!(run(LossinessMode::Hidden, record).is_empty());
    }

    #[test]
    fn quiet_and_launch_time_fields_print_nothing_by_default() {
        let out = run(LossinessMode::Surface, |d| {
            d.record_lossy_fields(
                "agent",
                "coder",
                &[
                    field("tools (deny edit)", "Codex", Lossiness::Quiet { why: "w" }),
                    field("approval", "Codex", Lossiness::MeridianOnly),
                ],
            )
        });
        assert!(out.is_empty());
    }

    #[test]
    fn a_dropped_skill_field_prints_by_default() {
        let out = run(LossinessMode::Surface, |d| {
            d.record_lossy_fields("skill", "s", &[field("x", "Codex", Lossiness::Dropped)])
        });
        assert_eq!(out[0].code, "skill-field-dropped");
    }
}
