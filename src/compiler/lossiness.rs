//! Shared lowering lossiness types and their rendering.
//!
//! Lowerers return per-field [`LossyField`] entries; callers hand them to
//! `DiagnosticCollector::record_lossy_fields`, which buffers one [`LossRecord`] each, and
//! `drain` calls [`render`] once. Nothing else turns a loss into a line.
//!
//! **A lost restriction that changes what an agent can do is shown by default. Every other lost
//! field is available under `--verbose`. Nothing is dropped without a record.** `Dropped` is the
//! one class that prints by default, as one line for the project ([`Lost`] names the kind). A site
//! goes quiet only as `Quiet { why }` (it loses nothing, or something else already covers it),
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
    /// A field the native artifact does not carry whose loss changes what the agent can do (a
    /// restriction) or that the user wrote and can fix (a setting). Printed by default.
    Dropped(Lost),
    /// A field the native artifact does not carry that loses nothing, or whose loss something
    /// else already covers; `why` says what. Shown only under `--verbose`.
    Quiet {
        why: &'static str,
    },
    /// A launch-time field meridian applies at spawn. Shown only under `--verbose`.
    MeridianOnly,
}

/// What a [`Lossiness::Dropped`] field costs, so the default line can say it plainly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lost {
    /// A shell restriction: a scoped `bash(...)` deny, or an allow list that also bounds the shell.
    Shell,
    /// A tool deny or `ask` other than the shell's.
    Tool,
    Mcp,
    Network,
    /// A value the user wrote that no target file carries (an override Mars discards,
    /// `model-invocable: false`).
    Setting,
}

impl Lost {
    fn label(self) -> &'static str {
        match self {
            Lost::Shell => "a shell restriction",
            Lost::Tool => "a tool restriction",
            Lost::Mcp => "an MCP restriction",
            Lost::Network => "a network restriction",
            Lost::Setting => "a setting",
        }
    }
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

/// How many item names the default line lists before it counts the rest.
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
/// `Surface` prints one line for the project covering every `Dropped` field. `Verbose` prints one
/// line per item and target for every lossy field: dropped, quiet, approximate and launch-time.
pub(crate) fn render(records: &[LossRecord], mode: LossinessMode) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if mode.verbose() {
        render_per_item(records, &mut out);
        render_meridian_only(records, &mut out);
    } else {
        render_dropped_rollup(records, &mut out);
    }
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
            Lossiness::Dropped(_) | Lossiness::Quiet { .. } => {
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

/// One line for the whole project: which items lose what on which targets. The per-item,
/// per-target, per-field detail is `--verbose`.
fn render_dropped_rollup(records: &[LossRecord], out: &mut Vec<Diagnostic>) {
    let lost: BTreeSet<(&str, &str, &str)> = records
        .iter()
        .filter(|r| matches!(r.field.classification, Lossiness::Dropped(_)))
        .map(|r| (r.item_kind, r.item_name.as_str(), r.field.target.as_str()))
        .collect();
    if lost.is_empty() {
        return;
    }
    let kinds_lost: BTreeSet<Lost> = records
        .iter()
        .filter_map(|r| match r.field.classification {
            Lossiness::Dropped(kind) => Some(kind),
            _ => None,
        })
        .collect();
    let items: BTreeSet<(&str, &str)> = lost.iter().map(|(k, n, _)| (*k, *n)).collect();
    let targets: BTreeSet<&str> = lost.iter().map(|(_, _, t)| *t).collect();
    let item_kinds: BTreeSet<&str> = items.iter().map(|(k, _)| *k).collect();
    let noun = match item_kinds.iter().next() {
        Some(kind) if item_kinds.len() == 1 => *kind,
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
    let what = join_and(
        &kinds_lost
            .iter()
            .map(|k| k.label().to_string())
            .collect::<Vec<_>>(),
    );
    let labels: Vec<String> = targets.iter().map(|t| target_label(t)).collect();
    out.push(diagnostic(
        DiagnosticLevel::Warning,
        "agent-restriction-dropped",
        format!(
            "{count} {noun}{plural} ({listed}{more}) {verb} {what} {on} {}: it does not apply there; run again with --verbose for the rules per {noun} and target",
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
    fn surface_prints_one_project_line_for_dropped_fields_and_verbose_prints_all() {
        let record = |d: &mut DiagnosticCollector| {
            for target in ["Codex", "Pi"] {
                let lossy = vec![
                    field(
                        "tools (deny bash(git reset --hard *))",
                        target,
                        Lossiness::Dropped(Lost::Shell),
                    ),
                    field("user-invocable", target, Lossiness::Quiet { why: "w" }),
                    field("effort", target, Lossiness::Approximate { note: "n" }),
                    field("approval", target, Lossiness::MeridianOnly),
                ];
                for name in ["a", "b", "c", "d", "e"] {
                    d.record_lossy_fields("agent", name, &lossy);
                }
            }
        };
        let surface = run(LossinessMode::Surface, record);
        assert_eq!(surface.len(), 1, "quiet fields print nothing by default");
        assert_eq!(surface[0].code, "agent-restriction-dropped");
        assert_eq!(
            surface[0].message,
            "5 agents (`a`, `b`, `c` and 2 more) lose a shell restriction on .codex and .pi: it does \
             not apply there; run again with --verbose for the rules per agent and target"
        );
        let verbose = run(LossinessMode::Verbose, record);
        assert_eq!(
            verbose.len(),
            30,
            "dropped+quiet, approximate and launch-time line per agent and target"
        );
        assert!(verbose[0].message.contains("user-invocable — w"));
        assert!(run(LossinessMode::Hidden, record).is_empty());
    }

    #[test]
    fn line_names_each_kind_lost_and_says_some_of_when_targets_differ() {
        let out = run(LossinessMode::Surface, |d| {
            d.record_lossy_fields(
                "agent",
                "a",
                &[
                    field(
                        "tools (deny bash(x))",
                        "Codex",
                        Lossiness::Dropped(Lost::Shell),
                    ),
                    field("network", "Codex", Lossiness::Dropped(Lost::Network)),
                ],
            );
            d.record_lossy_fields(
                "agent",
                "b",
                &[field("x", "Pi", Lossiness::Dropped(Lost::Setting))],
            );
        });
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains(
            "lose a shell restriction, a network restriction and a setting on some of .codex and .pi"
        ));
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
    fn a_dropped_skill_field_reaches_the_line() {
        let out = run(LossinessMode::Surface, |d| {
            d.record_lossy_fields(
                "skill",
                "s",
                &[field("x", "Codex", Lossiness::Dropped(Lost::Tool))],
            )
        });
        assert!(
            out[0]
                .message
                .starts_with("1 skill (`s`) loses a tool restriction on .codex")
        );
    }
}
