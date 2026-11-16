//! Dependency resolution: which version of which source, for the whole graph.
//!
//! The rules live in `mars_schema::resolve`, which Meridian Flow drives too. This module builds
//! its request from the configuration, answers its lookups with git and the disk
//! ([`CliAnswers`]), and then does what the core leaves to the binary on the graph it returns:
//! reports each package's files, finds its hooks and items, and selects what the graph installs.

mod answers;
mod provider;
mod publish;
mod requires;
mod select;
mod types;
mod upgrade;

use std::collections::BTreeMap;
use std::sync::Arc;

use indexmap::IndexMap;
use mars_schema::resolve::{
    Bounds, Dependency, Diagnostic as Note, Level, Mode, Outcome, Request, Resolved, Spec,
};

#[cfg(test)]
pub(crate) use answers::apply_subpath;
use answers::{CliAnswers, mars_error};
pub(crate) use provider::RealSourceProvider;
pub use provider::SourceProvider;
pub use publish::PublishLens;
pub(crate) use publish::resolve_publish_graph_from_config;
pub(crate) use requires::check_consumer_package_requirements;
pub(crate) use select::portable_facts;
#[cfg(test)]
pub(crate) use select::select_for_test;
pub use types::*;
pub(crate) use upgrade::upgrade_candidate;

use crate::config::{EffectiveConfig, SourceSpec};
use crate::diagnostic::DiagnosticCollector;
use crate::discover;
use crate::error::{MarsError, ResolutionError};
use crate::lock::LockFile;
use crate::types::{CommitHash, SourceId, SourceName, SourceUrl};
use mars_schema::item::ItemKind;
use mars_schema::names::ItemName;

/// A resolved package with the items discovery found in its files.
#[derive(Debug, Clone)]
pub(crate) struct RegisteredPackage {
    pub(crate) node: ResolvedNode,
    pub(crate) items: IndexMap<(ItemKind, ItemName), discover::DiscoveredItem>,
}

impl RegisteredPackage {
    pub(crate) fn items(&self) -> impl Iterator<Item = &discover::DiscoveredItem> {
        self.items.values()
    }
}

