//! `mars link <target>` — add a managed target directory.
//!
//! `mars link <target>` adds the target to `settings.targets` and copies
//! content from `.mars/` into that target. With `--local` it adds the target to
//! `mars.local.toml` instead, so only this machine syncs it.
//! Use `mars unlink <target>` to remove a target.

use crate::diagnostic::{Diagnostic, DiagnosticCollector};
use crate::error::MarsError;
use crate::lock::{ItemId, LockFile};
use crate::surface_ownership::CollisionAdoptHint;
use crate::sync::apply::{ActionOutcome, ActionTaken};
use crate::types::managed_cmd;
use mars_schema::item::ItemKind;
use mars_schema::names::ItemName;

use super::output;

/// Arguments for `mars link`.
#[derive(Debug, clap::Args)]
pub struct LinkArgs {
    /// Target directory to materialize (e.g. `.claude`).
    pub target: String,
    /// Adopt collisions without an installed-content claim (overwrite + record in lock).
    #[arg(long)]
    pub force: bool,
    /// Add the target to `mars.local.toml`: this machine syncs it, and `mars.toml` and the
    /// committed `mars.lock` do not change.
    #[arg(long)]
    pub local: bool,
}

/// Run `mars link`.
pub fn run(args: &LinkArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let parsed_target = super::target::normalize_target_name(&args.target)?;
    if let Some(retired) = mars_schema::retired::retired_target(&parsed_target) {
        return Err(MarsError::Link {
            target: retired.dir.to_string(),
            message: retired.message(&crate::types::managed_cmd(&format!(
                "mars unlink {}",
                retired.dir
            ))),
        });
    }
    let target_name = crate::config::migrations::link::normalize_link(&parsed_target).target;
    link_target(ctx, &target_name, args.force, args.local, json)
}

fn link_target(
    ctx: &super::MarsContext,
    target_name: &str,
    force: bool,
    local_only: bool,
    json: bool,
) -> Result<i32, MarsError> {
    let config_path = ctx.project_root.join("mars.toml");
    if !config_path.exists() {
        return Err(MarsError::Link {
            target: target_name.to_string(),
            message: format!(
                "mars.toml not found at {} — run `{cmd}` first",
                ctx.project_root.display(),
                cmd = managed_cmd("mars init"),
            ),
        });
    }

    if !json && !super::is_recognized_target(target_name) {
        output::warn(
            "target-unrecognized",
            format!("`{target_name}` is not a recognized tool directory — managing anyway"),
        );
    }

    let mars_dir = ctx.project_root.join(".mars");
    std::fs::create_dir_all(&mars_dir)?;
    let lock_path = mars_dir.join("sync.lock");
    let _sync_lock = crate::fs::FileLock::acquire(&lock_path)?;

    let config = crate::config::load(&ctx.project_root)?;
    let local = crate::config::load_local(&ctx.project_root)?;
    let user = crate::config::load_user_config()?;
    let (effective, _) =
        crate::config::merge_with_root(config.clone(), local.clone(), &user, &ctx.project_root)?;
    let mut runtime_targets = crate::config::targets::managed_targets(&effective.settings);
    ensure_target_list_contains(&mut runtime_targets, target_name);

    // The project file is the target's home unless `--local` asked for this machine only; a
    // target `mars.toml` already names stays there, and a copy in `mars.local.toml` goes.
    let in_project = crate::config::targets::managed_targets(&config.settings)
        .iter()
        .any(|target| target == target_name);
    let mut config = config;
    let mut local = local;
    let (settings_changed, local_changed) = if local_only {
        let add = !in_project && !named_by(&local.settings.targets, target_name);
        let floor = adding_targets_floor();
        let add_floor = add
            && !mars_schema::package::requirement_has_floor(
                effective.settings.min_mars_version.as_deref(),
                &floor,
            );
        if add {
            local
                .settings
                .targets
                .get_or_insert_with(Vec::new)
                .push(target_name.to_string());
        }
        if add_floor {
            local.settings.min_mars_version = Some(floor.to_string());
        }
        (false, add)
    } else {
        let local_changed = super::unlink::unset_target(&mut local.settings.targets, target_name);
        if !in_project {
            config
                .settings
                .targets
                .get_or_insert_with(Vec::new)
                .push(target_name.to_string());
        }
        (!in_project, local_changed)
    };

    let lock = crate::sync::recovery::load_lock(&ctx.project_root)?;
    let outcomes = lock_items_as_sync_outcomes(&lock);
    let mut diag = DiagnosticCollector::new();
    let provided = crate::provided::ProvidedNames::load(&ctx.project_root, &mut diag)?;
    let agent_copy_spec = crate::compiler::agent_copy::build_agent_copy_spec(
        effective.settings.meridian_agent_copy(),
        &runtime_targets,
        &mut diag,
    );
    let agent_surface_policy = crate::compiler::agent_surface_policy(
        effective.settings.agent_emission.as_ref(),
        agent_copy_spec.as_ref(),
        ctx.meridian_managed,
    );

    let cache = crate::source::GlobalCache::new()?;
    let source_provider = crate::resolve::RealSourceProvider::new(&cache, &ctx.project_root);
    let resolve_options = crate::resolve::ResolveOptions::sync();
    let graph = crate::resolve::resolve(
        &effective,
        &source_provider,
        Some(&lock),
        &resolve_options,
        &mut diag,
    )?;

    let (target_outcomes, provided_skills) = crate::compiler::sync_targets_under_policy(
        &ctx.project_root,
        &[target_name.to_string()],
        &outcomes,
        &provided,
        &agent_surface_policy,
        &lock,
        force,
        CollisionAdoptHint::LinkForce,
        &mut diag,
    );
    let mut diagnostics = diag.drain();
    diagnostics.extend(crate::config::targets::target_advisories(
        &runtime_targets,
        &crate::config::targets::effective_links(&effective.settings).retired_targets(),
    ));

    reject_unmanaged_collision(&diagnostics, target_name, force)?;

    let Some(outcome) = target_outcomes.first() else {
        return Err(MarsError::Link {
            target: target_name.to_string(),
            message: "target sync produced no result".to_string(),
        });
    };

    if !outcome.errors.is_empty() {
        return Err(MarsError::Link {
            target: target_name.to_string(),
            message: outcome.errors.join("; "),
        });
    }

    let (compiled_native_outputs, removed_native_outputs) =
        crate::compiler::materialize_native_agents_after_link(
            &crate::compiler::NativeAgentLinkMaterializeCtx {
                mars_ctx: ctx,
                managed_targets: &runtime_targets,
                effective: &effective,
                graph: &graph,
                old_lock: &lock,
                target_outcomes: &target_outcomes,
                force,
            },
            &mut diag,
        );
    diagnostics.extend(diag.drain());

    reject_unmanaged_collision(&diagnostics, target_name, force)?;

    let mut new_lock = lock.clone();
    crate::lock::record_provided_skills(&mut new_lock, &lock, &provided_skills, &target_outcomes);
    crate::lock::apply_target_sync_outputs(&mut new_lock, &target_outcomes);
    crate::lock::apply_removed_native_outputs(&mut new_lock, &removed_native_outputs);
    crate::lock::apply_compiled_native_outputs(&mut new_lock, &compiled_native_outputs)?;
    let project_targets = crate::config::targets::project_targets(&config);
    if let Some(warning) =
        crate::sync::recovery::publish(&ctx.project_root, &new_lock, &project_targets)?
    {
        diag.warn("native-agent-manifest-write", warning);
        diagnostics.extend(diag.drain());
    }

    if settings_changed {
        // The key is retired; leaving it beside the `targets` just written would only warn.
        config.settings.managed_root = None;
        crate::config::save(&ctx.project_root, &config)?;
    }
    if local_changed {
        crate::config::save_local(&ctx.project_root, &local)?;
    }

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "target": target_name,
            "settings_updated": settings_changed || local_changed,
            "local": local_only,
            "synced": outcome.items_synced,
            "removed": outcome.items_removed,
            "diagnostics": diagnostics,
        }));
    } else {
        output::print_success(&format!(
            "managed target `{target_name}` (synced {}, removed {})",
            outcome.items_synced, outcome.items_removed
        ));
        if local_only && in_project {
            output::print_info(&format!(
                "mars.toml already links `{target_name}`: it stays a project target, shared with everyone"
            ));
        }
        output::print_diagnostics(&diagnostics, false);
    }

    Ok(0)
}

