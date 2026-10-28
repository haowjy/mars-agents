//! Target-state validation: skill reference checks and config-side dangle
//! detection after renames.
//!
//! Called from `build_target` against the post-prune, post-rewrite target state.

use std::collections::{HashMap, HashSet};

use crate::diagnostic::DiagnosticCollector;
use crate::provided::ProvidedNames;
use crate::resolve::ResolvedGraph;
use crate::sync::LoadedConfig;
use crate::sync::target::{CollisionRename, ExplicitRename, TargetState};
use crate::types::{SourceName, SourceOrigin};
use crate::validate::ValidationWarning;
use mars_schema::item::ItemKind;
use mars_schema::provider::deferred_provider;

/// Validate skill references: check that agents' `skills:` frontmatter entries
/// reference skills that exist in the target state.
///
/// The one reporter of refs against what is installed, and the only place that combines a
/// package's declaration with this machine: the resolver never reports a ref it defers to a
/// declared provider, and `provided` (`None` where provided skills must not count, as in plugin
/// builds) is read only here. `local_declared` is the `[package] uses-providers` of the project's
/// own items, `None` for a consumer project without a `[package]` table.
pub(crate) fn validate_skill_refs(
    target: &TargetState,
    graph: &ResolvedGraph,
    provided: Option<&ProvidedNames>,
    local_declared: Option<&[String]>,
) -> Vec<ValidationWarning> {
    use crate::validate::find_suggestion;

    let available_skills: HashSet<String> = target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Skill)
        .map(|item| item.id.name.to_string())
        .collect();
    let local_source = SourceName::from(SourceOrigin::LocalPackage.to_string());

    let mut warnings = Vec::new();

    for item in target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Agent)
    {
        let declared = if item.source_name == local_source {
            local_declared
        } else {
            Some(
                graph
                    .nodes
                    .get(&item.source_name)
                    .and_then(|node| node.manifest.as_ref())
                    .map_or(&[][..], |manifest| {
                        manifest.package.uses_providers.as_slice()
                    }),
            )
        };
        let content = match &item.rewritten_content {
            Some(content) => content.clone(),
            None => std::fs::read_to_string(&item.source_path).unwrap_or_default(),
        };
        // An Agent already in schema v1: it was prepared, then possibly rewritten.
        let skills = mars_schema::agent::read(&content)
            .fields
            .map(|profile| profile.skills.all())
            .unwrap_or_default();
        for skill_name in skills {
            if available_skills.contains(&skill_name) {
                continue;
            }
            let agent = item.id.clone();
            let provided_here = provided.is_some_and(|names| names.provides(&skill_name));
            let warning = match mars_schema::provider::provider_of(&skill_name) {
                // The package declares the provider: the ref is valid wherever it supplies the
                // skill, and says so where it does not.
                Some(provider) if deferred_provider(&skill_name, declared).is_some() => {
                    if provided_here {
                        continue;
                    }
                    ValidationWarning::ProvidedSkillUnavailable {
                        agent,
                        provider: provider.to_string(),
                        skill_name,
                    }
                }
                // Provided here. A consumer project's own items have no `[package]` table, so
                // nothing needs declaring and the ref is satisfied; a package that does not
                // declare the provider would not resolve it for consumers.
                Some(provider) if provided_here => {
                    if declared.is_none() {
                        continue;
                    }
                    ValidationWarning::ProvidedSkillUndeclared {
                        agent,
                        provider: provider.to_string(),
                        skill_name,
                    }
                }
                _ => ValidationWarning::MissingSkill {
                    agent,
                    suggestion: find_suggestion(&skill_name, &available_skills),
                    skill_name,
                },
            };
            warnings.push(warning);
        }
    }

    warnings
}

pub(crate) fn warn_config_dangles_after_rename(
    explicit_renames: &[ExplicitRename],
    collision_renames: &[CollisionRename],
    target: &TargetState,
    loaded: &LoadedConfig,
    diag: &mut DiagnosticCollector,
) {
    let mut renamed: HashMap<(ItemKind, String), Vec<String>> = HashMap::new();
    for r in explicit_renames {
        renamed
            .entry((r.kind, r.original_name.to_string()))
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
    for name in loaded.effective.agents.keys() {
        check(name, ItemKind::Agent, "[agents.<name>] overlay", diag);
    }
    for name in loaded.effective.skills.keys() {
        check(name, ItemKind::Skill, "[skills.<name>] overlay", diag);
    }
}
