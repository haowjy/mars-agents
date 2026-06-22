//! Summarized lossiness diagnostics for agent/skill lowering.
//!
//! Lowerers record per-field [`LossyField`] entries; callers aggregate by
//! `(item, target)` before emitting so re-sync does not spam one line per field.

use std::collections::BTreeMap;

use crate::compiler::agents::lower::{Lossiness, LossyField};
use crate::diagnostic::{DiagnosticCategory, DiagnosticCollector};

fn target_label(target: &str) -> String {
    format!(".{}", target.to_lowercase())
}

fn summarize_fields(fields: &[&str]) -> String {
    fields.join(", ")
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
        "agent-field-meridian-only",
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
        "skill-field-meridian-only",
        "skill-field-approximate",
        lossy_fields,
        diag,
    );
}

fn emit_item_lossiness_warnings(
    item_kind: &str,
    item_name: &str,
    dropped_code: &'static str,
    meridian_code: &'static str,
    approximate_code: &'static str,
    lossy_fields: &[LossyField],
    diag: &mut DiagnosticCollector,
) {
    let mut dropped_by_target: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut meridian_by_target: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for lf in lossy_fields {
        match &lf.classification {
            Lossiness::Dropped => {
                dropped_by_target
                    .entry(lf.target.clone())
                    .or_default()
                    .push(lf.field.clone());
            }
            Lossiness::MeridianOnly => {
                meridian_by_target
                    .entry(lf.target.clone())
                    .or_default()
                    .push(lf.field.clone());
            }
            Lossiness::Approximate { note } => {
                diag.warn_with_category(
                    approximate_code,
                    format!(
                        "{item_kind} `{item_name}`: field `{}` approximately mapped in {} ({note})",
                        lf.field, lf.target
                    ),
                    DiagnosticCategory::Lossiness,
                );
            }
        }
    }

    emit_grouped_warnings(
        item_kind,
        item_name,
        dropped_code,
        "dropped",
        dropped_by_target,
        diag,
    );
    emit_grouped_warnings(
        item_kind,
        item_name,
        meridian_code,
        "not lowered (meridian-only)",
        meridian_by_target,
        diag,
    );
}

fn emit_grouped_warnings(
    item_kind: &str,
    item_name: &str,
    code: &'static str,
    classification_label: &str,
    grouped: BTreeMap<String, Vec<String>>,
    diag: &mut DiagnosticCollector,
) {
    for (target, mut fields) in grouped {
        fields.sort();
        fields.dedup();
        let field_refs: Vec<&str> = fields.iter().map(String::as_str).collect();
        let count = field_refs.len();
        let noun = if count == 1 { "field" } else { "fields" };
        diag.warn_with_category(
            code,
            format!(
                "{item_kind} `{item_name}`: {count} {noun} {classification_label} for {} ({})",
                target_label(&target),
                summarize_fields(&field_refs)
            ),
            DiagnosticCategory::Lossiness,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticLevel;

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
        let mut diag = DiagnosticCollector::new();
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
        assert!(warnings[0].message.contains("2 fields dropped for .opencode"));
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
        let mut diag = DiagnosticCollector::new();
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
    fn approximate_warnings_remain_per_field() {
        let lossy = vec![LossyField {
            field: "tools".into(),
            target: "Claude".into(),
            classification: Lossiness::Approximate {
                note: "unknown tool name passed through verbatim",
            },
        }];
        let mut diag = DiagnosticCollector::new();
        emit_agent_lossiness_warnings("coder", &lossy, &mut diag);
        let warnings = diag.drain();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "agent-field-approximate");
    }
}
