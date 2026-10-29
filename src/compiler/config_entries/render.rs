//! Hook preflight: read, validate and render every selected hook once, before any mutation.
//! Compile reuses the result.

use std::collections::{BTreeSet, HashMap};

use super::installed_token;
use crate::compiler::hooks::{HookRank, ParsedHookItem, error_message};
use crate::diagnostic::DiagnosticCollector;
use crate::sync::package_dir::PackageDir;
use crate::target::{HOOK_KEY_PREFIX, HookEntry, MCP_KEY_PREFIX, TargetAdapter};
use crate::types::MarsContext;
use mars_schema::hooks::render::Skip;
use mars_schema::hooks::{HookOutput, MergeOutput, Origin};

/// Every selected hook's output, by the hook's canonical directory, as preflight rendered it.
pub(crate) struct RenderedHooks(HashMap<String, HookOutput>);

impl RenderedHooks {
    /// The output of a hook preflight selected. Compile only asks for selected hooks.
    pub(crate) fn get(&self, dir: &PackageDir) -> &HookOutput {
        self.0
            .get(dir.canonical().as_str())
            .expect("preflight rendered every selected hook")
    }
}

/// Validate and render every selected hook, and check the destinations config entries will
/// touch, before the apply phase.
///
/// This is deliberately separate from config emission so an invalid hook
/// cannot leave canonical or target state partially mutated.
pub(crate) fn preflight_config_entries(
    ctx: &MarsContext,
    targeted: &crate::sync::TargetedState,
    diag: &mut DiagnosticCollector,
) -> Result<RenderedHooks, crate::error::MarsError> {
    use crate::compiler::hooks::load_output;
    use crate::error::{ConfigError, MarsError};
    use crate::target::TargetRegistry;
    use mars_schema::hooks::Tokens;

    let resolved = &targeted.resolved;
    let registry = TargetRegistry::new();
    let mut errors = Vec::new();
    let mut rendered = HashMap::new();
    let mut reported: HashMap<(String, String), Vec<Skip>> = HashMap::new();
    // Targets whose MCP file / hook files this sync reads: those the old lock
    // has records on, and those with something selected.
    let mut mcp_targets: BTreeSet<String> = BTreeSet::new();
    let mut hook_targets: BTreeSet<String> = BTreeSet::new();
    for (target, records) in &resolved.loaded.old_lock.config_entries {
        if records.keys().any(|key| key.starts_with(MCP_KEY_PREFIX)) {
            mcp_targets.insert(target.clone());
        }
        if records.keys().any(|key| key.starts_with(HOOK_KEY_PREFIX)) {
            hook_targets.insert(target.clone());
        }
    }
    hook_targets.extend(
        targeted
            .target
            .selected_hooks()
            .map(|(dir, _)| dir.target().to_string()),
    );
    mcp_targets.extend(targeted.target.selected_mcp_targets().map(str::to_string));
    for target_name in mcp_targets.union(&hook_targets) {
        let touches_mcp = mcp_targets.contains(target_name);
        let touches_hooks = hook_targets.contains(target_name);
        let target_dir = ctx.project_root.join(target_name);
        if let Err(error) = validate_destination_path(&ctx.project_root, &target_dir, true) {
            errors.push(error);
            continue;
        }
        if let Some(adapter) = registry.get(target_name) {
            if touches_mcp && let Some(file) = adapter.mcp_file() {
                let location = &file.at;
                let path = ctx.project_root.join(location.path);
                if let Err(error) = validate_destination_path(&ctx.project_root, &path, false) {
                    errors.push(error);
                } else if let Err(error) = crate::target::mcp_file::validate(location, &path) {
                    errors.push(format!(
                        "target `{target_name}` reads MCP servers from `{}`: {}",
                        location.path,
                        error_message(error)
                    ));
                }
            }
            if touches_hooks && let Some(merge) = super::merge_file(target_name) {
                let path = target_dir.join(merge.file);
                if let Err(error) = validate_destination_path(&ctx.project_root, &path, false) {
                    errors.push(error);
                }
                if let Err(error) = crate::target::hook_file::validate(&path) {
                    errors.push(error_message(error));
                }
            }
        }
    }
    for (dir, item) in targeted.target.selected_hooks() {
        let target_name = dir.target();
        let Some(target) = mars_schema::hooks::targets::target(target_name) else {
            errors.push(format!(
                "hook `{}`: target `{target_name}` has no hook fragment mechanism",
                item.def.name
            ));
            continue;
        };
        let installed = installed_token(ctx, dir);
        let tokens = Tokens {
            hook_dir: &installed,
            hook_target: target_name,
            ..Default::default()
        };
        let output = match load_output(item, target, &tokens) {
            Ok(output) => output,
            Err(error) => {
                errors.push(error_message(error));
                continue;
            }
        };
        match &output {
            HookOutput::Merge(merged) => {
                for event in &merged.unchecked_events {
                    diag.warn("hook-event-unchecked", format!("hook `{}` passes unknown event `{event}` through verbatim to `{target_name}` because `unchecked = true`", item.def.name));
                }
                for event in &merged.skipped_events {
                    diag.warn("hook-event-skipped", format!("hook `{}` from `{}`: skipping unknown event `{event}` for `{target_name}`; its other events are installed", item.def.name, item.source_name));
                }
                if let Some(fragment) = item.fragment(target_name) {
                    for event in &merged.overrides {
                        diag.info("hook-fragment-overrides", format!("hook `{}` from `{}`: `{target_name}` fragment `{}` replaces the generated `{event}` entries", item.def.name, item.source_name, fragment.file));
                    }
                }
            }
            HookOutput::File(files) => {
                let mut valid = true;
                for file in &files.files {
                    let destination = ctx.project_root.join(target_name).join(&file.path);
                    if let Err(error) =
                        validate_destination_path(&ctx.project_root, &destination, false)
                    {
                        errors.push(error);
                        valid = false;
                    }
                }
                if !valid {
                    continue;
                }
                if files.has_fragment_beside_generated() {
                    diag.info(
                        "hook-fragment-beside-generated",
                        format!(
                            "hook `{}` from `{}`: `{target_name}` fragment `{}` is installed next to the file generated from `[hooks]`; to replace the generated hook, put `{target_name}` in `skip-targets`",
                            item.def.name,
                            item.source_name,
                            item.fragment(target_name).map_or("", |f| f.file.as_str()),
                        ),
                    );
                }
                for limit in &files.answer_limits {
                    diag.info(
                        "hook-answer-unsupported",
                        format!(
                            "hook `{}` from `{}`: `{target_name}` `{}`: {}",
                            item.def.name,
                            item.source_name,
                            limit.event.as_deref().unwrap_or(""),
                            limit.reason,
                        ),
                    );
                }
                if let Some(problem) = files.fragment_problem {
                    diag.warn(
                        "hook-fragment-contract",
                        format!(
                            "hook `{}` from `{}`: `{target_name}` fragment `{}` {problem}; the fragment will still be installed",
                            item.def.name,
                            item.source_name,
                            item.fragment(target_name).map_or("", |f| f.file.as_str()),
                        ),
                    );
                }
            }
        }
        reported.insert(
            (hook_key(item), target_name.to_owned()),
            output.skips().to_vec(),
        );
        rendered.insert(dir.canonical().as_str().to_string(), output);
    }
    if errors.is_empty() {
        report_skips(
            targeted,
            &resolved.loaded.effective.settings.managed_targets(),
            reported,
            diag,
        );
        Ok(RenderedHooks(rendered))
    } else {
        Err(MarsError::Config(ConfigError::Invalid {
            message: errors.join("\n"),
        }))
    }
}

