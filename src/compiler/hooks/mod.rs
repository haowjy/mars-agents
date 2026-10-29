//! Native hook discovery and loading. The `hook.toml` model, its reader and every
//! per-target hook rule are `mars_schema::hooks`; this module reads the bytes and adds the
//! location to the schema's path-free diagnostics.

use std::path::{Path, PathBuf};

use mars_schema::diagnostic::Severity;
use mars_schema::hooks::targets::{Profile, Target};
use mars_schema::hooks::{self, HookDef, HookOutput, NativeHook, Origin, Tokens};

use crate::diagnostic::DiagnosticCollector;
use crate::error::{ConfigError, MarsError};
use crate::types::managed_cmd;

#[derive(Debug, Clone)]
pub struct ParsedHookItem {
    pub def: HookDef,
    pub source_name: String,
    pub package_depth: usize,
    pub decl_order: usize,
    pub hook_dir: PathBuf,
    /// Whose `hook.toml` this is: a dependency's native hooks are read leniently.
    pub origin: Origin,
}

/// Where a hook sorts among a target's hooks: package depth, declaration order, the hook's
/// own `order`. Merge order is the only source of order in a target's file, so a kept hook
/// carries the rank it had instead of being appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct HookRank {
    pub package_depth: usize,
    pub decl_order: usize,
    pub order: i32,
}

impl ParsedHookItem {
    pub(crate) fn rank(&self) -> HookRank {
        HookRank {
            package_depth: self.package_depth,
            decl_order: self.decl_order,
            order: self.def.order,
        }
    }

    pub(crate) fn discovered_item(&self, package_root: &Path) -> crate::discover::DiscoveredItem {
        crate::discover::DiscoveredItem {
            id: crate::lock::ItemId {
                kind: mars_schema::item::ItemKind::Hook,
                name: self.def.name.clone().into(),
            },
            source_path: self
                .hook_dir
                .strip_prefix(package_root)
                .expect("parsed hook belongs to its package root")
                .to_path_buf(),
        }
    }

    /// The native hook table this hook declares for `target`; a `[hooks]`-only hook has none.
    pub(crate) fn native_hook(&self, target: &str) -> Option<&NativeHook> {
        self.def.native_hook(target)
    }
}

/// Whose `hook.toml` is being read, for the location in an error.
#[derive(Debug, Clone, Copy)]
pub enum HookSource<'a> {
    /// The project's own hook.
    Project,
    /// A dependency's hook, at this package version.
    Dependency { version: &'a str },
}

/// A dependency hook that failed validation. `error` carries its location.
///
/// What the file says about itself is read leniently, since it may not parse: the last
/// install is found from it, but a kept hook is only ever one the old lock installed.
#[derive(Debug)]
pub struct InvalidHook {
    /// What selection knows the hook by (see `discover::hook_slug`).
    pub slug: String,
    /// The hook's directory in the package, which a lock records for finding its last install.
    pub hook_dir: PathBuf,
    /// The names the last sync may have installed it under: the `name` the file still
    /// declares, then its directory's. A hook is found by the first one the lock holds.
    pub installed_names: Vec<String>,
    /// The file still declares `visibility = "exported"`: a hook that is not exported was
    /// not installed for this consumer.
    pub exported: bool,
    /// The `order` the file still declares (0 when it does not read), for a kept hook's place.
    pub order: i32,
    pub error: MarsError,
    /// The file is in the schema Mars 0.11 removed.
    pub removed_schema: bool,
}

impl InvalidHook {
    /// A `hook.toml` that does not read.
    fn unreadable(text: &str, hook_dir: &Path, error: MarsError, removed_schema: bool) -> Self {
        let dir_name = &*hook_dir
            .file_name()
            .expect("discovered hook directory has a name")
            .to_string_lossy();
        let mut installed_names: Vec<String> = declared(text, "name")
            .and_then(|name| name.as_str().map(str::to_owned))
            .into_iter()
            .collect();
        installed_names.push(dir_name.to_owned());
        installed_names.dedup();
        Self {
            slug: crate::discover::hook_slug(text, dir_name),
            hook_dir: hook_dir.to_path_buf(),
            installed_names,
            exported: declared(text, "visibility").is_some_and(|v| v.as_str() == Some("exported")),
            order: declared(text, "order")
                .and_then(|order| order.as_integer())
                .and_then(|order| i32::try_from(order).ok())
                .unwrap_or(0),
            error,
            removed_schema,
        }
    }

