use mars_schema::item::ItemKind;
use mars_schema::version::{Constraint, parse_constraint};
use std::collections::HashMap;
use std::path::Path;

use crate::config::{EffectiveConfig, GitSpec, Manifest, SourceSpec, source_id_for_spec};
use crate::diagnostic::DiagnosticCollector;
use crate::discover;
use crate::error::{MarsError, ResolutionError};
use crate::lock::LockFile;
use crate::types::{SourceId, SourceName, SourceUrl};
use indexmap::IndexMap;
use mars_schema::dependency::Source;
use mars_schema::filter::FilterMode;
use mars_schema::names::{ItemName, SourceSubpath};

use super::EngineExclusions;
use super::SourceProvider;
use super::context::ResolverContext;
use super::requires::{EngineRequirementFailure, check_package_requirements};
use super::types::{Edge, ResolveOptions, ResolvedNode};
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
    pub(crate) constraint: Constraint,
    pub(crate) filter: FilterMode,
    pub(crate) required_by: String,
}

#[derive(Debug, Default)]
pub(crate) enum PackageResolutionState {
    #[default]
    Resolved,
    Resolving,
}

#[derive(Debug, Clone)]
pub(crate) struct RegisteredPackage {
    pub(crate) node: ResolvedNode,
    pub(crate) declared_source_id: SourceId,
    pub(crate) items: IndexMap<(ItemKind, ItemName), discover::DiscoveredItem>,
    pub(crate) is_local: bool,
}

impl RegisteredPackage {
    pub(crate) fn items(&self) -> impl Iterator<Item = &discover::DiscoveredItem> {
        self.items.values()
    }
}

