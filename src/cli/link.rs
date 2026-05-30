//! `mars link <target>` — add a managed target directory.
//!
//! `mars link <target>` adds the target to `settings.targets` and copies
//! content from `.mars/` into that target.
//! Use `mars unlink <target>` to remove a target.

use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCollector, DiagnosticLevel};
use crate::error::MarsError;
use crate::lock::{ItemId, ItemKind, LockFile};
use crate::surface_ownership::CollisionAdoptHint;
use crate::sync::apply::{ActionOutcome, ActionTaken};
use crate::types::ItemName;
use crate::types::managed_cmd;

use super::output;

/// Arguments for `mars link`.
#[derive(Debug, clap::Args)]
pub struct LinkArgs {
    /// Target directory to materialize (e.g. `.claude`).
    pub target: String,
    /// Adopt untracked collisions in the linked target (overwrite + record in lock).
    #[arg(long)]
    pub force: bool,
}

/// Run `mars link`.
pub fn run(args: &LinkArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let parsed_target = super::target::normalize_target_name(&args.target)?;
    let target_name = crate::config::migrations::link::normalize_link(&parsed_target).target;
    link_target(ctx, &target_name, args.force, json)
}

fn link_target(
    ctx: &super::MarsContext,
    target_name: &str,
    force: bool,
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

    if !json
        && !super::WELL_KNOWN.contains(&target_name)
        && !super::TOOL_DIRS.contains(&target_name)
    {
        output::print_warn(&format!(
            "`{target_name}` is not a recognized tool directory — managing anyway"
        ));
    }

    let mars_dir = ctx.project_root.join(".mars");
    std::fs::create_dir_all(&mars_dir)?;
    let lock_path = mars_dir.join("sync.lock");
    let _sync_lock = crate::fs::FileLock::acquire(&lock_path)?;

    let config = crate::config::load(&ctx.project_root)?;
    let local = crate::config::load_local(&ctx.project_root)?;
    let (effective, _) =
        crate::config::merge_with_root(config.clone(), local.clone(), &ctx.project_root)?;
    let mut targets = effective.settings.managed_targets();
    if !targets.iter().any(|target| target == target_name) {
        targets.push(target_name.to_string());
    }

    let settings_changed = config.settings.targets.as_ref() != Some(&targets);

    let lock = crate::lock::load(&ctx.project_root)?;
    let outcomes = lock_items_as_sync_outcomes(&lock);
    let mut diag = DiagnosticCollector::new();
    let agent_copy_spec = crate::compiler::agent_copy::build_agent_copy_spec(
        effective.settings.agent_copy.as_ref(),
        &targets,
        &mut diag,
    );
    let agent_surface_policy = crate::compiler::agent_surface_policy(
        effective.settings.agent_emission.as_ref(),
        agent_copy_spec.as_ref(),
        ctx.meridian_managed,
    );
    let suppressed_outcomes;
    let sync_outcomes = if matches!(
        agent_surface_policy,
        crate::compiler::AgentSurfacePolicy::SuppressAll
            | crate::compiler::AgentSurfacePolicy::EmitSelective(_)
    ) {
        suppressed_outcomes = crate::compiler::suppress_agent_outcomes(&outcomes);
        &suppressed_outcomes
    } else {
        &outcomes
    };
    let target_sync_ctx = crate::target_sync::TargetSyncContext {
        old_lock: &lock,
        force,
        collision_hint: CollisionAdoptHint::LinkForce,
    };
    let target_outcomes = crate::target_sync::sync_managed_targets(
        &ctx.project_root,
        &mars_dir,
        &[target_name.to_string()],
        sync_outcomes,
        &target_sync_ctx,
        &mut diag,
    );
    let mut diagnostics = diag.drain();
    if let Some(diagnostic) = deprecated_agents_target_diagnostic(target_name) {
        diagnostics.push(diagnostic);
    }

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

    let cache = crate::source::GlobalCache::new()?;
    let source_provider = crate::sync::provider::RealSourceProvider {
        cache: &cache,
        project_root: &ctx.project_root,
    };
    let resolve_options = crate::resolve::ResolveOptions::sync();
    let graph = crate::resolve::resolve(
        &effective,
        &source_provider,
        Some(&lock),
        &resolve_options,
        &mut diag,
    )?;
    let (compiled_native_outputs, removed_native_outputs) =
        crate::compiler::materialize_native_agents_after_link(
            &crate::compiler::NativeAgentLinkMaterializeCtx {
                mars_ctx: ctx,
                managed_targets: &targets,
                config: &config,
                local: &local,
                effective: &effective,
                graph: &graph,
                old_lock: &lock,
                force,
            },
            &mut diag,
        );

    let mut new_lock = lock;
    crate::lock::apply_target_sync_outputs(&mut new_lock, &target_outcomes);
    crate::lock::apply_removed_native_outputs(&mut new_lock, &removed_native_outputs);
    crate::lock::apply_compiled_native_outputs(&mut new_lock, &compiled_native_outputs);
    crate::lock::write(&ctx.project_root, &new_lock)?;

    if settings_changed {
        let mut config = config;
        config.settings.targets = Some(targets);
        crate::config::save(&ctx.project_root, &config)?;
    }

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "target": target_name,
            "settings_updated": settings_changed,
            "synced": outcome.items_synced,
            "removed": outcome.items_removed,
            "diagnostics": diagnostics,
        }));
    } else {
        output::print_success(&format!(
            "managed target `{target_name}` (synced {}, removed {})",
            outcome.items_synced, outcome.items_removed
        ));
        for diagnostic in diagnostics {
            output::print_warn(&diagnostic.to_string());
        }
    }

    Ok(0)
}

fn deprecated_agents_target_diagnostic(target_name: &str) -> Option<Diagnostic> {
    (target_name == ".agents").then(|| Diagnostic {
        level: DiagnosticLevel::Warning,
        code: "deprecated-agents-target",
        message: format!(
            "`.agents` is a deprecated link target. Run `{}` to remove it. Skills are now emitted to native harness dirs automatically.",
            managed_cmd("mars unlink .agents"),
        ),
        context: Some("link target".to_string()),
        category: Some(DiagnosticCategory::Compatibility),
    })
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
