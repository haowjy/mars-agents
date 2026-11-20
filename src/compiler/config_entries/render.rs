//! Hook preflight: read, validate and render every selected hook once, before any mutation.
//! Compile reuses the result.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::installed_token;
use crate::compiler::hooks::{HookRank, ParsedHookItem, error_message};
use crate::diagnostic::DiagnosticCollector;
use crate::sync::package_dir::PackageDir;
use crate::target::mcp_file::MCP_KIND;
use crate::target::plugin::HOOK_ENTRY_KIND;
use crate::target::{HOOK_KEY_PREFIX, HookEntry, TargetAdapter};
use crate::types::MarsContext;
use mars_schema::hooks::render::Skip;
use mars_schema::hooks::targets::Profile;
use mars_schema::hooks::{HookOutput, MergeOutput, Origin};

pub(crate) struct RenderedHooks(HashMap<String, HookOutput>);

impl RenderedHooks {
    pub(crate) fn get(&self, dir: &PackageDir) -> &HookOutput {
        self.0
            .get(dir.canonical().as_str())
            .expect("preflight rendered every selected hook")
    }
}

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
    // Targets whose keyed files and hook files this sync reads: those the old lock has records
    // on, those with something selected, and a managed target's fixed entries.
    let managed = crate::config::targets::managed_targets(&resolved.loaded.effective.settings);
    let mut keyed_touched: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
    let mut hook_targets: BTreeSet<String> = BTreeSet::new();
    for target in &managed {
        for (kind, _) in registry
            .get(target)
            .into_iter()
            .flat_map(|a| a.fixed_entries(&ctx.project_root))
        {
            keyed_touched
                .entry(target.clone())
                .or_default()
                .insert(kind.key_prefix);
        }
    }
    for (target, records) in &resolved.loaded.old_lock.config_entries {
        for (kind, _) in registry
            .get(target)
            .into_iter()
            .flat_map(|adapter| adapter.keyed_sections())
        {
            if records.keys().any(|key| key.starts_with(kind.key_prefix)) {
                keyed_touched
                    .entry(target.clone())
                    .or_default()
                    .insert(kind.key_prefix);
            }
        }
        if records.keys().any(|key| key.starts_with(HOOK_KEY_PREFIX)) {
            hook_targets.insert(target.clone());
        }
    }
    for target in targeted.target.selected_mcp_targets() {
        keyed_touched
            .entry(target.to_string())
            .or_default()
            .insert(MCP_KIND.key_prefix);
    }
    for (dir, _) in targeted.target.selected_hooks() {
        let target = dir.target().to_string();
        if mars_schema::hooks::targets::target(&target).is_some_and(|t| t.keyed_file().is_some()) {
            keyed_touched
                .entry(target.clone())
                .or_default()
                .insert(HOOK_ENTRY_KIND.key_prefix);
        }
        hook_targets.insert(target);
    }
    for target_name in keyed_touched
        .keys()
        .chain(hook_targets.iter())
        .collect::<BTreeSet<_>>()
    {
        let target_dir = ctx.project_root.join(target_name);
        if let Err(error) = validate_destination_path(&ctx.project_root, &target_dir, true) {
            errors.push(error);
            continue;
        }
        let Some(adapter) = registry.get(target_name) else {
            continue;
        };
        for (kind, location) in adapter.keyed_sections() {
            if !keyed_touched
                .get(target_name)
                .is_some_and(|kinds| kinds.contains(kind.key_prefix))
            {
                continue;
            }
            let path = ctx.project_root.join(location.path);
            if let Err(error) = validate_destination_path(&ctx.project_root, &path, false) {
                errors.push(error);
            } else if let Err(error) = crate::target::keyed_entries::validate(&location, &path) {
                errors.push(format!(
                    "target `{target_name}` reads {} from `{}`: {}",
                    kind.plural,
                    location.path,
                    error_message(error)
                ));
            }
        }
        if hook_targets.contains(target_name)
            && let Some(merge) = super::merge_file(target_name)
        {
            let path = target_dir.join(merge.file);
            if let Err(error) = validate_destination_path(&ctx.project_root, &path, false) {
                errors.push(error);
            }
            if let Err(error) = crate::target::hook_file::validate(&path) {
                errors.push(error_message(error));
            }
        }
    }
    // A translated hook's command runs the `mars` that wrote it, by absolute path.
    let mars_exe = invoked_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    for (dir, item) in targeted.target.selected_hooks() {
        let target_name = dir.target();
        let Some(target) = mars_schema::hooks::targets::target(target_name) else {
            errors.push(format!(
                "hook `{}`: target `{target_name}` has no native hook mechanism",
                item.def.name
            ));
            continue;
        };
        // A keyed hook's commands run from the plugin directory, so its directory is the
        // relative path there and the file stays valid wherever the target is.
        let installed = if target.keyed_file().is_some() {
            dir.native()
        } else {
            installed_token(ctx, dir)
        };
        let tokens = Tokens {
            hook_dir: &installed,
            hook_target: target_name,
            mars_exe: &mars_exe,
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
            HookOutput::Merge(merged) | HookOutput::Keyed(merged) => {
                for file in &merged.files {
                    let destination = ctx.project_root.join(target_name).join(&file.path);
                    if let Err(error) =
                        validate_destination_path(&ctx.project_root, &destination, false)
                    {
                        errors.push(error);
                    }
                }
                for limit in &merged.answer_limits {
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
                for event in &merged.unchecked_events {
                    diag.warn_advisory("hook-event-unchecked", format!("hook `{}` passes unknown event `{event}` through verbatim to `{target_name}` because `unchecked = true`", item.def.name));
                }
                for event in &merged.skipped_events {
                    diag.warn_advisory("hook-event-skipped", format!("hook `{}` from `{}`: skipping unknown event `{event}` for `{target_name}`; its other events are installed", item.def.name, item.source_name));
                }
                if let Some(native_hook) = item.native_hook(target_name) {
                    for event in &merged.overrides {
                        diag.info("hook-native-overrides", format!("hook `{}` from `{}`: `{target_name}` native hook `{}` replaces the generated `{event}` entries", item.def.name, item.source_name, native_hook.file));
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
                if files.has_native_hook_beside_generated() {
                    diag.info(
                        "hook-native-beside-generated",
                        format!(
                            "hook `{}` from `{}`: `{target_name}` native hook `{}` is installed next to the file generated from `[hooks]`; to replace the generated hook, put `{target_name}` in `skip-targets`",
                            item.def.name,
                            item.source_name,
                            item.native_hook(target_name).map_or("", |f| f.file.as_str()),
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
                if let Some(problem) = files.native_hook_problem {
                    diag.warn_advisory(
                        "hook-native-contract",
                        format!(
                            "hook `{}` from `{}`: `{target_name}` native hook `{}` {problem}; the native hook will still be installed",
                            item.def.name,
                            item.source_name,
                            item.native_hook(target_name).map_or("", |f| f.file.as_str()),
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
            &crate::config::targets::managed_targets(&resolved.loaded.effective.settings),
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
/// was selected there, and for a hook no target took. The project's own hooks warn, each skip once. A dependency's
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
                        diag.warn_advisory(
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

fn hook_key(item: &ParsedHookItem) -> String {
    format!("{}/{}", item.source_name, item.def.name)
}

/// The caller supplies each hook's directory token: the replacement for
/// `${MARS_HOOK_DIR}` in its native hook. `${MARS_HOOK_TARGET}` is `plugin` there: the
/// tree is shared by every harness that reads it.
pub(crate) fn lower_hook_entries(
    hooks: &[(String, &ParsedHookItem)],
    adapter: &dyn TargetAdapter,
    shared: Option<&Profile>,
    diag: &mut DiagnosticCollector,
) -> Vec<HookEntry> {
    let Some(target) =
        mars_schema::hooks::targets::target(adapter.name()).filter(|t| t.merge_file().is_some())
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
        match crate::compiler::hooks::load_plugin_output(item, target, shared, &tokens) {
            Ok(HookOutput::Merge(output)) => {
                for skip in &output.skips {
                    diag.warn_advisory(
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
            Ok(HookOutput::File(_) | HookOutput::Keyed(_)) => {}
            Err(error) => crate::compiler::hooks::report_native_hook(item, &error, diag),
        }
    }
    let outputs: Vec<_> = outputs
        .iter()
        .map(|(item, output)| (*item, output))
        .collect();
    lower_merge_outputs(&outputs, Vec::new())
}

/// Order validated merge outputs, and the entries of kept hooks (each with the rank it
/// had), deterministically and lower them to one entry per (hook, event). This sort is the
/// only source of order in a target's file.
pub(crate) fn lower_merge_outputs(
    outputs: &[(&ParsedHookItem, &MergeOutput)],
    kept: Vec<(HookRank, HookEntry)>,
) -> Vec<HookEntry> {
    let mut contributions: Vec<(HookRank, HookEntry)> = kept;
    for (item, output) in outputs {
        contributions.extend(
            output
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

/// The path this `mars` was invoked by, when that is a link to the running binary: a Homebrew
/// install links a stable `bin/mars` to a versioned cellar, and `current_exe` resolves the
/// link, so its answer goes stale at the next upgrade. An absolute `argv[0]`, or the `PATH`
/// entry a bare name resolves to, stays unresolved; anything else is `current_exe`.
fn invoked_exe() -> Option<std::path::PathBuf> {
    let real = std::env::current_exe().ok()?;
    let argv0 = std::path::PathBuf::from(std::env::args_os().next()?);
    let invoked = if argv0.is_absolute() {
        Some(argv0)
    } else if argv0.components().count() == 1 {
        which::which(&argv0).ok().filter(|path| path.is_absolute())
    } else {
        None
    };
    invoked
        .filter(|path| std::fs::canonicalize(path).ok() == std::fs::canonicalize(&real).ok())
        .or(Some(real))
}
