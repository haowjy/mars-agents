use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::config::{EffectiveConfig, FilterMode, GitSpec, Manifest, SourceSpec};
use crate::diagnostic::DiagnosticCollector;
use crate::dialect::Dialect;
use crate::discover;
use crate::error::{ConfigError, MarsError, ResolutionError};
use crate::lock::{ItemKind, LockFile};
use crate::staging;
use crate::types::{ItemName, SourceId, SourceName, SourceSubpath};
use indexmap::IndexMap;

use super::EngineExclusions;
use super::SourceProvider;
use super::constraint::parse_version_constraint;
use super::context::ResolverContext;
use super::filter::is_unfiltered_request;
use super::path::{apply_subpath, source_id_for_pending_spec};
use super::requires::{EngineRequirementFailure, check_package_requirements};
use super::types::{PendingItem, ResolveOptions, ResolvedNode, VersionConstraint};
use super::version::resolve_single_source;

/// Internal: a source waiting to be resolved.
#[derive(Debug, Clone)]
pub(crate) struct PendingSource {
    pub(crate) name: SourceName,
    /// Identity declared by the requesting manifest, before any local override.
    pub(crate) declared_source_id: SourceId,
    /// Effective identity fetched for this run (possibly a local override).
    pub(crate) source_id: SourceId,
    pub(crate) spec: SourceSpec,
    pub(crate) subpath: Option<SourceSubpath>,
    pub(crate) constraint: VersionConstraint,
    pub(crate) filter: FilterMode,
    pub(crate) required_by: String,
}