/// What a hook's `[hooks]` leaves out, said for every managed target whether or not the hook
/// was selected there, and for a hook no target took (D10). The project's own hooks warn, each skip once. A dependency's
/// hooks are the consumer's to accept, not fix: one summary line per sync.
fn report_skips(
    targeted: &crate::sync::TargetedState,
    managed: &[String],
    mut reported: HashMap<(String, String), Vec<Skip>>,
    diag: &mut DiagnosticCollector,
) {
    let mut seen = BTreeSet::new();
    let mut from_dependencies = Vec::new();
    for item in targeted.target.accepted_hooks() {
        if item.def.portable().is_none() || !seen.insert(hook_key(item)) {
            continue;
        }
        for target in managed {
            let skips = reported
                .remove(&(hook_key(item), target.clone()))
                .unwrap_or_else(|| item.def.skips_for(target));
            if skips.is_empty() {
                continue;
            }
            let lines: Vec<String> = skips.iter().map(skip_text).collect();
            match item.origin {
                Origin::Local => {
                    for line in lines {
                        diag.warn(
                            "hook-event-skipped",
                            format!("hook `{}`: `{target}` {line}", item.def.name),
                        );
                    }
                }
                Origin::Dependency => from_dependencies.push(format!(
                    "{}/{} on `{target}` ({} left out)",
                    item.source_name,
                    item.def.name,
                    lines.len()
                )),
            }
        }
    }
    if !from_dependencies.is_empty() {
        diag.info(
            "hook-event-skipped",
            format!(
                "some events or handlers of dependency hooks have no counterpart on a managed target: {}",
                from_dependencies.join(", ")
            ),
        );
    }
}

