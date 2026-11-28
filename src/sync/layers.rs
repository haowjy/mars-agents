//! The layers a project's sync brings in beside its own dependencies (`crate::layer`): the user
//! level and a host's bundle. Each enters as ordinary dependencies named with its layer's
//! prefix, so everything after `load_config` (resolution, the compiler, per-target lowering,
//! `mars why`, removal) treats them as any dependency, with the layer table's rules for claims,
//! targets and the machine-local split.
//!
//! The user level enters as `_user-<name>` per entry of its manifest's `[dependencies]`, pinned
//! to its own `mars.lock` (`ResolveOptions::with_user_layer`): a project sync replays what
//! `mars --user sync` resolved, from the shared source cache, and never resolves it again. Its
//! own items enter as `_user-self`, a path dependency on its `.mars-src/`. A user level whose lock
//! does not load, or that was never synced, contributes nothing, with a warning.
//!
//! A host enters as `_provided-<host>`, a path dependency on its bundle directory, offered only
//! to the targets that take a host's content. The bundle comes from `--provided-root`, else from
//! the lock row an earlier sync recorded while that directory exists, else from what
//! `mars provide` staged (`.mars/provided/<host>/`, hooks and MCP servers only).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use mars_schema::dependency::Entry;

use crate::config::{EffectiveDependency, EntrySource, SourceSpec};
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::layer::{HOST_PREFIX, USER_PREFIX, USER_SELF};
use crate::layout::Layout;
use crate::lock::LockFile;
use crate::types::SourceName;

/// A host's bundle named on the command line: `<host>@<version>=<directory>`.
#[derive(Debug, Clone)]
pub struct HostRoot {
    pub provider: String,
    /// Opaque: recorded so a host upgrade is news, never compared.
    pub version: String,
    pub dir: PathBuf,
}

#[derive(Default)]
pub(crate) struct Layers {
    pub dependencies: Vec<(SourceName, EffectiveDependency)>,
    pub user_layer: crate::resolve::UserLayer,
    /// The version the lock records for each host read from a bundle directory, by host.
    pub host_versions: BTreeMap<String, String>,
}

/// The skills a host makes available here, whatever the managed targets take: what each bundle
/// the sync knows ships, and, for one minor, what `mars provide` installed (`ProvidedNames`). A
/// skill a target does not take is supplied by the host where it runs the agent. The skill-ref
/// judge reads this as "provided here"; nothing is installed from it (`layer::offered_to`
/// decides what is written).
#[derive(Debug, Default)]
pub(crate) struct HostSkills(HashSet<String>);

impl HostSkills {
    pub(crate) fn new(
        layout: &Layout,
        lock: &LockFile,
        host: Option<&HostRoot>,
        provided: &crate::provided::ProvidedNames,
    ) -> Self {
        let mut names: HashSet<String> = host_bundles(layout, lock, host)
            .values()
            .flat_map(|(dir, _)| bundle_skills(dir))
            .collect();
        names.extend(provided.installed().map(str::to_owned));
        Self(names)
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }
}

/// The names of the skills a bundle ships: each `skills/<name>/SKILL.md`. A bundle that cannot
/// be read ships none here; reading it to install reports why.
fn bundle_skills(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir.join("skills")) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("SKILL.md").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

pub(crate) fn load(
    layout: &Layout,
    managed_targets: &[String],
    old_lock: &LockFile,
    host: Option<&HostRoot>,
    diag: &mut DiagnosticCollector,
) -> Result<Layers, MarsError> {
    let mut layers = Layers::default();
    // The user level is the layer, not something it pulls in. The home project, where the user
    // level used to be, never syncs: `--root ~` opens it only to unlink (`crate::home_project`).
    if layout.kind() == crate::layout::RootKind::Project {
        user_layer(&mut layers, diag)?;
    }
    if crate::provided::takes_provided_any(managed_targets) {
        host_layer(
            &mut layers,
            layout,
            host_bundles(layout, old_lock, host),
            host,
        )?;
    }
    Ok(layers)
}

fn path_dependency(dir: &Path, base: &Path) -> EffectiveDependency {
    let dependency = Entry::at_path(dir.to_path_buf());
    let EntrySource {
        declared_source_id,
        source_id,
        spec,
    } = crate::config::entry_source(&dependency, base, None);
    EffectiveDependency {
        declared_source_id,
        source_id,
        spec,
        subpath: None,
        filter: dependency.filter.to_mode(),
        rename: Default::default(),
        dialect: None,
    }
}

