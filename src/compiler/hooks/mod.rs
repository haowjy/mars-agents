//! Native hook discovery and fragment loading. The `hook.toml` model, its reader and every
//! per-target hook rule are `mars_schema::hooks`; this module reads the bytes and adds the
//! location to the schema's path-free diagnostics.

use std::path::{Path, PathBuf};

use mars_schema::diagnostic::Severity;
use mars_schema::hooks::targets::Target;
use mars_schema::hooks::{self, Fragment, HookDef, HookOutput, Origin, Tokens};

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
    /// Whose `hook.toml` this is: a dependency's fragments are read leniently.
    pub origin: Origin,
}

impl ParsedHookItem {
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

    /// The fragment table this hook declares for `target`; a `[hooks]`-only hook has none.
    pub(crate) fn fragment(&self, target: &str) -> Option<&Fragment> {
        self.def.fragment(target)
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

/// A `hook.toml` that failed validation. `error` carries its location; `name` is the hook's
/// name when the file said one, else its directory's.
#[derive(Debug)]
pub struct InvalidHook {
    pub name: String,
    pub error: MarsError,
    /// The file is in the schema Mars 0.11 removed.
    pub removed_schema: bool,
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
        match hook {
            Err(InvalidHook {
                error,
                removed_schema: true,
                ..
            }) => return Err(error),
            other => checked.push(other),
        }
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
        return Ok(Err(InvalidHook {
            name: toml::from_str::<toml::Value>(&text)
                .ok()
                .and_then(|value| value.get("name")?.as_str().map(str::to_owned))
                .unwrap_or(dir_name.clone()),
            removed_schema: error.code.as_str() == hooks::REMOVED_SCHEMA_CODE,
            error: invalid_manifest(
                &toml_path,
                &dir_name,
                source_name,
                source,
                error.code.as_str(),
                error.message(),
            ),
        }));
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

/// What an installed hook puts on `target`: its `[hooks]` rendered, with its fragment (when
/// it declares one for the target) laid over them.
pub fn load_output(
    item: &ParsedHookItem,
    target: &Target,
    tokens: &Tokens<'_>,
) -> Result<HookOutput, MarsError> {
    let fragment = item
        .fragment(target.root)
        .map(|fragment| {
            let path = item.hook_dir.join(&fragment.file);
            read_fragment(&path).map(|text| (path, text))
        })
        .transpose()?;
    item.def
        .output(
            target,
            fragment.as_ref().map(|(_, text)| text.as_str()),
            item.origin,
            tokens,
        )
        .map_err(|error| {
            let path = fragment
                .as_ref()
                .map_or_else(|| item.hook_dir.join("hook.toml"), |(path, _)| path.clone());
            invalid(&path, &error.to_string())
        })
}

fn read_fragment(path: &Path) -> Result<String, MarsError> {
    std::fs::read_to_string(path)
        .map_err(|error| invalid(path, &format!("failed to read fragment: {error}")))
}

fn invalid(path: &Path, message: &str) -> MarsError {
    MarsError::Config(ConfigError::Invalid {
        message: format!("{}: {message}", path.display()),
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
        // Only the targets that render `[hooks]` today have a reason an author can act on;
        // the others report "not rendered yet" at sync.
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