    /// A hook that read but fails to render.
    fn unrenderable(item: &ParsedHookItem, error: MarsError) -> Self {
        Self {
            slug: item.def.name.clone(),
            hook_dir: item.hook_dir.clone(),
            installed_names: vec![item.def.name.clone()],
            exported: item.def.visibility == mars_schema::visibility::Visibility::Exported,
            order: item.def.order,
            error,
            removed_schema: false,
        }
    }
}

/// A top-level `key` of a `hook.toml`, read even when the rest does not parse: the
/// first `key = …` line before the first table, which is where an author puts it.
fn declared(text: &str, key: &str) -> Option<toml::Value> {
    let value = |text: &str| toml::from_str::<toml::Value>(text).ok()?.get(key).cloned();
    value(text).or_else(|| {
        text.lines()
            .take_while(|line| !line.trim_start().starts_with('['))
            .find_map(value)
    })
}

/// The project's own hooks: the first invalid one is an error.
pub fn discover_hook_items(
    package_root: &Path,
    source_name: &str,
    source: HookSource<'_>,
    package_depth: usize,
    decl_order: usize,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<ParsedHookItem>, MarsError> {
    discover_each(
        package_root,
        source_name,
        source,
        package_depth,
        decl_order,
        diag,
    )?
    .into_iter()
    .map(|hook| hook.map_err(|invalid| invalid.error))
    .collect()
}

/// A dependency's hooks, one result each: an invalid `hook.toml` does not stop the others.
/// A hook in the removed schema still fails the sync, whatever the consumer selects: the
/// package must be upgraded or removed.
pub(crate) fn discover_resolved_hook_items(
    node: &crate::resolve::ResolvedNode,
    source_name: &crate::types::SourceName,
    package_depth: usize,
    decl_order: usize,
    managed_targets: &[String],
    diag: &mut DiagnosticCollector,
) -> Result<Vec<Result<ParsedHookItem, InvalidHook>>, MarsError> {
    let version = node
        .resolved_ref
        .version_tag
        .as_deref()
        .map(|version| version.trim_start_matches('v'))
        .or_else(|| {
            node.manifest
                .as_ref()
                .map(|manifest| manifest.package.version.as_str())
        })
        .unwrap_or("unknown");
    let hooks = discover_each(
        &node.rooted_ref.package_root,
        source_name.as_str(),
        HookSource::Dependency { version },
        package_depth,
        decl_order,
        diag,
    )?;
    let mut checked = Vec::with_capacity(hooks.len());
    for hook in hooks {
        checked.push(match hook {
            Err(InvalidHook {
                error,
                removed_schema: true,
                ..
            }) => return Err(error),
            // The one place a dependency's hook is judged invalid: it must also render on
            // every managed target it is installed for.
            Ok(item) => match render_check(&item, managed_targets) {
                Ok(()) => Ok(item),
                Err(error) => Err(InvalidHook::unrenderable(&item, error)),
            },
            invalid => invalid,
        });
    }
    Ok(checked)
}

fn discover_each(
    package_root: &Path,
    source_name: &str,
    source: HookSource<'_>,
    package_depth: usize,
    decl_order: usize,
    diag: &mut DiagnosticCollector,
) -> Result<Vec<Result<ParsedHookItem, InvalidHook>>, MarsError> {
    let mut hooks = Vec::new();
    for hook_dir in crate::discover::discover_hook_directories(package_root, diag)? {
        hooks.push(load_hook_item(
            &hook_dir,
            source_name,
            source,
            package_depth,
            decl_order,
            diag,
        )?);
    }
    Ok(hooks)
}

/// Read one hook directory. The outer error is an unreadable file; the inner one a
/// `hook.toml` that fails validation.
fn load_hook_item(
    hook_dir: &Path,
    source_name: &str,
    source: HookSource<'_>,
    package_depth: usize,
    decl_order: usize,
    diag: &mut DiagnosticCollector,
) -> Result<Result<ParsedHookItem, InvalidHook>, MarsError> {
    let dir_name = hook_dir
        .file_name()
        .expect("discovered hook directory has a name")
        .to_string_lossy()
        .into_owned();
    let toml_path = hook_dir.join("hook.toml");
    let text = std::fs::read_to_string(&toml_path)?;
    let origin = match source {
        HookSource::Project => Origin::Local,
        HookSource::Dependency { .. } => Origin::Dependency,
    };
    let read = hooks::read(&text, &dir_name, origin);
    for diagnostic in read
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Warning)
    {
        diag.warn(
            diagnostic_code(&diagnostic.code),
            format!(
                "{}: {}",
                located(&toml_path, &dir_name, source_name, source),
                diagnostic.message()
            ),
        );
    }
    if let Some(error) = read
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Ok(Err(InvalidHook::unreadable(
            &text,
            hook_dir,
            invalid_manifest(
                &toml_path,
                &dir_name,
                source_name,
                source,
                error.code.as_str(),
                error.message(),
            ),
            error.code.as_str() == hooks::REMOVED_SCHEMA_CODE,
        )));
    }
    let def = read
        .def
        .expect("a reading with no error holds its definition");
    if let Some(text) = &read.unknown_visibility {
        crate::compiler::warn_unknown_visibility(text, "hook", &def.name, source_name, diag);
    }
    Ok(Ok(ParsedHookItem {
        def,
        source_name: source_name.to_string(),
        package_depth,
        decl_order,
        hook_dir: hook_dir.to_path_buf(),
        origin,
    }))
}

