//! Skill lowering and binary diagnostic integration.

pub mod lower;

use crate::diagnostic::{DiagnosticCategory, DiagnosticCollector};
pub use mars_schema::skill::*;
use std::path::Path;

pub(crate) fn emit_skill_schema_diags(
    diag: &mut DiagnosticCollector,
    skill_name: &str,
    skill_diags: &[mars_schema::diagnostic::SchemaDiagnostic],
) {
    for d in skill_diags {
        diag.schema(d, &format!("skill `{skill_name}`"), false);
    }
}

pub(crate) fn emit_skill_schema_diags_from_source(
    source_path: &Path,
    skill_name: &str,
    diag: &mut DiagnosticCollector,
) {
    emit_skill_schema_diags_from_source_with(
        source_path,
        skill_name,
        diag,
        SkillDiagnostic::message,
    );
}

pub(crate) fn emit_plugin_skill_schema_diags_from_source(
    source_path: &Path,
    skill_name: &str,
    diag: &mut DiagnosticCollector,
) {
    emit_skill_schema_diags_from_source_with(
        source_path,
        skill_name,
        diag,
        SkillDiagnostic::plugin_message,
    );
}

fn emit_skill_schema_diags_from_source_with(
    source_path: &Path,
    skill_name: &str,
    diag: &mut DiagnosticCollector,
    message: fn(&SkillDiagnostic) -> String,
) {
    let skill_md = if source_path.is_dir() {
        source_path.join("SKILL.md")
    } else {
        source_path.to_path_buf()
    };
    let Ok(content) = std::fs::read_to_string(skill_md) else {
        return;
    };
    let mut skill_diags = Vec::new();
    let _ = parse_skill_content_legacy(&content, &mut skill_diags);
    for d in &skill_diags {
        let message = format!("skill `{skill_name}`: {}", message(d));
        if d.is_error() {
            diag.error_with_category(
                "skill-schema-error",
                message,
                DiagnosticCategory::Validation,
            );
        } else {
            diag.warn("skill-schema-warning", message);
        }
    }
}
