//! Skill lowering and binary diagnostic integration.

pub mod lower;

use std::path::Path;

use crate::diagnostic::DiagnosticCollector;

pub(crate) fn emit_skill_schema_diags(
    diag: &mut DiagnosticCollector,
    skill_name: &str,
    skill_diags: &[mars_schema::diagnostic::SchemaDiagnostic],
) {
    for diagnostic in skill_diags {
        diag.schema(diagnostic, &format!("skill `{skill_name}`"), false);
    }
}

pub(crate) fn emit_skill_schema_diags_from_source(
    source_path: &Path,
    skill_name: &str,
    diag: &mut DiagnosticCollector,
) {
    emit_skill_schema_diags_from_source_with(source_path, skill_name, diag);
}

pub(crate) fn emit_plugin_skill_schema_diags_from_source(
    source_path: &Path,
    skill_name: &str,
    diag: &mut DiagnosticCollector,
) {
    emit_skill_schema_diags_from_source_with(source_path, skill_name, diag);
}

fn emit_skill_schema_diags_from_source_with(
    source_path: &Path,
    skill_name: &str,
    diag: &mut DiagnosticCollector,
) {
    let skill_md = if source_path.is_dir() {
        source_path.join("SKILL.md")
    } else {
        source_path.to_path_buf()
    };
    let Ok(content) = std::fs::read_to_string(skill_md) else {
        return;
    };
    let reading = mars_schema::skill::read(&content);
    emit_skill_schema_diags(diag, skill_name, &reading.diagnostics);
}