/// The sync diagnostic code for a schema warning: an unknown event is `hook-event-skipped`,
/// any other `hook.<name>` is `hook-<name>`.
fn diagnostic_code(code: &str) -> String {
    match code {
        "hook.unknown-event" => "hook-event-skipped".to_owned(),
        other => other.replace('.', "-"),
    }
}

/// Where a `hook.toml` is, for a message: its path for the project's hook, the package,
/// version and hook for a dependency's (whose path is a staging directory).
fn located(toml_path: &Path, hook_name: &str, source_name: &str, source: HookSource<'_>) -> String {
    match source {
        HookSource::Project => toml_path.display().to_string(),
        HookSource::Dependency { version } => {
            format!("source package `{source_name}` version `{version}`, hook `{hook_name}`")
        }
    }
}

/// An error's message without its `config error:` prefix: preflight wraps the
/// joined messages in one `ConfigError::Invalid`, which supplies it.
pub(crate) fn error_message(error: crate::error::MarsError) -> String {
    match error {
        crate::error::MarsError::Config(crate::error::ConfigError::Invalid { message }) => message,
        other => other.to_string(),
    }
}

/// The one place a `hook.toml` diagnostic gains its location: the real path for the
/// project's hook, the package, version and hook name for a dependency's (whose path is
/// a staging directory).
fn invalid_manifest(
    toml_path: &Path,
    hook_name: &str,
    source_name: &str,
    source: HookSource<'_>,
    code: &str,
    message: &str,
) -> MarsError {
    let message = match source {
        HookSource::Project => format!("{}: {message}", toml_path.display()),
        HookSource::Dependency { version } if code == hooks::REMOVED_SCHEMA_CODE => format!(
            "source package `{source_name}` version `{version}` {message}; \
             suggested: `{cmd_upgrade}` or `{cmd_remove}`",
            cmd_upgrade = managed_cmd(&format!("mars upgrade {source_name} --bump")),
            cmd_remove = managed_cmd(&format!("mars remove {source_name}")),
        ),
        HookSource::Dependency { version } => format!(
            "source package `{source_name}` version `{version}`, hook `{hook_name}`: {message}"
        ),
    };
    ConfigError::Invalid { message }.into()
}

/// What an installed hook puts on `target`: its `[hooks]` rendered, with its native hook (when
/// it declares one for the target) laid over them.
pub fn load_output(
    item: &ParsedHookItem,
    target: &Target,
    tokens: &Tokens<'_>,
) -> Result<HookOutput, MarsError> {
    load(item, target, |native_hook| {
        item.def.output(target, native_hook, item.origin, tokens)
    })
}

/// What an installed hook puts in a plugin build's tree for `target`; see
/// `HookDef::plugin_output`.
pub fn load_plugin_output(
    item: &ParsedHookItem,
    target: &Target,
    shared: Option<&Profile>,
    tokens: &Tokens<'_>,
) -> Result<HookOutput, MarsError> {
    load(item, target, |native_hook| {
        item.def
            .plugin_output(target, shared, native_hook, item.origin, tokens)
    })
}

/// Say a hook's native hook could not be loaded for a plugin build: an error for the project's
/// own hook, a warning for a dependency's, which never fails the build (D19).
pub(crate) fn report_native_hook(
    item: &ParsedHookItem,
    error: &MarsError,
    diag: &mut DiagnosticCollector,
) {
    match item.origin {
        Origin::Local => diag.error("hook-native-hook", error.to_string()),
        Origin::Dependency => diag.warn("hook-native-hook", error.to_string()),
    }
}

