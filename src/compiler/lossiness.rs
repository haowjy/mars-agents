//! Shared lowering lossiness types and their rendering.
//!
//! Lowerers return per-field [`LossyField`] entries; callers hand them to
//! `DiagnosticCollector::record_lossy_fields`, which buffers one [`LossRecord`] each, and
//! `drain` calls [`render`] once. Nothing else turns a loss into a line.
//!
//! **Consequence tiers:** `Dropped` is the default and is reported: a field the native file
//! does not carry may be a restriction (a tool deny, ask or allow list, an MCP deny, a network
//! limit, an overridden policy key), and then the target runs the agent with what the author
//! withheld. A plain sync prints one line for the whole project. A site goes quiet only as
//! `Harmless { why }` (or `Approximate`, `MeridianOnly`): those appear per item under
//! `--verbose` and in the `check` / `init` previews ([`LossinessMode::per_item`]);
//! meridian-only fields otherwise roll into one summary line.

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
    /// A field the native artifact does not carry, reported by default: a site is `Dropped`
    /// unless it can say why nothing the author withheld reaches the target
    /// ([`Lossiness::Harmless`]). A restriction (a tool deny, ask or allow list, an MCP deny, a
    /// network limit, an overridden policy key) the target never receives is always `Dropped`.
    Dropped,
    /// A field the native artifact does not carry that loses no restriction; `why` says so.
    /// Shown only per item (`--verbose`, previews).
    Harmless {
        why: &'static str,
    },
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

/// Warn once per target that project `[settings]` keys reach no native file of it: the
/// settings belong to the project, so the per-Agent summaries do not carry them.
pub fn emit_project_settings_dropped_warning(
    keys: &[&str],
    target: &str,
    diag: &mut DiagnosticCollector,
) {
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

/// How many agent names the default roll-up line lists before it counts the rest.
const ROLLUP_NAME_CAP: usize = 3;

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
/// - `Surface`: one line per project for every restriction lost on any target, and one
///   summary line for launch-time fields.
/// - `Verbose` / `Preview`: one line per item and target for dropped and approximate fields.
///   Launch-time fields are per item in `Verbose` and one summary line in `Preview`.
pub(crate) fn render(records: &[LossRecord], mode: LossinessMode) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if mode.per_item() {
        render_per_item(records, &mut out);
    } else {
        render_restriction_rollup(records, &mut out);
    }
    render_meridian_only(records, mode, &mut out);
    out
}

#[derive(Default)]
struct ItemTargetFields<'a> {
    dropped: Vec<&'a LossyField>,
    approximate: Vec<(String, &'static str)>,
}

