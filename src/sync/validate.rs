//! Target-state validation: skill reference checks and config-side dangle
//! detection after renames.
//!
//! Called from `build_target` against the post-prune, post-rewrite target state.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::diagnostic::DiagnosticCollector;
use crate::local_source::{LocalDiscoveredItem, LocalItemContent};
use crate::provided::ProvidedNames;
use crate::resolve::ResolvedGraph;
use crate::sync::LoadedConfig;
use crate::sync::target::{
    CollisionRename, ExplicitRename, RenamableKind, TargetState, default_dest_path,
};
use crate::types::{DestPath, SourceName, SourceOrigin};
use crate::validate::ValidationWarning;
use mars_schema::item::{DefinitionKind, ItemKind};
use mars_schema::prepare::PreparedDefinition;

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
        warnings.extend(skills.into_iter().filter_map(|skill_name| {
            let provided_here = provided.is_some_and(|names| names.provides(&skill_name));
            crate::validate::classify_skill_ref(
                item.id.clone(),
                skill_name,
                &available_skills,
                declared,
                provided_here,
            )
        }));
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

/// Report the body links of every installed skill that stop resolving once the skills are
/// installed, judged against the whole installed tree (`target.items`, every source) after the
/// consumer's filters, explicit renames and collision renames. That is the graph's packages
/// (a plugin build's root among them) and `local_items`, the project's own `.mars-src` and
/// package skills, which sync merged into `target`. A skill that is not installed has no links
/// checked, though it can still be a link target. The rule and its coverage are
/// `mars_schema::links::check`.
pub(crate) fn report_skill_links(
    target: &TargetState,
    graph: &ResolvedGraph,
    local_items: &[LocalDiscoveredItem],
    diag: &mut DiagnosticCollector,
) {
    let installed: Vec<&str> = target
        .items
        .values()
        .map(|item| item.dest_path.as_str())
        .collect();
    let skill_dests: HashMap<(&SourceName, &Path), &DestPath> = target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Skill)
        .map(|item| {
            (
                (&item.source_name, item.source_path.as_path()),
                &item.dest_path,
            )
        })
        .collect();

    for (source_name, node) in &graph.nodes {
        let skills = node
            .prepared
            .definitions
            .iter()
            .map(|definition| {
                let item = crate::discover::definition_item(definition, &definition.output_path);
                let dest = skill_dests
                    .get(&(
                        source_name,
                        node.rooted_ref
                            .package_root
                            .join(item.source_path)
                            .as_path(),
                    ))
                    .map(|dest| dest.as_str());
                (definition, dest)
            })
            .collect::<Vec<_>>();
        let package = crate::package_prepare::dependency_name(&node.prepared, source_name);
        for (definition, diagnostic) in mars_schema::links::check(&skills, &installed) {
            diag.schema(
                &diagnostic,
                &format!("{package}: {}", definition.source_path),
                false,
            );
        }
    }

    // Each authored root is its own package: a link's target is named by that root's layout.
    let mut roots: Vec<(&Path, Vec<(&PreparedDefinition, DestPath)>)> = Vec::new();
    for item in local_items {
        let LocalItemContent::Prepared { definition, .. } = &item.content else {
            continue;
        };
        if definition.kind != DefinitionKind::Skill {
            continue;
        }
        let dest = default_dest_path(RenamableKind::Skill, &definition.slug);
        match roots.iter_mut().find(|(root, _)| *root == item.root) {
            Some((_, skills)) => skills.push((definition, dest)),
            None => roots.push((&item.root, vec![(definition, dest)])),
        }
    }
    for (root, skills) in &roots {
        let skills = skills
            .iter()
            .map(|(definition, dest)| (*definition, Some(dest.as_str())))
            .collect::<Vec<_>>();
        for (definition, diagnostic) in mars_schema::links::check(&skills, &installed) {
            diag.schema(
                &diagnostic,
                &crate::package_prepare::source_label(root)(&definition.source_path),
                false,
            );
        }
    }
}