pub(crate) fn skip_text(skip: &Skip) -> String {
    match &skip.event {
        Some(event) => format!("does not run `{event}`: {}", skip.reason),
        None => format!("gets no generated hooks: {}", skip.reason),
    }
}

/// A hook, for grouping what is said about it: its package and name.
fn hook_key(item: &ParsedHookItem) -> String {
    format!("{}/{}", item.source_name, item.def.name)
}

/// Read and lower a plugin build's mergeable hook entries for one target.
///
/// The caller supplies each hook's directory token: the replacement for
/// `${MARS_HOOK_DIR}` in its native fragment. `${MARS_HOOK_TARGET}` is `plugin` there: the
/// tree is shared by every harness that reads it.
pub(crate) fn lower_hook_entries(
    hooks: &[(String, &ParsedHookItem)],
    adapter: &dyn TargetAdapter,
    diag: &mut DiagnosticCollector,
) -> Vec<HookEntry> {
    let Some(target) = mars_schema::hooks::targets::target(adapter.name()).filter(|t| !t.is_file())
    else {
        return Vec::new();
    };
    let mut outputs = Vec::new();
    for (dir, item) in hooks {
        let tokens = mars_schema::hooks::Tokens {
            hook_dir: dir,
            hook_target: "plugin",
            ..Default::default()
        };
        match crate::compiler::hooks::load_plugin_output(item, target, &tokens) {
            Ok(HookOutput::Merge(output)) => {
                for skip in &output.skips {
                    diag.warn(
                        "hook-event-skipped",
                        format!(
                            "hook `{}` from `{}`: the `{}` plugin tree {}",
                            item.def.name,
                            item.source_name,
                            adapter.name().trim_start_matches('.'),
                            skip_text(skip)
                        ),
                    );
                }
                outputs.push((*item, output));
            }
            Ok(HookOutput::File(_)) => {}
            Err(error) => crate::compiler::hooks::report_fragment(item, &error, diag),
        }
    }
    let outputs: Vec<_> = outputs
        .iter()
        .map(|(item, output)| (*item, output))
        .collect();
    lower_merge_fragments(&outputs, Vec::new())
}

/// Order validated merge fragments, and the entries of kept hooks (each with the rank it
/// had), deterministically and lower them to one entry per (hook, event). This sort is the
/// only source of order in a target's file.
pub(crate) fn lower_merge_fragments(
    fragments: &[(&ParsedHookItem, &MergeOutput)],
    kept: Vec<(HookRank, HookEntry)>,
) -> Vec<HookEntry> {
    let mut contributions: Vec<(HookRank, HookEntry)> = kept;
    for (item, fragment) in fragments {
        contributions.extend(
            fragment
                .events
                .iter()
                .filter(|(_, entries)| !entries.is_empty())
                .map(|(event, entries)| {
                    (
                        item.rank(),
                        HookEntry {
                            name: item.def.name.clone(),
                            native_event: event.clone(),
                            // `to_json` sorts keys: the bytes mars has always written.
                            entries: entries.iter().map(|entry| entry.to_json()).collect(),
                        },
                    )
                }),
        );
    }
    contributions.sort_by(|(left_rank, left), (right_rank, right)| {
        (left_rank, &left.name, &left.native_event).cmp(&(
            right_rank,
            &right.name,
            &right.native_event,
        ))
    });
    contributions.into_iter().map(|(_, entry)| entry).collect()
}

fn validate_destination_path(
    root: &std::path::Path,
    destination: &std::path::Path,
    directory: bool,
) -> Result<(), String> {
    let relative = destination.strip_prefix(root).unwrap_or(destination);
    let mut current = root.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        let is_destination = index + 1 == components.len();
        let Ok(metadata) = std::fs::metadata(&current) else {
            continue;
        };
        let expects_directory = !is_destination || directory;
        if metadata.is_dir() != expects_directory {
            let expected = if expects_directory {
                "directory"
            } else {
                "file"
            };
            return Err(format!(
                "destination `{}` must be a {expected}",
                current.display()
            ));
        }
    }
    Ok(())
}