#[derive(Debug, Default)]
pub(crate) enum PackageResolutionState {
    #[default]
    Resolved,
    Resolving {
        deferred_seed_requests: Vec<PendingSource>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct RegisteredPackage {
    pub(crate) node: ResolvedNode,
    pub(crate) declared_source_id: SourceId,
    pub(crate) items: IndexMap<(ItemKind, ItemName), discover::DiscoveredItem>,
    pub(crate) constraint: VersionConstraint,
    pub(crate) is_local: bool,
}

impl RegisteredPackage {
    pub(crate) fn items(&self) -> impl Iterator<Item = &discover::DiscoveredItem> {
        self.items.values()
    }

    pub(crate) fn item(
        &self,
        kind: ItemKind,
        name: &ItemName,
    ) -> Option<&discover::DiscoveredItem> {
        self.items.get(&(kind, name.clone()))
    }

    pub(crate) fn has_skill(&self, skill: &ItemName) -> bool {
        self.skill_names().any(|name| name == skill)
    }

    pub(crate) fn skill_names(&self) -> impl Iterator<Item = &ItemName> {
        self.items
            .keys()
            .filter(|(kind, _)| *kind == ItemKind::Skill)
            .map(|(_, name)| name)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_package_bottom_up(
    pending_src: &PendingSource,
    seed_items: bool,
    provider: &dyn SourceProvider,
    locked: Option<&LockFile>,
    options: &ResolveOptions,
    effective_config: &EffectiveConfig,
    diag: &mut DiagnosticCollector,
    ctx: &mut ResolverContext,
    exclusions: &mut EngineExclusions,
) -> Result<(), MarsError> {
    if let Some(existing_name) = ctx.id_index().get(&pending_src.source_id)
        && existing_name != &pending_src.name
    {
        return Err(ResolutionError::DuplicateSourceIdentity {
            existing_name: existing_name.to_string(),
            duplicate_name: pending_src.name.to_string(),
            source_id: pending_src.source_id.to_string(),
        }
        .into());
    }

    if let Some(existing_package) = ctx.registry().get(&pending_src.name)
        && existing_package.declared_source_id != pending_src.declared_source_id
    {
        return Err(ResolutionError::SourceIdentityMismatch {
            name: pending_src.name.to_string(),
            existing: existing_package.declared_source_id.to_string(),
            incoming: pending_src.declared_source_id.to_string(),
        }
        .into());
    }

    ctx.add_version_constraint(
        &pending_src.name,
        &pending_src.required_by,
        pending_src.constraint.clone(),
    );
    if seed_items {
        ctx.add_filter(&pending_src.name, pending_src.filter.clone());
    }

    if matches!(
        ctx.package_states().get(&pending_src.name),
        Some(PackageResolutionState::Resolved)
    ) {
        // Re-resolution check: when a new constraint arrives on an already-resolved
        // git package, check whether the full accumulated constraint set (now including
        // the new constraint) would select a different version or commit than what is
        // currently in the registry.
        //
        // If so, emit `ResolutionRestartNeeded` — the driver will start a fresh
        // `ResolverContext`, carry the correct (new) ref as an override, and re-run
        // the bottom-up phase from scratch. B1 (stale manifest-derived constraints) and
        // B2 (new deps not materialized) are both avoided by construction: the fresh
        // context has no stale state, and the override is used at first-resolution time
        // (which runs the normal seeding path).
        //
        // Fast paths that skip the check:
        //   Path sources — no version selection, never change.
        //   RefPin constraints — fixed refs, semver re-selection doesn't apply.
        //   Semver(req) where existing version already satisfies req — adding that
        //     requirement cannot invalidate the currently selected version.
        let needs_check = matches!(pending_src.spec, SourceSpec::Git(_))
            && !matches!(pending_src.constraint, VersionConstraint::RefPin(_));

        if needs_check {
            let existing_ref = ctx
                .registry()
                .get(&pending_src.name)
                .map(|p| p.node.resolved_ref.clone());

            // Fast path: semver constraint already satisfied → selection cannot change.
            let skip = match (&pending_src.constraint, &existing_ref) {
                (VersionConstraint::Semver(req), Some(ref_)) => {
                    ref_.version.as_ref().is_some_and(|v| req.matches(v))
                }
                _ => false, // Latest or no existing ref → must run full check
            };

            if !skip {
                let new_ref = match resolve_single_source(
                    pending_src,
                    provider,
                    locked,
                    options,
                    ctx.version_constraints(),
                    exclusions,
                    diag,
                ) {
                    Ok(resolved) => resolved,
                    Err(error @ MarsError::Resolution(ResolutionError::VersionConflict { .. })) => {
                        return Err(engine_unsatisfiable_error(
                            &pending_src.name,
                            exclusions,
                            ctx.version_constraints(),
                        )
                        .map(MarsError::Resolution)
                        .unwrap_or(error));
                    }
                    Err(error) => return Err(error),
                };

                // Compare version AND commit (N3: same semver, different commit when
                // maximize policy changes after a locked-commit first-pass).
                let ref_changed = existing_ref.as_ref().is_none_or(|existing| {
                    new_ref.version != existing.version
                        || new_ref.commit != existing.commit
                        || new_ref.tree_path != existing.tree_path
                });

                if ref_changed {
                    let new_rooted = apply_subpath(
                        &pending_src.name,
                        &new_ref.tree_path,
                        pending_src.subpath.as_ref(),
                    )?;
                    let staged = stage_rooted_package(
                        &pending_src.name,
                        new_rooted,
                        effective_config,
                        options,
                        diag,
                    )?;
                    ctx.set_pending_restart(
                        pending_src.name.clone(),
                        new_ref,
                        staged.rooted,
                        staged.hook_surface,
                    );
                    return Err(MarsError::ResolutionRestartNeeded {
                        package: pending_src.name.to_string(),
                    });
                }
            }
        }

        if seed_items {
            let package =
                ctx.registry()
                    .get(&pending_src.name)
                    .ok_or_else(|| MarsError::Source {
                        source_name: pending_src.name.to_string(),
                        message: "resolved package missing from registry".to_string(),
                    })?;
            for pending_item in seed_items_for_request(pending_src, package) {
                ctx.push_pending(pending_item);
            }
        }
        return Ok(());
    }

    if matches!(
        ctx.package_states().get(&pending_src.name),
        Some(PackageResolutionState::Resolving { .. })
    ) {
        if seed_items
            && let Some(PackageResolutionState::Resolving {
                deferred_seed_requests,
            }) = ctx.package_states_mut().get_mut(&pending_src.name)
        {
            deferred_seed_requests.push(pending_src.clone());
        }
        return Ok(());
    }

    ctx.package_states_mut().insert(
        pending_src.name.clone(),
        PackageResolutionState::Resolving {
            deferred_seed_requests: Vec::new(),
        },
    );

    // Check for a version override carried from a prior restart pass.
    // When the driver restarts after a would-change detection, it seeds the fresh
    // context with the correct (new) ref for the package that triggered the restart.
    // Using it here ensures that when `resolve_package_bottom_up` is first called
    // for this package in the new pass, it immediately uses the right version without
    // having to trigger another restart. B1 and B2 are avoided because:
    //   B1: no stale manifest-derived constraints — fresh context, fresh accumulator.
    //   B2: we fall through to normal first-resolution logic below, which runs the
    //       same seed_items / filter path as any non-overridden first resolution.
    let mut rejected: Vec<(String, Vec<EngineRequirementFailure>)> = Vec::new();
    let mut override_candidate =
        ctx.version_override(&pending_src.name)
            .filter(|(resolved, _, _)| {
                resolved.version.as_ref().is_none_or(|version| {
                    !exclusions.contains_key(&(pending_src.name.clone(), version.clone()))
                })
            });
    let (resolved_ref, rooted_ref, hook_surface, manifest) = loop {
        let candidate = if let Some(value) = override_candidate.take() {
            value
        } else {
            match resolve_single_source(
                pending_src,
                provider,
                locked,
                options,
                ctx.version_constraints(),
                exclusions,
                diag,
            ) {
                Ok(ref_) => {
                    let rooted = apply_subpath(
                        &pending_src.name,
                        &ref_.tree_path,
                        pending_src.subpath.as_ref(),
                    )?;
                    let staged = stage_rooted_package(
                        &pending_src.name,
                        rooted,
                        effective_config,
                        options,
                        diag,
                    )?;
                    (ref_, staged.rooted, staged.hook_surface)
                }
                Err(MarsError::Resolution(ResolutionError::VersionConflict { .. }))
                    if !rejected.is_empty() =>
                {
                    return Err(engine_unsatisfiable_error(
                        &pending_src.name,
                        exclusions,
                        ctx.version_constraints(),
                    )
                    .expect("the just-rejected candidate is persisted")
                    .into());
                }
                Err(error) => return Err(error),
            }
        };
        let manifest = provider.read_manifest(&candidate.1.package_root, diag)?;
        let failures = manifest
            .as_ref()
            .map(|manifest| check_package_requirements(&manifest.package, options))
            .transpose()?
            .unwrap_or_default();
        if failures.is_empty() {
            break (candidate.0, candidate.1, candidate.2, manifest);
        }

        let label = candidate_version_label(&candidate.0);
        if options.version_selection_policy(&pending_src.name)
            == super::types::VersionSelectionPolicy::LockOnly
        {
            return Err(MarsError::FrozenViolation {
                message: format!(
                    "--frozen locked source `{}` version {label} is incompatible: {}",
                    pending_src.name,
                    describe_engine_failures(&failures)
                ),
            });
        }
        let Some(version) = candidate.0.version.clone() else {
            return Err(ResolutionError::RequiresEngineIncompatible {
                name: pending_src.name.to_string(),
                message: format!(
                    "version {label} requires {}; required by {}",
                    describe_engine_failures(&failures),
                    pending_src.required_by
                ),
            }
            .into());
        };
        exclusions.insert((pending_src.name.clone(), version), failures.clone());
        diag.warn(
            "requires-mars-fallback",
            format!(
                "skipping `{}` {label}: {}; required by {}",
                pending_src.name,
                describe_engine_failures(&failures),
                pending_src.required_by
            ),
        );
        rejected.push((label, failures));
    };
    if !rejected.is_empty() {
        ctx.set_version_override(
            pending_src.name.clone(),
            (
                resolved_ref.clone(),
                rooted_ref.clone(),
                hook_surface.clone(),
            ),
        );
    }
    ctx.set_hook_surface(&pending_src.name, hook_surface);
    if let Some((locked_version, failures)) = rejected.first()
        && locked
            .and_then(|lock| lock.dependencies.get(&pending_src.name))
            .and_then(|source| source.version.as_deref())
            .is_some_and(|version| version.trim_start_matches('v') == locked_version)
    {
        diag.warn(
            "requires-mars-lock-fallback",
            format!(
                "locked `{}` {locked_version} is engine-incompatible ({}); selected {} instead",
                pending_src.name,
                describe_engine_failures(failures),
                candidate_version_label(&resolved_ref)
            ),
        );
    }
    if !rejected.is_empty() {
        let mut engines = Vec::new();
        let skipped = rejected
            .iter()
            .map(|(version, failures)| {
                let requirements = failures
                    .iter()
                    .map(|failure| {
                        let engine = failure.engine.to_string();
                        if !engines.contains(&engine) {
                            engines.push(engine.clone());
                        }
                        super::EngineFallbackRequirement {
                            engine,
                            requirement: failure.requirement.clone(),
                        }
                    })
                    .collect();
                super::EngineFallbackSkippedVersion {
                    version: version.clone(),
                    requirements,
                }
            })
            .collect();
        diag.record_engine_fallback(super::EngineFallback {
            source: pending_src.name.to_string(),
            skipped,
            selected_version: candidate_version_label(&resolved_ref),
            engines,
        });
    }
    let manifest_requests =
        collect_manifest_requests(pending_src, &rooted_ref.package_root, &manifest, options)?;
    let deps = manifest_requests
        .iter()
        .map(|request| request.name.clone())
        .collect();

    let discovered = discover::discover_resolved_source(
        &rooted_ref.package_root,
        Some(pending_src.name.as_ref()),
    )?;
    let mut items: IndexMap<(ItemKind, ItemName), discover::DiscoveredItem> = IndexMap::new();
    for item in &discovered {
        items.insert((item.id.kind, item.id.name.clone()), item.clone());
    }

    ctx.registry_mut().insert(
        pending_src.name.clone(),
        RegisteredPackage {
            node: ResolvedNode {
                source_name: pending_src.name.clone(),
                source_id: pending_src.source_id.clone(),
                rooted_ref,
                resolved_ref,
                manifest,
                deps,
            },
            declared_source_id: pending_src.declared_source_id.clone(),
            items,
            constraint: pending_src.constraint.clone(),
            is_local: matches!(pending_src.spec, SourceSpec::Path(_)),
        },
    );
    ctx.id_index_mut()
        .insert(pending_src.source_id.clone(), pending_src.name.clone());

    // Version graph expansion is always required, but transitive item seeding is
    // only allowed when this package has at least one unfiltered materialization
    // request and the inbound path has remained unfiltered.
    let seed_transitive_manifest_deps =
        seed_items && package_has_unfiltered_materialization_request(ctx, &pending_src.name);
    for request in manifest_requests
        .iter()
        .filter(|request| is_unfiltered_request(&request.filter))
    {
        let seed_request_items = seed_transitive_manifest_deps;
        resolve_package_bottom_up(
            request,
            seed_request_items,
            provider,
            locked,
            options,
            effective_config,
            diag,
            ctx,
            exclusions,
        )?;
    }
    for request in manifest_requests
        .iter()
        .filter(|request| !is_unfiltered_request(&request.filter))
    {
        resolve_package_bottom_up(
            request,
            false,
            provider,
            locked,
            options,
            effective_config,
            diag,
            ctx,
            exclusions,
        )?;
    }

    let mut deferred_seed_requests = Vec::new();
    if let Some(PackageResolutionState::Resolving {
        deferred_seed_requests: deferred,
    }) = ctx.package_states_mut().remove(&pending_src.name)
    {
        deferred_seed_requests = deferred;
    }
    ctx.package_states_mut()
        .insert(pending_src.name.clone(), PackageResolutionState::Resolved);

    let pending_to_push = {
        let package = ctx
            .registry()
            .get(&pending_src.name)
            .ok_or_else(|| MarsError::Source {
                source_name: pending_src.name.to_string(),
                message: "resolved package missing from registry".to_string(),
            })?;
        let mut pending_to_push = Vec::new();
        if seed_items {
            pending_to_push.extend(seed_items_for_request(pending_src, package));
        }
        for deferred_request in deferred_seed_requests {
            pending_to_push.extend(seed_items_for_request(&deferred_request, package));
        }
        pending_to_push
    };
    for pending_item in pending_to_push {
        ctx.push_pending(pending_item);
    }

    Ok(())
}

fn candidate_version_label(resolved: &crate::source::ResolvedRef) -> String {
    resolved
        .version
        .as_ref()
        .map(ToString::to_string)
        .or_else(|| resolved.version_tag.clone())
        .unwrap_or_else(|| "HEAD/path".to_string())
}

fn describe_engine_failures(failures: &[EngineRequirementFailure]) -> String {
    failures
        .iter()
        .map(|failure| {
            format!(
                "requires-{} `{}` (running {}; use a {} version matching `{}`)",
                failure.engine,
                failure.requirement,
                failure.running,
                failure.engine,
                failure.requirement
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn engine_unsatisfiable_error(
    name: &SourceName,
    exclusions: &EngineExclusions,
    constraints: &HashMap<SourceName, Vec<(String, VersionConstraint)>>,
) -> Option<ResolutionError> {
    let mut rejected = exclusions
        .iter()
        .filter(|((source, version), _)| {
            source == name
                && constraints.get(name).is_none_or(|constraints| {
                    constraints.iter().all(|(_, constraint)| match constraint {
                        VersionConstraint::Semver(requirement) => requirement.matches(version),
                        VersionConstraint::Latest => true,
                        VersionConstraint::RefPin(_) => false,
                    })
                })
        })
        .map(|((_, version), failures)| (version, failures))
        .collect::<Vec<_>>();
    rejected.sort_by(|(left, _), (right, _)| right.cmp(left));
    if rejected.is_empty() {
        return None;
    }
    let candidates = rejected
        .into_iter()
        .map(|(version, failures)| format!("  {version}: {}", describe_engine_failures(failures)))
        .collect::<Vec<_>>()
        .join("\n");
    Some(ResolutionError::RequiresMarsUnsatisfiable {
        name: name.to_string(),
        message: format!("no compatible candidate remains:\n{candidates}"),
    })
}

fn stage_rooted_package(
    source_name: &SourceName,
    rooted: super::types::RootedSourceRef,
    effective_config: &EffectiveConfig,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Result<staging::StagedRootedSource, MarsError> {
    let Some(staging_root) = options.staging_root.as_deref() else {
        return Ok(staging::StagedRootedSource {
            rooted,
            hook_surface: staging::HookSurfaceState::Readable,
        });
    };

    let dep = effective_config.dependencies.get(source_name);
    let dialect = Dialect::resolve(dep.and_then(|entry| entry.dialect), &rooted.package_root);
    let renames = dep.map(|entry| entry.rename.clone()).unwrap_or_default();
    staging::stage_rooted_source(
        source_name,
        rooted,
        staging::RootedStageOptions { dialect },
        &effective_config.skills,
        &renames,
        staging_root,
        diag,
    )
}

fn package_has_unfiltered_materialization_request(
    ctx: &ResolverContext,
    package: &SourceName,
) -> bool {
    ctx.materialization_filters()
        .get(package)
        .is_some_and(|filters| filters.iter().any(is_unfiltered_request))
}

pub(crate) fn seed_items_for_request(
    pending_src: &PendingSource,
    package: &RegisteredPackage,
) -> Vec<PendingItem> {
    let mut selected: Vec<&discover::DiscoveredItem> = Vec::new();
    match &pending_src.filter {
        FilterMode::All => {
            selected.extend(package.items());
        }
        FilterMode::Include { agents, skills } => {
            let wanted_agents: HashSet<ItemName> = agents.iter().cloned().collect();
            let wanted_skills: HashSet<ItemName> = skills.iter().cloned().collect();
            selected.extend(package.items().filter(|item| match item.id.kind {
                ItemKind::Agent => wanted_agents.contains(&item.id.name),
                ItemKind::Skill => wanted_skills.contains(&item.id.name),
                // Package-level bootstrap docs are passive package content:
                // if any part of the package is requested, seed them with the
                // selected agents/skills so they can materialize.
                ItemKind::BootstrapDoc => true,
                // New active/config kinds are not yet selectable via Include filter.
                ItemKind::Hook | ItemKind::McpServer => false,
            }));
        }
        FilterMode::Exclude(excluded) => {
            selected.extend(package.items().filter(|item| {
                let source_path = item.source_path.to_string_lossy();
                !excluded.iter().any(|excluded_item| {
                    excluded_item == &item.id.name
                        || crate::target::paths_equivalent(excluded_item.as_ref(), &source_path)
                })
            }));
        }
        FilterMode::OnlySkills => {
            selected.extend(
                package
                    .items()
                    .filter(|item| item.id.kind == ItemKind::Skill),
            );
        }
        FilterMode::OnlyAgents => {
            selected.extend(
                package
                    .items()
                    .filter(|item| item.id.kind == ItemKind::Agent),
            );
        }
    }

    selected
        .into_iter()
        .map(|item| PendingItem {
            package: pending_src.name.clone(),
            item: item.id.name.clone(),
            kind: item.id.kind,
            constraint: pending_src.constraint.clone(),
            required_by: pending_src.required_by.clone(),
            is_local: package.is_local,
        })
        .collect()
}

pub(crate) fn collect_manifest_requests(
    pending_src: &PendingSource,
    package_root: &Path,
    manifest: &Option<Manifest>,
    options: &ResolveOptions,
) -> Result<Vec<PendingSource>, MarsError> {
    let mut requests = Vec::new();
    let Some(manifest_data) = manifest else {
        return Ok(requests);
    };
    for (dep_name, dep_spec) in &manifest_data.dependencies {
        let dep_name_typed = SourceName::from(dep_name.clone());
        let dep_subpath = dep_spec.subpath.clone();
        let dep_filter = dep_spec.filter.to_mode();

        let (declared_spec, declared_constraint) = match (&dep_spec.url, &dep_spec.path) {
            (Some(url), None) => (
                SourceSpec::Git(GitSpec {
                    url: url.clone(),
                    version: dep_spec.version.clone(),
                }),
                parse_version_constraint(dep_spec.version.as_deref()),
            ),
            (None, Some(path)) => {
                let resolved_path = if path.is_absolute() {
                    path.clone()
                } else {
                    package_root.join(path)
                };
                (SourceSpec::Path(resolved_path), VersionConstraint::Latest)
            }
            (Some(_), Some(_)) => {
                return Err(ConfigError::Invalid {
                    message: format!("source `{dep_name}` has both `url` and `path` — pick one"),
                }
                .into());
            }
            (None, None) => {
                return Err(ConfigError::Invalid {
                    message: format!(
                        "source `{dep_name}` has neither `url` nor `path` — one is required"
                    ),
                }
                .into());
            }
        };
        let declared_source_id =
            source_id_for_pending_spec(package_root, &declared_spec, dep_subpath.clone());
        let (dep_spec_resolved, dep_constraint) =
            if let Some(path) = options.source_overrides.get(&dep_name_typed) {
                (SourceSpec::Path(path.clone()), VersionConstraint::Latest)
            } else {
                (declared_spec, declared_constraint)
            };
        let effective_source_id =
            source_id_for_pending_spec(package_root, &dep_spec_resolved, dep_subpath.clone());
        requests.push(PendingSource {
            name: dep_name_typed,
            declared_source_id,
            source_id: effective_source_id,
            spec: dep_spec_resolved,
            subpath: dep_subpath,
            constraint: dep_constraint,
            filter: dep_filter,
            required_by: pending_src.name.to_string(),
        });
    }

    Ok(requests)
}