/// Checked after the file copy and again after native agents are written: each step
/// reports its own collisions.
fn reject_unmanaged_collision(
    diagnostics: &[Diagnostic],
    target_name: &str,
    force: bool,
) -> Result<(), MarsError> {
    if !force
        && diagnostics
            .iter()
            .any(|d| d.code == "target-unmanaged-collision")
    {
        return Err(MarsError::Link {
            target: target_name.to_string(),
            message: format!(
                "unmanaged collision in `{target_name}` — hand-written files would be skipped; \
                 run `{}` to adopt",
                managed_cmd(&format!("mars link {target_name} --force")),
            ),
        });
    }
    Ok(())
}

/// The first Mars where a local `targets` list adds to the project's. Mars 0.38.0 reads it as
/// the whole list and, under its advised `--force`, rewrites the shared lock to match. A build
/// before it is a development build of it, which must not stop itself.
fn adding_targets_floor() -> semver::Version {
    let running =
        semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver");
    running.min(semver::Version::new(0, 39, 0))
}

fn named_by(targets: &Option<Vec<String>>, target_name: &str) -> bool {
    targets
        .iter()
        .flatten()
        .any(|raw| crate::config::migrations::link::normalize_link(raw).target == target_name)
}

fn ensure_target_list_contains(targets: &mut Vec<String>, target_name: &str) {
    if !targets.iter().any(|target| target == target_name) {
        targets.push(target_name.to_string());
    }
}

fn lock_items_as_sync_outcomes(lock: &LockFile) -> Vec<ActionOutcome> {
    lock.canonical_flat_items()
        .into_iter()
        .map(|(dest_path, item)| ActionOutcome {
            item_id: ItemId {
                kind: item.kind,
                name: item_name_from_dest_path(&dest_path, item.kind),
            },
            action: ActionTaken::Skipped,
            dest_path,
            source_name: item.source,
            source_checksum: None,
            installed_checksum: Some(item.installed_checksum),
        })
        .collect()
}

fn item_name_from_dest_path(dest_path: &crate::types::DestPath, kind: ItemKind) -> ItemName {
    let last = dest_path.as_str().rsplit('/').next().unwrap_or("");
    let name = match kind {
        ItemKind::Agent => last.strip_suffix(".md").unwrap_or(last).to_string(),
        ItemKind::Skill | ItemKind::Hook | ItemKind::McpServer | ItemKind::BootstrapDoc => {
            last.to_string()
        }
    };

    ItemName::from(name)
}