fn user_layer(layers: &mut Layers, diag: &mut DiagnosticCollector) -> Result<(), MarsError> {
    // A user level that cannot be located is reported once, by the user config's load.
    let Some(user) = Layout::user_level() else {
        return Ok(());
    };
    // Advisory: the user level is a separate root, so these repeat on every sync until it is
    // fixed there, and the project itself reached the state it asked for.
    let skipped = |diag: &mut DiagnosticCollector, why: String| {
        diag.warn_advisory(
            "user-level-skipped",
            format!("the user level's agents, skills, hooks and MCP servers are not installed here: {why}"),
        );
    };
    // Its folder's files are the home project's, whatever user files sit beside them; the run
    // ends with the steps to move it.
    if crate::home_project::HomeProject::in_user_folder().is_some() {
        crate::home_project::raise_notice();
        return Ok(());
    }
    if !user.manifest().is_file() && !user.local_source().is_dir() {
        return Ok(());
    }
    if !user.lock().is_file() {
        skipped(
            diag,
            format!("it has not been synced; run `{}`", user.cmd("sync")),
        );
        return Ok(());
    }
    let loaded = crate::lock::load(&user).and_then(|lock| {
        let config = crate::config::load(&user)?;
        let local = crate::config::load_local(&user)?;
        let settings = crate::config::load_user_config()?;
        let (effective, _) = crate::config::merge_with_root(config, local, &settings, user.root())?;
        Ok((lock, effective))
    });
    let (lock, effective) = match loaded {
        Ok(loaded) => loaded,
        Err(err) => {
            skipped(diag, err.to_string());
            return Ok(());
        }
    };

    for (name, mut dependency) in effective.dependencies {
        if let SourceSpec::Git(git) = &mut dependency.spec {
            let pin = lock.dependencies.get(&name).filter(|pin| {
                pin.url.as_deref().is_some_and(|url| {
                    mars_schema::git_url::canonicalize_git_url(url)
                        == mars_schema::git_url::canonicalize_git_url(git.url.as_ref())
                })
            });
            let Some(pin) = pin else {
                diag.warn_advisory(
                    "user-level-unsynced",
                    format!(
                        "user dependency `{name}` is not installed here: the user level's mars.toml changed it since its last sync; run `{}`",
                        user.cmd("sync")
                    ),
                );
                continue;
            };
            // The version the user level resolved, so the replay meets its own constraint.
            if pin.version.is_some() {
                git.version.clone_from(&pin.version);
            }
        }
        let name = SourceName::from(format!("{USER_PREFIX}{name}"));
        layers.user_layer.roots.insert(name.clone());
        layers.dependencies.push((name, dependency));
    }
    let own = SourceName::from(crate::types::SourceOrigin::LocalPackage.to_string());
    layers.user_layer.pins = lock
        .dependencies
        .iter()
        .filter(|(name, _)| **name != own)
        .map(|(name, pin)| {
            (
                SourceName::from(format!("{USER_PREFIX}{name}")),
                pin.clone(),
            )
        })
        .collect();
    if user.local_source().is_dir() {
        layers.dependencies.push((
            SourceName::from(USER_SELF),
            path_dependency(&user.local_source(), user.root()),
        ));
    }
    Ok(())
}

/// The bundle of each host this sync knows, by provider: `--provided-root`, else the lock row an
/// earlier sync recorded while that directory exists. `mars provide`'s staging is not a bundle.
fn host_bundles(
    layout: &Layout,
    old_lock: &LockFile,
    host: Option<&HostRoot>,
) -> BTreeMap<String, (PathBuf, String)> {
    let staging = layout.store().join("provided");
    let mut bundles: BTreeMap<String, (PathBuf, String)> = BTreeMap::new();
    for (name, row) in &old_lock.dependencies {
        let (Some(provider), Some(path), Some(version)) = (
            name.as_str().strip_prefix(HOST_PREFIX),
            row.path.as_deref(),
            row.version.as_deref(),
        ) else {
            continue;
        };
        let dir = layout.root().join(path);
        if !dir.starts_with(&staging) && dir.is_dir() {
            bundles.insert(provider.to_string(), (dir, version.to_string()));
        }
    }
    if let Some(host) = host {
        let dir = std::path::absolute(&host.dir).unwrap_or_else(|_| host.dir.clone());
        let dir = dunce::canonicalize(&dir).unwrap_or(dir);
        bundles.insert(host.provider.clone(), (dir, host.version.clone()));
    }
    bundles
}

fn host_layer(
    layers: &mut Layers,
    layout: &Layout,
    bundles: BTreeMap<String, (PathBuf, String)>,
    host: Option<&HostRoot>,
) -> Result<(), MarsError> {
    if let Some(host) = host
        && !host.dir.is_dir()
    {
        return Err(crate::error::ConfigError::Invalid {
            message: format!("--provided-root {} is not a directory", host.dir.display()),
        }
        .into());
    }
    for (provider, (dir, version)) in bundles {
        layers.dependencies.push((
            SourceName::from(crate::provided::source_name(&provider)),
            path_dependency(&dir, layout.root()),
        ));
        layers.host_versions.insert(provider, version);
    }
    for (name, dependency) in crate::provided::staged_dependencies(layout)? {
        if !layers.dependencies.iter().any(|(held, _)| *held == name) {
            layers.dependencies.push((name, dependency));
        }
    }
    Ok(())
}
