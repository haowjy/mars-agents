//! Target-state validation: skill reference checks, frontmatter schema checks,
//! and config-side dangle detection after renames.
//!
//! Called from `build_target` against the post-prune, post-rewrite target state.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::diagnostic::DiagnosticCollector;
use crate::lock::ItemKind;
use crate::sync::LoadedConfig;
use crate::sync::target::{CollisionRename, ExplicitSkillRename, TargetState};
use crate::validate::ValidationWarning;

/// Validate skill references: check that agents' `skills:` frontmatter entries
/// reference skills that exist in the target state.
pub(crate) fn validate_skill_refs(target: &TargetState) -> Vec<ValidationWarning> {
    use crate::validate::{extract_skills_from_content, find_suggestion};

    let available_skills: HashSet<String> = target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Skill)
        .map(|item| item.id.name.to_string())
        .collect();

    let mut warnings = Vec::new();

    for item in target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Agent)
    {
        let content = match &item.rewritten_content {
            Some(content) => content.clone(),
            None => std::fs::read_to_string(&item.source_path).unwrap_or_default(),
        };
        for skill_name in extract_skills_from_content(&content) {
            if !available_skills.contains(&skill_name) {
                let suggestion = find_suggestion(&skill_name, &available_skills);
                warnings.push(ValidationWarning::MissingSkill {
                    agent: item.id.clone(),
                    skill_name,
                    suggestion,
                });
            }
        }
    }

    warnings
}

pub(crate) fn validate_skill_frontmatter_in_target(
    target: &TargetState,
    diag: &mut DiagnosticCollector,
) {
    for item in target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Skill)
    {
        validate_skill_frontmatter_at_source(&item.source_path, item.id.name.as_str(), diag);
    }
}

fn validate_skill_frontmatter_at_source(
    source_path: &Path,
    skill_name: &str,
    diag: &mut DiagnosticCollector,
) {
    let skill_md = if source_path.is_dir() {
        source_path.join("SKILL.md")
    } else {
        source_path.to_path_buf()
    };
    let Ok(content) = std::fs::read_to_string(&skill_md) else {
        return;
    };
    let mut skill_diags = Vec::new();
    let _ = crate::compiler::skills::parse_skill_content(&content, &mut skill_diags);
    crate::compiler::skills::emit_skill_schema_diags(diag, skill_name, &skill_diags);
}

pub(crate) fn warn_config_dangles_after_rename(
    explicit_skill_renames: &[ExplicitSkillRename],
    collision_renames: &[CollisionRename],
    target: &TargetState,
    loaded: &LoadedConfig,
    diag: &mut DiagnosticCollector,
) {
    let mut renamed: HashMap<(ItemKind, String), Vec<String>> = HashMap::new();
    for r in explicit_skill_renames {
        renamed
            .entry((ItemKind::Skill, r.original_name.to_string()))
            .or_default()
            .push(r.new_name.to_string());
    }
    for r in collision_renames {
        renamed
            .entry((r.kind, r.original_name.to_string()))
            .or_default()
            .push(r.new_name.to_string());
    }
    if renamed.is_empty() {
        return;
    }

    let installed: HashSet<(ItemKind, String)> = target
        .items
        .values()
        .map(|item| (item.id.kind, item.id.name.to_string()))
        .collect();

    let check = |name: &str, kind: ItemKind, location: &str, diag: &mut DiagnosticCollector| {
        let Some(new_names) = renamed.get(&(kind, name.to_string())) else {
            return;
        };
        if installed.contains(&(kind, name.to_string())) {
            return;
        }
        diag.warn(
            "config-rename-dangle",
            format!(
                "`{name}` in {location} no longer matches an installed {kind} after rename (now: {}); update the config",
                new_names.join(", "),
            ),
        );
    };

    for name in loaded.effective.settings.meridian_fanout_agents() {
        check(
            name,
            ItemKind::Agent,
            "[settings.meridian.fanout].agents",
            diag,
        );
    }
    let agent_overlay_names: HashSet<&String> = loaded
        .config
        .agents
        .keys()
        .chain(loaded.local.agents.keys())
        .collect();
    for name in agent_overlay_names {
        check(name, ItemKind::Agent, "[agents.<name>] overlay", diag);
    }
    for name in loaded.effective.skills.keys() {
        check(name, ItemKind::Skill, "[skills.<name>] overlay", diag);
    }
}
