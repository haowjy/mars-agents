//! Preview lossiness diagnostics for source-tree agents/skills without running sync.
//!
//! `mars check` and `mars init` call this after validation/scaffolding to surface
//! field-loss warnings against the project's configured target harnesses — the
//! same harness set the sync pipeline uses for native emission.

use std::path::{Path, PathBuf};

use crate::compiler::agents::lower::{NativeModel, lower_for_harness_with_model};
use crate::compiler::agents::{HarnessKind, parse_agent_profile};
use crate::compiler::lossiness::{
    emit_agent_lossiness_warnings, emit_skill_lossiness_warnings,
};
use crate::compiler::skills::lower::{SkillHarness, lower_skill_for_harness};
use crate::compiler::skills::parse_skill_content;
use crate::config::Settings;
use crate::diagnostic::{Diagnostic, DiagnosticCollector};
use crate::error::MarsError;
use crate::frontmatter;
use crate::lock::ItemKind;

/// Harnesses that would receive native artifacts during sync for these settings.
pub fn configured_emit_harnesses(settings: &Settings) -> Vec<HarnessKind> {
    settings
        .managed_targets()
        .iter()
        .filter_map(|t| HarnessKind::from_target_dir(t))
        .collect()
}

fn harness_to_skill_harness(harness: &HarnessKind) -> SkillHarness {
    match harness {
        HarnessKind::Claude => SkillHarness::Claude,
        HarnessKind::Codex => SkillHarness::Codex,
        HarnessKind::OpenCode => SkillHarness::OpenCode,
        HarnessKind::Pi => SkillHarness::Pi,
        HarnessKind::Cursor => SkillHarness::Cursor,
    }
}

/// Lower discovered source agents/skills against configured target harnesses and
/// return lossiness diagnostics.
pub fn collect_source_lossiness_diagnostics(
    base: &Path,
    settings: &Settings,
) -> Result<Vec<Diagnostic>, MarsError> {
    let harnesses = configured_emit_harnesses(settings);
    if harnesses.is_empty() {
        return Ok(Vec::new());
    }

    let discovered = crate::discover::discover_resolved_source(base, None)?;
    let mut diag = DiagnosticCollector::new();

    for item in discovered {
        match item.id.kind {
            ItemKind::Agent => {
                let path = base.join(&item.source_path);
                collect_agent_lossiness(&path, &item.id.name, &harnesses, &mut diag);
            }
            ItemKind::Skill => {
                let (skill_md, fallback_name) = skill_markdown_path(base, &item);
                collect_skill_lossiness(&skill_md, &fallback_name, &harnesses, &mut diag);
            }
            ItemKind::Hook | ItemKind::McpServer | ItemKind::BootstrapDoc => {}
        }
    }

    Ok(diag.drain())
}

fn skill_markdown_path(base: &Path, item: &crate::discover::DiscoveredItem) -> (PathBuf, String) {
    if item.source_path == Path::new(".") {
        (base.join("SKILL.md"), item.id.name.to_string())
    } else {
        let path = base.join(&item.source_path);
        let dirname = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        (path.join("SKILL.md"), dirname)
    }
}

fn collect_agent_lossiness(
    path: &Path,
    fallback_name: &str,
    harnesses: &[HarnessKind],
    diag: &mut DiagnosticCollector,
) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(fm) = frontmatter::parse(&content) else {
        return;
    };
    let mut agent_diags = Vec::new();
    let profile = parse_agent_profile(&fm, &mut agent_diags);
    if agent_diags.iter().any(|d| d.is_error()) {
        return;
    }
    let name = fm.name().unwrap_or(fallback_name);
    let body = fm.body();
    for harness in harnesses {
        let lowered = lower_for_harness_with_model(
            harness,
            &profile,
            &fm,
            body,
            &NativeModel::Clear,
        );
        emit_agent_lossiness_warnings(name, &lowered.lossy_fields, diag);
    }
}

fn collect_skill_lossiness(
    skill_md: &Path,
    fallback_name: &str,
    harnesses: &[HarnessKind],
    diag: &mut DiagnosticCollector,
) {
    let Ok(content) = std::fs::read_to_string(skill_md) else {
        return;
    };
    let mut skill_diags = Vec::new();
    let Ok((profile, fm)) = parse_skill_content(&content, &mut skill_diags) else {
        return;
    };
    if skill_diags.iter().any(|d| d.is_error()) {
        return;
    }
    let name = profile
        .name
        .as_deref()
        .or_else(|| fm.name())
        .unwrap_or(fallback_name);
    let body = fm.body();
    for harness in harnesses {
        let lowered = lower_skill_for_harness(harness_to_skill_harness(harness), &profile, body);
        emit_skill_lossiness_warnings(name, &lowered.lossy_fields, diag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use crate::diagnostic::DiagnosticCategory;
    use tempfile::TempDir;

    #[test]
    fn configured_emit_harnesses_uses_managed_targets() {
        let mut settings = Settings::default();
        settings.targets = Some(vec![".cursor".into(), ".agents".into()]);
        let harnesses = configured_emit_harnesses(&settings);
        assert_eq!(harnesses, vec![HarnessKind::Cursor]);
    }

    #[test]
    fn collect_source_lossiness_empty_without_targets() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("agents")).unwrap();
        std::fs::write(
            dir.path().join("agents/coder.md"),
            "---\nname: coder\ndescription: test\nharness-overrides:\n  cursor:\n    native-config:\n      x: true\n---\n# Coder",
        )
        .unwrap();

        let diags = collect_source_lossiness_diagnostics(dir.path(), &Settings::default()).unwrap();
        assert!(diags.is_empty());
    }

    #[test]
    fn collect_source_lossiness_reports_agent_field_loss() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[settings]\ntargets = [\".cursor\"]\nagent_emission = \"always\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("agents")).unwrap();
        std::fs::write(
            dir.path().join("agents/worker.md"),
            "---\nname: worker\ndescription: test\nharness-overrides:\n  cursor:\n    native-config:\n      cursor.only: true\n---\n# Worker",
        )
        .unwrap();

        let settings = crate::config::load(dir.path()).unwrap().settings;
        let diags = collect_source_lossiness_diagnostics(dir.path(), &settings).unwrap();
        assert!(
            diags.iter().any(|d| {
                d.category == Some(DiagnosticCategory::Lossiness)
                    && d.message.contains("native-config")
            }),
            "expected lossiness diagnostic: {diags:?}"
        );
    }
}