fn prepare_package(
    source_name: &SourceName,
    rooted: crate::resolve::RootedSourceRef,
    effective_config: &EffectiveConfig,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Result<crate::package_prepare::PreparedRootedSource, MarsError> {
    if let Some(prepared) = options.prepared_sources.get(source_name) {
        crate::package_prepare::prepared_rooted(rooted, std::sync::Arc::clone(prepared), diag)
    } else {
        crate::package_prepare::prepare_rooted(source_name, rooted, effective_config, diag)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_package_bottom_up(
    pending_src: &PendingSource,
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
        // (the normal first-resolution path).
        //
        // Fast paths that skip the check:
        //   Path sources — no version selection, never change.
        //   RefPin constraints — fixed refs, semver re-selection doesn't apply.
        //   Semver(req) where existing version already satisfies req — adding that
        //     requirement cannot invalidate the currently selected version.
        let needs_check = matches!(pending_src.spec, SourceSpec::Git(_))
            && !matches!(pending_src.constraint, Constraint::RefPin(_));

        if needs_check {
            let existing_ref = ctx
                .registry()
                .get(&pending_src.name)
                .map(|p| p.node.resolved_ref.clone());

            // Fast path: semver constraint already satisfied → selection cannot change.
            let skip = match (&pending_src.constraint, &existing_ref) {
                (Constraint::Semver(req), Some(ref_)) => {
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
                    let new_rooted = super::plugin::root_package(
                        pending_src,
                        &new_ref,
                        provider,
                        locked,
                        options,
                        diag,
                    )?;
                    let staged = prepare_package(
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

        return Ok(());
    }

    if matches!(
        ctx.package_states().get(&pending_src.name),
        Some(PackageResolutionState::Resolving)
    ) {
        return Ok(());
    }

    ctx.package_states_mut()
        .insert(pending_src.name.clone(), PackageResolutionState::Resolving);

    // Check for a version override carried from a prior restart pass.
    // When the driver restarts after a would-change detection, it seeds the fresh
    // context with the correct (new) ref for the package that triggered the restart.
    // Using it here ensures that when `resolve_package_bottom_up` is first called
    // for this package in the new pass, it immediately uses the right version without
    // having to trigger another restart. B1 and B2 are avoided because:
    //   B1: no stale manifest-derived constraints — fresh context, fresh accumulator.
    //   B2: we fall through to normal first-resolution logic below, which runs the
    //       same path as any non-overridden first resolution.
    let mut rejected: Vec<(String, Vec<EngineRequirementFailure>)> = Vec::new();
    let mut override_candidate =
        ctx.version_override(&pending_src.name)
            .filter(|(resolved, _, _)| {
                resolved.version.as_ref().is_none_or(|version| {
                    !exclusions.contains_key(&(pending_src.name.clone(), version.clone()))
                })
            });
    let (resolved_ref, rooted_ref, hook_surface, prepared, manifest) = loop {
        let candidate = if let Some((resolved, rooted, _hook_surface)) = override_candidate.take() {
            let prepared =
                prepare_package(&pending_src.name, rooted, effective_config, options, diag)?;
            let manifest = test_manifest_fallback(
                provider,
                &prepared.rooted.authored_root,
                prepared.manifest,
                diag,
            )?;
            (
                resolved,
                prepared.rooted,
                prepared.hook_surface,
                prepared.prepared,
                manifest,
            )
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
                    let rooted = super::plugin::root_package(
                        pending_src,
                        &ref_,
                        provider,
                        locked,
                        options,
                        diag,
                    )?;
                    let staged = prepare_package(
                        &pending_src.name,
                        rooted,
                        effective_config,
                        options,
                        diag,
                    )?;
                    let manifest = test_manifest_fallback(
                        provider,
                        &staged.rooted.authored_root,
                        staged.manifest,
                        diag,
                    )?;
                    (
                        ref_,
                        staged.rooted,
                        staged.hook_surface,
                        staged.prepared,
                        manifest,
                    )
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
        let failures = candidate
            .4
            .as_ref()
            .map(|manifest| check_package_requirements(&manifest.package, options))
            .transpose()?
            .unwrap_or_default();
        if failures.is_empty() {
            break candidate;
        }

        let label = candidate_version_label(&candidate.0);
        if options.version_selection_policy(&pending_src.name)
            == mars_schema::version::SelectionPolicy::LockOnly
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
        collect_manifest_requests(pending_src, &rooted_ref.authored_root, &manifest, options)?;
    let edges = manifest_requests
        .iter()
        .map(|request| Edge {
            name: request.name.clone(),
            filter: request.filter.clone(),
        })
        .collect();

    let discovered = discover::package_items(
        &rooted_ref.authored_root,
        Some(pending_src.name.as_ref()),
        &prepared,
        None,
        diag,
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
                prepared,
                resolved_ref,
                manifest,
                edges,
            },
            declared_source_id: pending_src.declared_source_id.clone(),
            items,
            is_local: matches!(pending_src.spec, SourceSpec::Path(_)),
        },
    );
    ctx.id_index_mut()
        .insert(pending_src.source_id.clone(), pending_src.name.clone());

    for request in &manifest_requests {
        resolve_package_bottom_up(
            request,
            provider,
            locked,
            options,
            effective_config,
            diag,
            ctx,
            exclusions,
        )?;
    }

    ctx.package_states_mut()
        .insert(pending_src.name.clone(), PackageResolutionState::Resolved);

    Ok(())
}

#[cfg(test)]
fn test_manifest_fallback(
    provider: &dyn SourceProvider,
    authored_root: &Path,
    manifest: Option<Manifest>,
    diag: &mut DiagnosticCollector,
) -> Result<Option<Manifest>, MarsError> {
    Ok(provider.read_manifest(authored_root, diag)?.or(manifest))
}

#[cfg(not(test))]
fn test_manifest_fallback(
    _provider: &dyn SourceProvider,
    _authored_root: &Path,
    manifest: Option<Manifest>,
    _diag: &mut DiagnosticCollector,
) -> Result<Option<Manifest>, MarsError> {
    Ok(manifest)
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
    constraints: &HashMap<SourceName, Vec<(String, Constraint)>>,
) -> Option<ResolutionError> {
    let mut rejected = exclusions
        .iter()
        .filter(|((source, version), _)| {
            source == name
                && constraints.get(name).is_none_or(|constraints| {
                    constraints
                        .iter()
                        .all(|(_, constraint)| mars_schema::version::satisfies(constraint, version))
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

        let (declared_spec, declared_constraint) = match &dep_spec.source {
            Source::Git { url } => (
                SourceSpec::Git(GitSpec {
                    url: SourceUrl::from(url.as_str()),
                    version: dep_spec.version.clone(),
                }),
                parse_constraint(dep_spec.version.as_deref()),
            ),
            Source::Path { path } => {
                let resolved_path = if path.is_absolute() {
                    path.clone()
                } else {
                    package_root.join(path)
                };
                (SourceSpec::Path(resolved_path), Constraint::Latest)
            }
        };
        let declared_source_id = source_id_for_spec(
            package_root,
            &declared_spec,
            dep_subpath.clone(),
            dep_spec.plugin.as_deref(),
        );
        let (dep_spec_resolved, dep_constraint) =
            if let Some(path) = options.source_overrides.get(&dep_name_typed) {
                (SourceSpec::Path(path.clone()), Constraint::Latest)
            } else {
                (declared_spec, declared_constraint)
            };
        let effective_source_id = source_id_for_spec(
            package_root,
            &dep_spec_resolved,
            dep_subpath.clone(),
            dep_spec.plugin.as_deref(),
        );
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