fn load<T>(
    item: &ParsedHookItem,
    target: &Target,
    output: impl FnOnce(Option<&str>) -> Result<T, mars_schema::hooks::NativeHookError>,
) -> Result<T, MarsError> {
    let native_hook = item
        .native_hook(target.root)
        .map(|native_hook| read_native_hook(item, &native_hook.file))
        .transpose()?;
    output(native_hook.as_deref()).map_err(|error| {
        let file = native_hook.as_ref().map_or("hook.toml", |_| {
            item.native_hook(target.root).map_or("", |f| &f.file)
        });
        native_hook_error(item, file, &error.to_string())
    })
}

/// Whether a dependency hook's native hook loads on every managed target it is installed for.
/// `[hooks]` never fails to render, so it is left to preflight, which renders each hook once
/// (D25); the tokens only change string values, so none are needed here.
fn render_check(item: &ParsedHookItem, managed_targets: &[String]) -> Result<(), MarsError> {
    for root in item.def.select(managed_targets) {
        let Some(target) = hooks::targets::target(root) else {
            return Err(native_hook_error(
                item,
                "hook.toml",
                &format!("target `{root}` has no native hook mechanism"),
            ));
        };
        load(item, target, |native_hook| {
            item.def.check_native_hook(target, native_hook, item.origin)
        })?;
    }
    Ok(())
}

fn read_native_hook(item: &ParsedHookItem, file: &str) -> Result<String, MarsError> {
    std::fs::read_to_string(item.hook_dir.join(file)).map_err(|error| {
        native_hook_error(item, file, &format!("failed to read native hook: {error}"))
    })
}

/// A hook's file that cannot be used, located by its path for the project's hook and by its
/// package and hook for a dependency's, whose path is a staging directory.
fn native_hook_error(item: &ParsedHookItem, file: &str, message: &str) -> MarsError {
    let located = match item.origin {
        Origin::Local => item.hook_dir.join(file).display().to_string(),
        Origin::Dependency => format!(
            "source package `{}`, hook `{}`, `{file}`",
            item.source_name, item.def.name
        ),
    };
    MarsError::Config(ConfigError::Invalid {
        message: format!("{located}: {message}"),
    })
}

/// The first Mars that reads `[hooks]`; older ones reject the whole `hook.toml` and, with it,
/// the sync.
const PORTABLE_HOOKS_MARS_FLOOR: &str = "0.25.0";

/// What `mars check` says about a package's own hooks: `(errors, warnings)`. The author can
/// fix every one, so a hook that renders nowhere or loses a handler is a warning, not a note.
pub(crate) fn check_hooks(
    package_root: &Path,
    requires_mars: Option<Option<&str>>,
    diag: &mut DiagnosticCollector,
) -> Result<(Vec<String>, Vec<String>), MarsError> {
    let items = match discover_hook_items(
        package_root,
        "this package",
        HookSource::Project,
        0,
        0,
        diag,
    ) {
        Ok(items) => items,
        Err(error) => return Ok((vec![error_message(error)], Vec::new())),
    };
    let mut warnings = Vec::new();
    for item in &items {
        let Some(portable) = item.def.portable() else {
            continue;
        };
        let name = &item.def.name;
        if let Some(requirement) = requires_mars {
            warnings.extend(mars_schema::package::requires_mars_floor_warning(
                requirement,
                PORTABLE_HOOKS_MARS_FLOOR,
                &format!("hook `{name}` uses `[hooks]`, which a Mars before {PORTABLE_HOOKS_MARS_FLOOR} rejects, failing the whole sync"),
            ));
        }
        for event in portable.conditions_on_non_tool_events() {
            warnings.push(format!(
                "hook `{name}`: `if` on `{event}` never matches: Claude does not run a handler with `if` on a non-tool event"
            ));
        }
        // Cursor renders nothing before 0.26, so its one blanket skip is no reason an author
        // can act on.
        for target in mars_schema::hooks::targets::all()
            .iter()
            .filter(|target| target.profile.renders())
        {
            for skip in item.def.skips_for(target.root) {
                warnings.push(format!(
                    "hook `{name}`: `{}` {}",
                    target.root,
                    crate::compiler::config_entries::render::skip_text(&skip)
                ));
            }
        }
    }
    Ok((Vec::new(), warnings))
}
