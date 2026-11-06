//! Shared lowering lossiness types and summarized diagnostics.
//!
//! Lowerers record per-field [`LossyField`] entries; callers aggregate by
//! `(item, target)` before emitting so re-sync does not spam one line per field.
//!
//! **Consequence tiers:** `Dropped` is the default and is reported: a field the native file
//! does not carry may be a restriction (a tool deny, ask or allow list, an MCP deny, a network
//! limit, an overridden policy key), and then the target runs the agent with what the author
//! withheld. Default sync output names the agents once per target (`DiagnosticCollector`).
//! A site goes quiet only as `Harmless { why }` (or `Approximate`, `MeridianOnly`): those
//! appear per item under `--verbose` and in the `check` / `init` previews
//! ([`LossinessMode::per_item`]); meridian-only fields otherwise roll into one summary line.

use std::collections::BTreeMap;

use crate::diagnostic::{DiagnosticCategory, DiagnosticCollector};

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

fn summarize_fields(fields: &[&str]) -> String {
    fields.join(", ")
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

/// Emit deduplicated lossiness warnings for one lowered agent artifact.
pub fn emit_agent_lossiness_warnings(
    agent_name: &str,
    lossy_fields: &[LossyField],
    diag: &mut DiagnosticCollector,
) {
    emit_item_lossiness_warnings(
        "agent",
        agent_name,
        "agent-field-dropped",
        "agent-field-approximate",
        lossy_fields,
        diag,
    );
}

/// Emit deduplicated lossiness warnings for one lowered skill artifact.
pub fn emit_skill_lossiness_warnings(
    skill_name: &str,
    lossy_fields: &[LossyField],
    diag: &mut DiagnosticCollector,
) {
    emit_item_lossiness_warnings(
        "skill",
        skill_name,
        "skill-field-dropped",
        "skill-field-approximate",
        lossy_fields,
        diag,
    );
}

fn emit_item_lossiness_warnings(
    item_kind: &str,
    item_name: &str,
    dropped_code: &'static str,
    approximate_code: &'static str,
    lossy_fields: &[LossyField],
    diag: &mut DiagnosticCollector,
) {
    let mut dropped_by_target: BTreeMap<String, Vec<&LossyField>> = BTreeMap::new();
    let mut approximate_by_target: BTreeMap<String, Vec<(String, &'static str)>> = BTreeMap::new();
    let mut meridian_by_target: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for lf in lossy_fields {
        match &lf.classification {
            Lossiness::Dropped | Lossiness::Harmless { .. } => {
                dropped_by_target
                    .entry(lf.target.clone())
                    .or_default()
                    .push(lf);
            }
            Lossiness::MeridianOnly => {
                meridian_by_target
                    .entry(lf.target.clone())
                    .or_default()
                    .push(lf.field.clone());
            }
            Lossiness::Approximate { note } => {
                approximate_by_target
                    .entry(lf.target.clone())
                    .or_default()
                    .push((lf.field.clone(), note));
            }
        }
    }

    if diag.lossiness_per_item() {
        emit_grouped_warnings(item_kind, item_name, dropped_code, dropped_by_target, diag);
        emit_grouped_approximate_warnings(
            item_kind,
            item_name,
            approximate_code,
            approximate_by_target,
            diag,
        );
    } else {
        for (target, fields) in &dropped_by_target {
            if fields
                .iter()
                .any(|f| f.classification == Lossiness::Dropped)
            {
                diag.record_restriction_lost(item_name, target);
            }
        }
    }
    for (target, fields) in meridian_by_target {
        for field in fields {
            diag.record_meridian_only_field(item_kind, item_name, &target, &field);
        }
    }
}

fn emit_grouped_approximate_warnings(
    item_kind: &str,
    item_name: &str,
    code: &'static str,
    grouped: BTreeMap<String, Vec<(String, &'static str)>>,
    diag: &mut DiagnosticCollector,
) {
    for (target, mut fields) in grouped {
        fields.sort_unstable();
        fields.dedup();
        let count = fields.len();
        let noun = if count == 1 { "field" } else { "fields" };
        let details = fields
            .iter()
            .map(|(field, note)| format!("{field}: {note}"))
            .collect::<Vec<_>>()
            .join("; ");
        diag.warn_with_category(
            code,
            format!(
                "{item_kind} `{item_name}`: {count} {noun} approximately mapped for {} ({details})",
                target_label(&target)
            ),
            DiagnosticCategory::Lossiness,
        );
    }
}

fn emit_grouped_warnings(
    item_kind: &str,
    item_name: &str,
    code: &'static str,
    grouped: BTreeMap<String, Vec<&LossyField>>,
    diag: &mut DiagnosticCollector,
) {
    for (target, fields) in grouped {
        let restriction = fields
            .iter()
            .any(|f| f.classification == Lossiness::Dropped);
        let mut names: Vec<String> = fields
            .iter()
            .map(|f| match f.classification {
                Lossiness::Harmless { why } => format!("{} — {why}", f.field),
                _ => f.field.clone(),
            })
            .collect();
        names.sort_unstable();
        names.dedup();
        let count = names.len();
        let noun = if count == 1 { "field" } else { "fields" };
        let tool_loss = if restriction {
            format!(
                "; a restriction does not apply on {}",
                target_label(&target)
            )
        } else {
            String::new()
        };
        diag.warn_with_category(
            code,
            format!(
                "{item_kind} `{item_name}`: {count} {noun} dropped for {} ({}){tool_loss}",
                target_label(&target),
                summarize_fields(&names.iter().map(String::as_str).collect::<Vec<_>>())
            ),
            DiagnosticCategory::Lossiness,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{DiagnosticLevel, LossinessMode};

    #[test]
    fn multi_field_drop_produces_one_summarized_warning_per_target() {
        let lossy = vec![
            LossyField {
                field: "disallowed-tools".into(),
                target: "OpenCode".into(),
                classification: Lossiness::Dropped,
            },
            LossyField {
                field: "user-invocable".into(),
                target: "OpenCode".into(),
                classification: Lossiness::Dropped,
            },
        ];
        let mut diag = DiagnosticCollector::with_lossiness_mode(LossinessMode::Preview);
        emit_skill_lossiness_warnings("planning", &lossy, &mut diag);
        let warnings: Vec<_> = diag
            .drain()
            .into_iter()
            .filter(|d| d.level == DiagnosticLevel::Warning)
            .collect();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "skill-field-dropped");
        assert_eq!(warnings[0].category, Some(DiagnosticCategory::Lossiness));
        assert!(warnings[0].message.contains("skill `planning`"));
        assert!(
            warnings[0]
                .message
                .contains("2 fields dropped for .opencode")
        );
        assert!(warnings[0].message.contains("disallowed-tools"));
        assert!(warnings[0].message.contains("user-invocable"));
    }

    #[test]
    fn repeated_emit_on_resync_still_one_warning_per_target_not_per_field() {
        let lossy = vec![
            LossyField {
                field: "model-invocable".into(),
                target: "Claude".into(),
                classification: Lossiness::Dropped,
            },
            LossyField {
                field: "user-invocable".into(),
                target: "Claude".into(),
                classification: Lossiness::Dropped,
            },
        ];
        let mut diag = DiagnosticCollector::with_lossiness_mode(LossinessMode::Preview);
        for _ in 0..2 {
            emit_agent_lossiness_warnings("coder", &lossy, &mut diag);
        }
        let dropped: Vec<_> = diag
            .drain()
            .into_iter()
            .filter(|d| d.code == "agent-field-dropped")
            .collect();
        assert_eq!(dropped.len(), 2, "each lowering pass emits one summary");
        assert!(
            dropped
                .iter()
                .all(|d| d.message.contains("2 fields dropped for .claude"))
        );
    }

    #[test]
    fn approximate_warnings_are_summarized_per_target() {
        let lossy = vec![
            LossyField {
                field: "tools".into(),
                target: "Claude".into(),
                classification: Lossiness::Approximate {
                    note: "unknown tool name passed through verbatim",
                },
            },
            LossyField {
                field: "mode".into(),
                target: "Claude".into(),
                classification: Lossiness::Approximate {
                    note: "native mode has different semantics",
                },
            },
        ];
        let mut diag = DiagnosticCollector::with_lossiness_mode(LossinessMode::Preview);
        emit_agent_lossiness_warnings("coder", &lossy, &mut diag);
        let warnings = diag.drain();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "agent-field-approximate");
        assert!(
            warnings[0]
                .message
                .contains("2 fields approximately mapped for .claude")
        );
        assert!(warnings[0].message.contains("mode:"));
        assert!(warnings[0].message.contains("tools:"));
    }

    #[test]
    fn meridian_only_surface_emits_summary_not_per_item() {
        let lossy = vec![LossyField {
            field: "approval".into(),
            target: "Claude".into(),
            classification: Lossiness::MeridianOnly,
        }];
        let mut diag = DiagnosticCollector::with_lossiness_mode(LossinessMode::Surface);
        emit_agent_lossiness_warnings("coder", &lossy, &mut diag);
        let warnings = diag.drain();
        assert!(
            !warnings
                .iter()
                .any(|d| d.code == "agent-field-meridian-only"),
            "surface must not emit per-item meridian-only warnings"
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "launch-time-field-summary");
        assert!(
            warnings[0]
                .message
                .contains("launch-time field mapping handled by meridian at spawn")
        );
    }

    #[test]
    fn meridian_only_verbose_emits_per_item_detail() {
        let lossy = vec![
            LossyField {
                field: "approval".into(),
                target: "Claude".into(),
                classification: Lossiness::MeridianOnly,
            },
            LossyField {
                field: "sandbox".into(),
                target: "Claude".into(),
                classification: Lossiness::MeridianOnly,
            },
        ];
        let mut diag = DiagnosticCollector::with_lossiness_mode(LossinessMode::Verbose);
        emit_agent_lossiness_warnings("coder", &lossy, &mut diag);
        let warnings = diag.drain();
        assert!(
            !warnings
                .iter()
                .any(|d| d.code == "launch-time-field-summary"),
            "verbose must not emit summary line"
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "agent-field-meridian-only");
        assert!(warnings[0].message.contains("approval"));
        assert!(warnings[0].message.contains("sandbox"));
    }

    #[test]
    fn surface_reports_only_lost_restrictions_one_line_per_target() {
        let field = |field: &str, target: &str, classification| LossyField {
            field: field.into(),
            target: target.into(),
            classification,
        };
        let lossy = vec![
            field("tools (deny edit)", "Codex", Lossiness::Dropped),
            field("user-invocable", "Codex", Lossiness::Harmless { why: "w" }),
            field("effort", "Codex", Lossiness::Approximate { note: "n" }),
        ];
        let run = |mode| {
            let mut diag = DiagnosticCollector::with_lossiness_mode(mode);
            for name in ["a", "b", "c", "d", "e"] {
                emit_agent_lossiness_warnings(name, &lossy, &mut diag);
            }
            emit_skill_lossiness_warnings("s", &lossy[1..], &mut diag);
            diag.drain()
        };
        let surface = run(LossinessMode::Surface);
        assert_eq!(
            surface.len(),
            1,
            "plain drops and approximations stay quiet"
        );
        assert_eq!(surface[0].code, "agent-restriction-dropped");
        assert!(
            surface[0]
                .message
                .contains("5 agents (`a`, `b`, `c` and 2 more)")
        );
        assert!(surface[0].message.contains("lose a restriction on .codex"));
        let verbose = run(LossinessMode::Verbose);
        assert_eq!(verbose.len(), 12, "dropped and approximate line per item");
        assert!(verbose[0].message.contains("tools (deny edit)"));
        assert!(run(LossinessMode::Hidden).is_empty());
    }
}
