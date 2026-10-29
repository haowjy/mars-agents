//! Recovery halt: stopping before the compiler when a hook surface is unreadable, and the
//! persisted-intent bookkeeping that halt shares with the apply phase.

use std::collections::BTreeMap;

use crate::error::MarsError;
use crate::sync::mutation::{ConfigMutation, DependencyUpsertChange};
use crate::sync::{LoadedConfig, ResolutionMode, ResolvedState, SyncRequest};
use crate::types::{MarsContext, SourceName, managed_cmd};

/// A source package preventing a recovery command from entering the compiler.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecoveryBlocker {
    pub package: String,
    pub version: String,
    pub hook_names: Vec<String>,
    pub guidance: String,
    pub suggested_command: String,
}

/// Successful intent persistence that still requires recovery work.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecoveryHalt {
    pub persisted: Vec<String>,
    pub blockers: Vec<RecoveryBlocker>,
    pub next_step: String,
}

pub(super) fn build_recovery_halt(
    resolved: &ResolvedState,
    unreadable_hook_surfaces: &BTreeMap<SourceName, std::collections::BTreeSet<String>>,
    request: &SyncRequest,
) -> RecoveryHalt {
    let persisted = persisted_intent_descriptions(resolved, request);
    let blockers = unreadable_hook_surfaces
        .iter()
        .map(|(source_name, hook_names)| {
            let node = resolved
                .graph
                .nodes
                .get(source_name)
                .expect("unreadable surface belongs to a resolved node");
            let version = node
                .resolved_ref
                .version
                .as_ref()
                .map(ToString::to_string)
                .or_else(|| node.resolved_ref.version_tag.clone())
                .or_else(|| {
                    node.manifest
                        .as_ref()
                        .map(|manifest| manifest.package.version.clone())
                })
                .or_else(|| node.resolved_ref.commit.as_ref().map(ToString::to_string))
                .unwrap_or_else(|| "path".to_string());
            let parents: Vec<_> = resolved
                .graph
                .nodes
                .values()
                .filter(|candidate| candidate.deps().any(|dep| &dep == source_name))
                .map(|candidate| candidate.source_name.to_string())
                .collect();
            let upgrade_cmd =
                managed_cmd(&format!("mars upgrade {source_name}")).into_owned();
            let (guidance, suggested_command) = match &request.mutation {
                Some(ConfigMutation::RemoveDependency { name })
                    if name == source_name && !parents.is_empty() =>
                {
                    (
                        format!(
                            "removed direct dependency `{name}`, but `{source_name}` is still required by {} and remains legacy; override it",
                            parents.join(", ")
                        ),
                        managed_cmd(&format!("mars override {source_name} --path <path>")).into_owned(),
                    )
                }
                None if matches!(request.resolution, ResolutionMode::Maximize { .. }) => {
                    let bump = matches!(
                        request.resolution,
                        ResolutionMode::Maximize { bump: true, .. }
                    );
                    if bump {
                        (
                            format!(
                                "newest available `{source_name}@{version}` still uses the removed hook schema; override or remove it"
                            ),
                            format!(
                                "{cmd_override} --path <path> or {cmd_remove}",
                                cmd_override = managed_cmd(&format!("mars override {source_name}")),
                                cmd_remove = managed_cmd(&format!("mars remove {source_name}")),
                            ),
                        )
                    } else {
                        (
                            format!(
                                "newest compatible `{source_name}@{version}` still uses the removed hook schema; try --bump to escape the version constraint, or override/remove"
                            ),
                            managed_cmd(&format!("mars upgrade {source_name} --bump")).into_owned(),
                        )
                    }
                }
                None if request.options.force => (
                    format!(
                        "cannot repair while `{source_name}@{version}` uses the removed hook schema; upgrade, override, or remove it"
                    ),
                    upgrade_cmd.clone(),
                ),
                _ => (
                    format!(
                        "`{source_name}@{version}` still uses the removed hook schema; upgrade, override, or remove it"
                    ),
                    upgrade_cmd,
                ),
            };
            RecoveryBlocker {
                package: source_name.to_string(),
                version,
                hook_names: hook_names.iter().cloned().collect(),
                guidance,
                suggested_command,
            }
        })
        .collect();
    RecoveryHalt {
        persisted,
        blockers,
        next_step: format!("then run `{}`", managed_cmd("mars sync")),
    }
}

fn has_bump_version_changes(loaded: &LoadedConfig, request: &SyncRequest) -> bool {
    has_version_changes(&loaded.dependency_changes)
        && matches!(
            request.resolution,
            ResolutionMode::Maximize { bump: true, .. }
        )
}

/// Persist only the user's pending intent mutation. This is shared by the
/// normal apply phase and the recovery halt at the reader/compiler boundary.
pub(super) fn persist_pending_config_mutation(
    ctx: &MarsContext,
    loaded: &LoadedConfig,
    request: &SyncRequest,
) -> Result<(), MarsError> {
    if request.options.dry_run {
        return Ok(());
    }
    let bump_changed = has_bump_version_changes(loaded, request);
    match &request.mutation {
        Some(ConfigMutation::SetOverride { .. }) => {
            crate::config::save_local(&ctx.project_root, &loaded.local)?;
        }
        Some(
            ConfigMutation::UpsertDependency { .. }
            | ConfigMutation::BatchUpsert(..)
            | ConfigMutation::RemoveDependency { .. }
            | ConfigMutation::SetRename { .. },
        ) => {
            crate::config::save(&ctx.project_root, &loaded.config)?;
        }
        None if bump_changed => {
            crate::config::save(&ctx.project_root, &loaded.config)?;
        }
        None => {}
    }
    Ok(())
}

fn persisted_intent_descriptions(resolved: &ResolvedState, request: &SyncRequest) -> Vec<String> {
    let dry_prefix = if request.options.dry_run {
        "would persist"
    } else {
        "persisted"
    };
    match &request.mutation {
        Some(ConfigMutation::SetOverride {
            source_name,
            local_path,
        }) => vec![format!(
            "{dry_prefix} override for `{source_name}` to `{}` in mars.local.toml",
            local_path.display()
        )],
        Some(ConfigMutation::RemoveDependency { name }) => {
            vec![format!(
                "{dry_prefix} removal of direct dependency `{name}` in mars.toml"
            )]
        }
        Some(_) => vec![format!("{dry_prefix} config mutation")],
        None if has_bump_version_changes(&resolved.loaded, request) => resolved
            .loaded
            .dependency_changes
            .iter()
            .filter(|change| change.old_version != change.new_version)
            .map(|change| {
                format!(
                    "{dry_prefix} bumped constraint for `{}` to `{}` in mars.toml",
                    change.name,
                    change.new_version.as_deref().unwrap_or("latest")
                )
            })
            .collect(),
        None => vec!["nothing persisted".to_string()],
    }
}

fn has_version_changes(changes: &[DependencyUpsertChange]) -> bool {
    changes
        .iter()
        .any(|change| change.old_version != change.new_version)
}