/// Resolve `config`'s dependencies: the core walks the graph, asking `provider` through
/// [`CliAnswers`] for tags, refs and trees as it needs them.
pub fn resolve(
    config: &EffectiveConfig,
    provider: &dyn SourceProvider,
    locked: Option<&LockFile>,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedGraph, MarsError> {
    let request = request(config, locked, options);
    let answers = CliAnswers::new(provider);
    match mars_schema::resolve::resolve(&request, &answers) {
        Outcome::Needs(needs) => Err(MarsError::Internal(format!(
            "resolution asked for answers the CLI does not leave open: {needs:?}"
        ))),
        Outcome::Failed(failed) => {
            for note in &failed.diagnostics {
                replay(note, diag);
            }
            Err(match mars_error(failed.failure, &answers) {
                // Only `--frozen` reads a commit from the lock and stops at the first one gone;
                // a plain sync re-resolves, so this is the one place the lock is the cause.
                MarsError::CommitUnreachable { commit, url }
                    if matches!(options.mode, ResolveMode::Frozen) =>
                {
                    MarsError::FrozenViolation {
                        message: format!(
                            "locked commit {commit} is no longer reachable in {url} (the tag may have been force-pushed); run `mars sync` without --frozen to re-resolve it, or `mars upgrade` to move to the newest version"
                        ),
                    }
                }
                error => error,
            })
        }
        Outcome::Resolved(resolved) => finish(config, options, resolved, &answers, diag),
    }
}

fn request(
    config: &EffectiveConfig,
    locked: Option<&LockFile>,
    options: &ResolveOptions,
) -> Request {
    let root = config
        .dependencies
        .iter()
        .map(|(name, dependency)| {
            let spec = match &dependency.spec {
                SourceSpec::Git(git) => Spec::Git {
                    url: git.url.to_string(),
                    version: git.version.clone(),
                },
                SourceSpec::Path(path) => Spec::Path(path.to_string_lossy().into_owned()),
            };
            // A local override replaces the source; the declared one only names the identity.
            let declared = match (&dependency.declared_source_id, &dependency.source_id) {
                (declared, actual) if declared == actual => spec.clone(),
                (SourceId::Git { url, .. }, _) => Spec::Git {
                    url: url.to_string(),
                    version: None,
                },
                (SourceId::Path { canonical, .. }, _) => {
                    Spec::Path(canonical.to_string_lossy().into_owned())
                }
            };
            Dependency {
                name: name.to_string(),
                constraint: options.direct_constraint_for(name, spec.constraint()),
                declared,
                spec,
                subpath: dependency.subpath.clone(),
                plugin: dependency.source_id.plugin().map(str::to_owned),
                required_by: "mars.toml".to_string(),
                declared_in: None,
                dialect: dependency.dialect,
            }
        })
        .collect();
    Request {
        root,
        supplied: options
            .prepared_sources
            .iter()
            .map(|(name, prepared)| (name.to_string(), Arc::clone(prepared)))
            .collect(),
        overrides: options
            .source_overrides
            .iter()
            .map(|(name, path)| (name.to_string(), path.to_string_lossy().into_owned()))
            .collect(),
        locked: locked
            .map(|lock| {
                lock.dependencies
                    .iter()
                    .map(|(name, source)| (name.to_string(), source.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        mode: match &options.mode {
            ResolveMode::Sync => Mode::Sync,
            ResolveMode::Frozen => Mode::Frozen,
            ResolveMode::Upgrade { targets, .. } => Mode::Upgrade {
                targets: targets.iter().map(ToString::to_string).collect(),
            },
        },
        engine: requires::engine(options),
        bounds: Bounds::default(),
    }
}

fn replay(note: &Note, diag: &mut DiagnosticCollector) {
    let Note::Note {
        level,
        code,
        message,
        once,
    } = note
    else {
        return;
    };
    match (level, once) {
        (Level::Warning, true) => diag.warn_once(code.clone(), message.clone()),
        (Level::Warning, false) => diag.warn(code.clone(), message.clone()),
        (Level::Advisory, _) => diag.warn_advisory(code.clone(), message.clone()),
        (Level::Info, _) => diag.info(code.clone(), message.clone()),
    }
}

/// What the core leaves to the binary, package by package in the order the core registered them:
/// report what preparing the files found, read the manifest the way the rest of the binary does,
/// and find the hooks and items beside the schema's.
fn finish(
    config: &EffectiveConfig,
    options: &ResolveOptions,
    resolved: Resolved,
    answers: &CliAnswers<'_>,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedGraph, MarsError> {
    let mut registry: IndexMap<SourceName, RegisteredPackage> = IndexMap::new();
    let mut unreadable_hook_surfaces = BTreeMap::new();
    for note in &resolved.diagnostics {
        let Note::Registered { package } = note else {
            replay(note, diag);
            continue;
        };
        let core = &resolved.nodes[package];
        let node = node(core, answers)?;
        if !options.prepared_sources.contains_key(&node.source_name) {
            crate::package_prepare::report_dependency(
                config,
                &node.source_name,
                node.resolved_ref.version.as_ref(),
                options.is_upgrade_target(&node.source_name),
                &node.prepared,
                &core.normalized,
                diag,
            );
        }
        if let crate::staging::HookSurfaceState::Unreadable { hook_names } =
            crate::staging::detect_unreadable_hook_surface(&node.rooted_ref.authored_root, diag)?
        {
            unreadable_hook_surfaces.insert(node.source_name.clone(), hook_names);
        }
        let items = discover::package_items(
            &node.rooted_ref.authored_root,
            Some(node.source_name.as_ref()),
            &node.prepared,
            None,
            diag,
        )?
        .into_iter()
        .map(|item| ((item.id.kind, item.id.name.clone()), item))
        .collect();
        registry.insert(node.source_name.clone(), RegisteredPackage { node, items });
    }

    let selection = select::select_registry(config, &registry, options.root.as_deref());
    // With a root, the caller judges what the selection found: refs it could not resolve and
    // slugs two packages install (`mars check`, through `mars_schema::check`).
    if options.root.is_none() {
        for diagnostic in &selection.diagnostics {
            diag.schema(diagnostic, "selection", false);
        }
        if let Some(missing) = selection.missing_skills.first() {
            return Err(ResolutionError::SkillNotFound {
                skill: missing.skill.clone(),
                required_by: match &missing.package {
                    Some(package) => format!("{package}/{}", missing.agent),
                    None => missing.agent.clone(),
                },
                searched: missing.searched.clone(),
                hint: missing.hint.clone(),
            }
            .into());
        }
    }

    let nodes: IndexMap<SourceName, ResolvedNode> = registry
        .into_iter()
        .map(|(name, package)| (name, package.node))
        .collect();
    let mut order: Vec<SourceName> = nodes.keys().cloned().collect();
    order.sort();
    for fallback in &resolved.engine_fallbacks {
        diag.record_engine_fallback(engine_fallback(fallback));
    }
    Ok(ResolvedGraph {
        nodes,
        order,
        selection,
        version_constraints: resolved
            .constraints
            .into_iter()
            .map(|(name, constraints)| (SourceName::from(name), constraints))
            .collect(),
        unreadable_hook_surfaces,
    })
}

fn engine_fallback(fallback: &mars_schema::resolve::EngineFallback) -> EngineFallback {
    let mut engines: Vec<String> = Vec::new();
    let skipped = fallback
        .skipped
        .iter()
        .map(|skipped| EngineFallbackSkippedVersion {
            version: skipped.version.clone(),
            requirements: skipped
                .failures
                .iter()
                .map(|failure| {
                    let engine = failure.engine.to_string();
                    if !engines.contains(&engine) {
                        engines.push(engine.clone());
                    }
                    EngineFallbackRequirement {
                        engine,
                        requirement: failure.requirement.clone(),
                    }
                })
                .collect(),
        })
        .collect();
    EngineFallback {
        source: fallback.source.clone(),
        skipped,
        selected_version: fallback.selected_version.clone(),
        engines,
    }
}

/// The graph node for a package the core resolved: the directories its files are in, and the
/// manifest read as the rest of the binary reads one.
fn node(
    core: &mars_schema::resolve::Node,
    answers: &CliAnswers<'_>,
) -> Result<ResolvedNode, MarsError> {
    let authored_root = answers.rooted_dir(&core.rooted);
    let manifest = core
        .prepared
        .manifest
        .as_ref()
        .and_then(|manifest| {
            std::str::from_utf8(&manifest.normalized_bytes)
                .ok()
                .map(|content| (manifest, content))
        })
        .map(|(manifest, content)| {
            crate::config::parse_prepared_manifest(
                content,
                &authored_root.join("mars.toml"),
                &manifest.reading,
            )
        })
        .transpose()?
        .flatten();
    let name = SourceName::from(core.name.as_str());
    Ok(ResolvedNode {
        source_name: name.clone(),
        source_id: SourceId::from(&core.id),
        fetch_url: core.fetch_url.as_deref().map(SourceUrl::from),
        rooted_ref: RootedSourceRef {
            checkout_root: answers.checkout_dir(&core.rooted.checkout),
            authored_root: authored_root.clone(),
            package_root: authored_root,
            plugin: core.plugin.clone(),
        },
        prepared: Arc::clone(&core.prepared),
        resolved_ref: crate::source::ResolvedRef {
            source_name: name,
            version: core.resolved.version.clone(),
            version_tag: core.resolved.tag.clone(),
            commit: core.resolved.commit.as_deref().map(CommitHash::from),
            tree_path: answers.checkout_dir(&core.resolved.origin),
        },
        manifest,
    })
}

#[cfg(test)]
mod tests;
