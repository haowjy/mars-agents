//! Target-state validation: skill reference checks and config-side dangle
//! detection after renames.
//!
//! Called from `build_target` against the post-prune, post-rewrite target state.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::diagnostic::DiagnosticCollector;
use crate::diagnostic::Owner;
use crate::local_source::{LocalDiscoveredItem, LocalItemContent};
use crate::lock::RecordRef;
use crate::provided::ProvidedNames;
use crate::resolve::ResolvedGraph;
use crate::sync::LoadedConfig;
use crate::sync::target::{
    CollisionRename, ExplicitRename, RenamableKind, TargetState, default_dest_path,
};
use crate::types::{DestPath, SourceName};
use crate::validate::{SkillRef, classify_skill_ref};
use mars_schema::item::{DefinitionKind, ItemKind};
use mars_schema::prepare::PreparedDefinition;

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
    held_back_skills: &HashSet<String>,
    diag: &mut DiagnosticCollector,
) {
    let available_skills: HashSet<String> = target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Skill)
        .map(|item| item.id.name.to_string())
        .chain(held_back_skills.iter().cloned())
        .collect();

    for item in target
        .items
        .values()
        .filter(|item| item.id.kind == ItemKind::Agent)
    {
        let owner = Owner::of(item.source_name.as_str());
        let declared = if owner == Owner::Own {
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
        let skills = mars_schema::agent::read(&content)
            .fields
            .map(|profile| profile.skills.all())
            .unwrap_or_default();
        for skill_name in &skills {
            let reference = SkillRef {
                agent: item.id.name.as_str(),
                skill_name,
                owner,
                declared,
                provided_here: provided.is_some_and(|names| names.provides(skill_name)),
            };
            if let Some(found) = classify_skill_ref(&reference, &available_skills) {
                diag.hold_until_written(owner, found);
            }
        }
    }
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

    // The warning is news when a sync installs one of the renamed items; the config it names
    // does not change the lock.
    let check = |name: &str, kind: ItemKind, location: String, diag: &mut DiagnosticCollector| {
        let Some(new_names) = renamed.get(&(kind, name.to_string())) else {
            return;
        };
        if installed.contains(&(kind, name.to_string())) {
            return;
        }
        diag.hold_warning(
            RecordRef::Any(
                new_names
                    .iter()
                    .map(|new| RecordRef::Item(format!("{kind}/{new}")))
                    .collect(),
            ),
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
            "[settings.meridian.fanout].agents".to_owned(),
            diag,
        );
    }
    for name in loaded.effective.agents.keys() {
        let files = overlay_files(loaded, |agents, _| agents.contains_key(name));
        check(
            name,
            ItemKind::Agent,
            format!("the [agents.<name>] overlay in {files}"),
            diag,
        );
    }
    for name in loaded.effective.skills.keys() {
        let files = overlay_files(loaded, |_, skills| skills.contains_key(name));
        check(
            name,
            ItemKind::Skill,
            format!("the [skills.<name>] overlay in {files}"),
            diag,
        );
    }
}

/// The config files that hold an overlay, since the effective config merges the project's
/// `mars.toml`, `mars.local.toml` and the user config.
fn overlay_files(
    loaded: &LoadedConfig,
    holds: impl Fn(
        &indexmap::IndexMap<String, crate::config::AgentOverlay>,
        &indexmap::IndexMap<String, crate::config::SkillOverlay>,
    ) -> bool,
) -> String {
    let mut files = Vec::new();
    if holds(&loaded.config.agents, &loaded.config.skills) {
        files.push("mars.toml".to_owned());
    }
    if holds(&loaded.local.agents, &loaded.local.skills) {
        files.push("mars.local.toml".to_owned());
    }
    if holds(&loaded.user.local.agents, &loaded.user.local.skills) {
        files.push(loaded.user.path.display().to_string());
    }
    files.join(", ")
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
                Owner::of(source_name.as_str()),
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
                Owner::Own,
            );
        }
    }
}
