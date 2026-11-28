//! Target-state validation: skill reference checks and config-side dangle
//! detection after renames.
//!
//! Called from `build_target` against the post-prune, post-rewrite target state.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::diagnostic::{Diagnostic, DiagnosticCollector, DiagnosticLevel, Owner};
use crate::local_source::{LocalDiscoveredItem, LocalItemContent};
use crate::resolve::ResolvedGraph;
use crate::sync::LoadedConfig;
use crate::sync::layers::HostSkills;
use crate::sync::target::{
    CollisionRename, ExplicitRename, RenamableKind, TargetState, default_dest_path,
};
use crate::types::{DestPath, SourceName};
use mars_schema::item::{DefinitionKind, ItemKind};
use mars_schema::prepare::PreparedDefinition;

/// The project's own source, whose items are not a graph node: its `[package] uses-providers`
/// (`None` without a `[package]` table) and the folder its items are authored in, the package
/// root for a package and `.mars-src` otherwise.
pub(crate) struct OwnSource<'a> {
    pub dir: std::path::PathBuf,
    pub declared: Option<&'a [String]>,
}

impl<'a> OwnSource<'a> {
    pub(crate) fn new(layout: &crate::layout::Layout, declared: Option<&'a [String]>) -> Self {
        let dir = match declared {
            Some(_) => layout.root().to_path_buf(),
            None => layout.local_source(),
        };
        Self { dir, declared }
    }
}

/// The one judge of an Agent's skill refs in a sync and a plugin build, against what is
/// installed, and the only place that combines a package's declaration with this machine:
/// selection places refs and never judges them, and `provided` (the skills a host makes available
/// here, whatever the targets; `None` where they must not count, as in plugin builds) is read only
/// here. A ref's declaration is its source's
/// `[package] uses-providers`, `None` for a source without a `[package]` table.
pub(crate) fn validate_skill_refs(
    target: &TargetState,
    graph: &ResolvedGraph,
    provided: Option<&HostSkills>,
    own: &OwnSource<'_>,
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
        let (owner, declared) = match graph.nodes.get(&item.source_name) {
            Some(node) => (node.owner(), node.declared()),
            None => (Owner::Own { dir: &own.dir }, own.declared),
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
                provided_here: provided.is_some_and(|names| names.contains(skill_name)),
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

    let check = |name: &str, kind: ItemKind, location: &str, diag: &mut DiagnosticCollector| {
        let Some(new_names) = renamed.get(&(kind, name.to_string())) else {
            return;
        };
        if installed.contains(&(kind, name.to_string())) {
            return;
        }
        diag.warn_advisory(
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
                graph.owner(source_name),
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
                Owner::Own { dir: root },
            );
        }
    }
}

/// What an Agent's skill ref needs checked: `declared` is the `uses-providers` of the Agent's
/// source, `Some` exactly when that source has a `[package]` table; a source without one (a
/// consumer project's own items, the user level's `.mars-src`) needs no declaration.
/// `provided_here` is whether a host makes the name available on this machine
/// (`sync::layers::HostSkills`).
pub(crate) struct SkillRef<'a> {
    pub agent: &'a str,
    pub skill_name: &'a str,
    pub owner: Owner<'a>,
    pub declared: Option<&'a [String]>,
    pub provided_here: bool,
}

/// The report for one skill ref the installed skills do not satisfy, or `None` when they do.
/// The one place a ref is judged, so sync, validate and plugin builds word it alike.
pub(crate) fn classify_skill_ref(
    reference: &SkillRef<'_>,
    available_skills: &HashSet<String>,
) -> Option<Diagnostic> {
    let SkillRef {
        agent,
        skill_name,
        owner,
        declared,
        provided_here,
    } = *reference;
    if available_skills.contains(skill_name) {
        return None;
    }
    let own_level = match owner {
        Owner::Own { .. } | Owner::Path { .. } => DiagnosticLevel::Warning,
        Owner::Dependency(_) => DiagnosticLevel::Info,
    };
    let whose = owner.whose();
    Some(match mars_schema::provider::provider_of(skill_name) {
        // The package declares the provider: the ref is valid wherever it supplies the skill, and
        // says so where it does not.
        Some(provider)
            if mars_schema::provider::deferred_provider(skill_name, declared).is_some() =>
        {
            if provided_here {
                return None;
            }
            Diagnostic::new(
                DiagnosticLevel::Warning,
                "provided-skill-unavailable",
                format!(
                    "agent `{agent}`{whose} references skill `{skill_name}`, which provider \
                     `{provider}` supplies; it is not provided here, so the agent runs without it"
                ),
            )
        }
        // Provided here. An item whose source has no `[package]` table (a consumer project's own,
        // the user level's) needs nothing declared, so the ref is satisfied; a package that does not declare the provider
        // would not resolve it for consumers.
        Some(provider) if provided_here => {
            declared?;
            Diagnostic::new(
                own_level,
                "provided-skill-undeclared",
                format!(
                    "agent `{agent}`{whose} references skill `{skill_name}`, provided here by \
                     `{provider}`, but this package's `[package] uses-providers` does not list \
                     `{provider}`; consumers will not resolve it"
                ),
            )
        }
        _ => {
            let suggestion = find_suggestion(skill_name, available_skills)
                .map_or(String::new(), |name| format!(" (did you mean `{name}`?)"));
            let fix = match owner {
                Owner::Dependency(source) => format!(
                    "; its author has to ship the skill, and {} takes a newer release",
                    crate::diagnostic::newer_release(source)
                ),
                _ if !suggestion.is_empty() => String::new(),
                Owner::Own { dir } | Owner::Path { dir, .. } => {
                    match mars_schema::provider::provider_of(skill_name) {
                        // A package's consumers resolve the ref only if it is declared, as
                        // `mars check` says.
                        Some(provider) if declared.is_some() => format!(
                            "; {}",
                            mars_schema::provider::undeclared_provider_hint(provider)
                        ),
                        Some(provider) => format!(
                            "; no package ships it, and `{provider}` has not provided it here"
                        ),
                        None => format!(
                            "; create it in {}, or remove it from the agent's `skills`",
                            dir.join("skills").join(skill_name).display()
                        ),
                    }
                }
            };
            // The agent runs without a skill it declares, so this is a warning whoever shipped it.
            Diagnostic::new(
                DiagnosticLevel::Warning,
                "missing-skill",
                format!(
                    "agent `{agent}`{whose} references missing skill `{skill_name}`{suggestion}{fix}"
                ),
            )
        }
    })
}

pub(crate) fn find_suggestion(missing: &str, available: &HashSet<String>) -> Option<String> {
    let missing_lower = missing.to_lowercase();

    let mut candidates: Vec<&String> = available.iter().collect();
    candidates.sort();

    for name in candidates {
        let name_lower = name.to_lowercase();
        if name_lower.contains(&missing_lower) || missing_lower.contains(&name_lower) {
            return Some(name.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_reverse_substring() {
        let available: HashSet<String> = ["planning"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            find_suggestion("plan", &available),
            Some("planning".to_string())
        );
    }

    #[test]
    fn suggestion_forward_substring() {
        let available: HashSet<String> = ["review"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            find_suggestion("review-pr", &available),
            Some("review".to_string())
        );
    }

    #[test]
    fn suggestion_case_insensitive() {
        let available: HashSet<String> = ["Planning"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            find_suggestion("plan", &available),
            Some("Planning".to_string())
        );
    }

    #[test]
    fn no_suggestion_when_no_match() {
        let available: HashSet<String> = ["review"].iter().map(|s| s.to_string()).collect();
        assert_eq!(find_suggestion("completely-different", &available), None);
    }
}