fn render_per_item(records: &[LossRecord], out: &mut Vec<Diagnostic>) {
    let mut grouped: BTreeMap<(&str, &str, &str), ItemTargetFields> = BTreeMap::new();
    for record in records {
        let key = (
            record.item_kind,
            record.item_name.as_str(),
            record.field.target.as_str(),
        );
        match record.field.classification {
            Lossiness::Dropped | Lossiness::Harmless { .. } => {
                grouped.entry(key).or_default().dropped.push(&record.field);
            }
            Lossiness::Approximate { note } => grouped
                .entry(key)
                .or_default()
                .approximate
                .push((record.field.field.clone(), note)),
            Lossiness::MeridianOnly => {}
        }
    }
    for ((kind, name, target), mut fields) in grouped {
        let label = target_label(target);
        if !fields.dropped.is_empty() {
            let restriction = fields
                .dropped
                .iter()
                .any(|f| f.classification == Lossiness::Dropped);
            let mut names: Vec<String> = fields
                .dropped
                .iter()
                .map(|f| match f.classification {
                    Lossiness::Harmless { why } => format!("{} — {why}", f.field),
                    _ => f.field.clone(),
                })
                .collect();
            names.sort_unstable();
            names.dedup();
            let (count, noun) = counted(names.len());
            let tool_loss = if restriction {
                format!("; a restriction does not apply on {label}")
            } else {
                String::new()
            };
            out.push(diagnostic(
                DiagnosticLevel::Warning,
                if kind == "skill" {
                    "skill-field-dropped"
                } else {
                    "agent-field-dropped"
                },
                format!(
                    "{kind} `{name}`: {count} {noun} dropped for {label} ({}){tool_loss}",
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

/// One line for the whole project: which items lose a restriction on which targets. The
/// per-item, per-target, per-field detail is `--verbose`.
fn render_restriction_rollup(records: &[LossRecord], out: &mut Vec<Diagnostic>) {
    let lost: BTreeSet<(&str, &str, &str)> = records
        .iter()
        .filter(|r| r.field.classification == Lossiness::Dropped)
        .map(|r| (r.item_kind, r.item_name.as_str(), r.field.target.as_str()))
        .collect();
    if lost.is_empty() {
        return;
    }
    let items: BTreeSet<(&str, &str)> = lost.iter().map(|(k, n, _)| (*k, *n)).collect();
    let targets: BTreeSet<&str> = lost.iter().map(|(_, _, t)| *t).collect();
    let kinds: BTreeSet<&str> = items.iter().map(|(k, _)| *k).collect();
    let noun = match kinds.iter().next() {
        Some(kind) if kinds.len() == 1 => *kind,
        _ => "item",
    };
    let count = items.len();
    let listed = items
        .iter()
        .take(ROLLUP_NAME_CAP)
        .map(|(_, name)| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let more = match count.saturating_sub(ROLLUP_NAME_CAP) {
        0 => String::new(),
        rest => format!(" and {rest} more"),
    };
    let (plural, verb) = if count == 1 {
        ("", "loses")
    } else {
        ("s", "lose")
    };
    // "some of" unless every listed item loses on every listed target.
    let on = if lost.len() == items.len() * targets.len() || targets.len() == 1 {
        "on"
    } else {
        "on some of"
    };
    let labels: Vec<String> = targets.iter().map(|t| target_label(t)).collect();
    let verbose_cmd = crate::types::managed_cmd("mars sync --verbose");
    out.push(diagnostic(
        DiagnosticLevel::Warning,
        "agent-restriction-dropped",
        format!(
            "{count} {noun}{plural} ({listed}{more}) {verb} a restriction {on} {}: it does not apply there; run `{verbose_cmd}` for the rules per {noun} and target",
            join_and(&labels)
        ),
    ));
}

fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

fn render_meridian_only(records: &[LossRecord], mode: LossinessMode, out: &mut Vec<Diagnostic>) {
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
    if mode.meridian_detail() {
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
    } else {
        let count: usize = grouped.values().map(BTreeSet::len).sum();
        let noun = if count == 1 { "mapping" } else { "mappings" };
        out.push(diagnostic(
            DiagnosticLevel::Info,
            "launch-time-field-summary",
            format!(
                "{count} launch-time field {noun} handled by meridian at spawn — run with --verbose for detail"
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticLevel;

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
    fn multi_field_drop_produces_one_summarized_warning_per_target() {
        let lossy = vec![
            field("disallowed-tools", "OpenCode", Lossiness::Dropped),
            field(
                "user-invocable",
                "OpenCode",
                Lossiness::Harmless { why: "w" },
            ),
        ];
        let out = run(LossinessMode::Preview, |d| {
            d.record_lossy_fields("skill", "planning", &lossy)
        });
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, "skill-field-dropped");
        assert_eq!(out[0].category, Some(DiagnosticCategory::Lossiness));
        assert_eq!(out[0].level, DiagnosticLevel::Warning);
        assert!(out[0].message.contains("skill `planning`"));
        assert!(out[0].message.contains("2 fields dropped for .opencode"));
        assert!(out[0].message.contains("disallowed-tools"));
        assert!(out[0].message.contains("user-invocable — w"));
    }

    #[test]
    fn repeated_record_of_one_item_is_one_line_per_target() {
        let lossy = vec![
            field("model-invocable", "Claude", Lossiness::Dropped),
            field("user-invocable", "Claude", Lossiness::Dropped),
        ];
        let out = run(LossinessMode::Preview, |d| {
            for _ in 0..2 {
                d.record_lossy_fields("agent", "coder", &lossy);
            }
        });
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("2 fields dropped for .claude"));
    }

    #[test]
    fn approximate_warnings_are_summarized_per_target() {
        let lossy = vec![
            field(
                "tools",
                "Claude",
                Lossiness::Approximate {
                    note: "unknown tool name passed through verbatim",
                },
            ),
            field(
                "mode",
                "Claude",
                Lossiness::Approximate {
                    note: "native mode has different semantics",
                },
            ),
        ];
        let out = run(LossinessMode::Preview, |d| {
            d.record_lossy_fields("agent", "coder", &lossy)
        });
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, "agent-field-approximate");
        assert!(
            out[0]
                .message
                .contains("2 fields approximately mapped for .claude")
        );
        assert!(out[0].message.contains("mode:"));
        assert!(out[0].message.contains("tools:"));
    }

    #[test]
    fn meridian_only_surface_emits_summary_not_per_item() {
        let lossy = vec![field("approval", "Claude", Lossiness::MeridianOnly)];
        let out = run(LossinessMode::Surface, |d| {
            d.record_lossy_fields("agent", "coder", &lossy)
        });
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, "launch-time-field-summary");
        assert!(
            out[0]
                .message
                .contains("1 launch-time field mapping handled by meridian at spawn")
        );
    }

    #[test]
    fn meridian_only_verbose_emits_per_item_detail() {
        let lossy = vec![
            field("approval", "Claude", Lossiness::MeridianOnly),
            field("sandbox", "Claude", Lossiness::MeridianOnly),
        ];
        let out = run(LossinessMode::Verbose, |d| {
            d.record_lossy_fields("agent", "coder", &lossy)
        });
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, "agent-field-meridian-only");
        assert!(out[0].message.contains("approval, sandbox"));
    }

    #[test]
    fn surface_reports_lost_restrictions_in_one_line_for_the_project() {
        let record = |d: &mut DiagnosticCollector| {
            for target in ["Codex", "Pi"] {
                let lossy = vec![
                    field("tools (deny edit)", target, Lossiness::Dropped),
                    field("user-invocable", target, Lossiness::Harmless { why: "w" }),
                    field("effort", target, Lossiness::Approximate { note: "n" }),
                ];
                for name in ["a", "b", "c", "d", "e"] {
                    d.record_lossy_fields("agent", name, &lossy);
                }
                d.record_lossy_fields("skill", "s", &lossy[1..]);
            }
        };
        let surface = run(LossinessMode::Surface, record);
        assert_eq!(
            surface.len(),
            1,
            "plain drops and approximations stay quiet"
        );
        assert_eq!(surface[0].code, "agent-restriction-dropped");
        assert_eq!(
            surface[0].message,
            "5 agents (`a`, `b`, `c` and 2 more) lose a restriction on .codex and .pi: it does \
             not apply there; run `mars sync --verbose` for the rules per agent and target"
                .replace(
                    "mars sync --verbose",
                    &crate::types::managed_cmd("mars sync --verbose")
                )
        );
        let verbose = run(LossinessMode::Verbose, record);
        assert_eq!(
            verbose.len(),
            24,
            "dropped and approximate line per item and target"
        );
        assert!(verbose[0].message.contains("tools (deny edit)"));
        assert!(run(LossinessMode::Hidden, record).is_empty());
    }

    #[test]
    fn rollup_says_some_of_when_targets_differ_per_agent() {
        let out = run(LossinessMode::Surface, |d| {
            d.record_lossy_fields(
                "agent",
                "a",
                &[field("tools (deny edit)", "Codex", Lossiness::Dropped)],
            );
            d.record_lossy_fields(
                "agent",
                "b",
                &[field("tools (deny edit)", "Pi", Lossiness::Dropped)],
            );
        });
        assert_eq!(out.len(), 1);
        assert!(
            out[0]
                .message
                .contains("lose a restriction on some of .codex and .pi")
        );
    }

    #[test]
    fn a_dropped_skill_field_still_reaches_the_rollup() {
        let out = run(LossinessMode::Surface, |d| {
            d.record_lossy_fields(
                "skill",
                "s",
                &[field("tools (deny edit)", "Codex", Lossiness::Dropped)],
            )
        });
        assert_eq!(out.len(), 1);
        assert!(
            out[0]
                .message
                .starts_with("1 skill (`s`) loses a restriction on .codex")
        );
    }
}
