//! CLI layer — clap definitions + command dispatch.
//!
//! Each subcommand is a separate module. The CLI layer:
//! - Parses args into typed commands
//! - Locates project root (walk up from cwd, or `--root` flag)
//! - Calls library functions
//! - Formats output (human-readable by default, `--json` for machine)
//! - Maps `MarsError` to exit codes and stderr messages

pub mod add;
pub mod adopt;
pub mod agents;
pub mod build;
pub mod cache;
pub mod check;
pub(crate) mod conflict_markers;
pub mod doctor;
pub mod export;
pub mod init;
pub mod link;
pub mod list;
pub mod models;
mod models_common;
mod models_inventory;
mod models_prompting;
pub mod outdated;
pub mod output;
pub mod override_cmd;
pub mod provide;
pub mod remove;
pub mod rename;
pub mod repair;
pub mod resolve_cmd;
pub mod skills;
pub mod sync;
pub mod target;
pub mod unlink;
pub mod upgrade;
pub mod validate;
pub mod version;
pub mod why;

use std::path::{Component, Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::error::{ConfigError, LockError, MarsError};
pub use crate::types::MarsContext;
use crate::types::managed_cmd;

pub const WELL_KNOWN: &[&str] = &[".agents"];

/// `.agents` or a harness's target directory.
pub fn is_recognized_target(name: &str) -> bool {
    WELL_KNOWN.contains(&name) || crate::harness::registry::harness_for_target(name).is_some()
}

impl MarsContext {
    pub fn new(project_root: PathBuf) -> Result<Self, MarsError> {
        let project_canon = if project_root.exists() {
            dunce::canonicalize(&project_root).unwrap_or(project_root.clone())
        } else {
            project_root.clone()
        };

        // Mars deletes and rewrites inside `.mars/`, so it must not be a symlink out of the
        // project. A target may be a symlink (a dotfiles repo, say) and is only written where
        // Mars recorded an item, so that is the user's layout, not a widening of the workspace.
        let layout = crate::layout::Layout::project(project_canon);
        let mars_dir = layout.store();
        if let Ok(resolved) = dunce::canonicalize(mars_dir)
            && !resolved.starts_with(layout.root())
        {
            return Err(MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "{} resolves to {} which is outside {}. Use --root to override.",
                    mars_dir.display(),
                    resolved.display(),
                    layout.root().display(),
                ),
            }));
        }

        Ok(MarsContext {
            layout,
            meridian_managed: crate::types::meridian_managed_from_env(),
        })
    }

    /// The user level, at the folder `platform::home` resolves. Nothing is created: an absent
    /// folder is an empty user level until a command writes to it.
    pub fn user() -> Result<Self, MarsError> {
        let layout = crate::layout::Layout::user_level().ok_or_else(|| {
            MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "cannot find the user level: the home directory is not an absolute path; set {}",
                    crate::platform::home::HOME_VARIABLE
                ),
            })
        })?;
        Ok(MarsContext {
            layout,
            meridian_managed: crate::types::meridian_managed_from_env(),
        })
    }
}

/// mars — agent package manager for agent and skill packages.
#[derive(Debug, Parser)]
#[command(name = "mars", version, about = "Agent package manager")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Path to project root containing mars.toml (default: auto-detect).
    #[arg(long, global = true, conflicts_with = "user")]
    pub root: Option<PathBuf>,

    /// Use your user level (`~/.mars`, or `MARS_HOME`) instead of a project.
    #[arg(long, global = true)]
    pub user: bool,

    /// Output in JSON format.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize project-level mars.toml and .mars/ compiled store.
    Init(init::InitArgs),

    /// Add a dependency (git URL, GitHub shorthand, or local path).
    Add(add::AddArgs),

    /// Adopt an unmanaged target item into `.mars-src/`, then sync.
    Adopt(adopt::AdoptArgs),

    /// Remove a dependency.
    Remove(remove::RemoveArgs),

    /// Sync: resolve + install (make reality match config).
    Sync(sync::SyncArgs),

    /// Upgrade dependency versions to newest compatible releases.
    Upgrade(upgrade::UpgradeArgs),

    /// Show available updates without applying.
    Outdated(outdated::OutdatedArgs),

    /// Bump package version in mars.toml, commit, and tag (--no-tag: leave the tag to CI).
    Version(version::VersionArgs),

    /// List managed items with status.
    List(list::ListArgs),

    /// Explain why an item is installed.
    Why(why::WhyArgs),

    /// Rename a managed item.
    Rename(rename::RenameArgs),

    /// Deprecated: mark conflicts as resolved (removed in the next minor).
    Resolve(resolve_cmd::ResolveArgs),

    /// Set a local dev override for a source.
    Override(override_cmd::OverrideArgs),

    /// Add a managed target directory (e.g. .claude).
    Link(link::LinkArgs),

    /// Install a host program's own skills into `.mars/skills/`.
    Provide(provide::ProvideArgs),

    /// Remove a managed target directory.
    Unlink(unlink::UnlinkArgs),

    /// Dry-run the compiler pipeline and report diagnostics without writing.
    Validate(validate::ValidateArgs),

    /// Export the compile plan as JSON (dry-run, no writes).
    Export(export::ExportArgs),

    /// Validate a source package before publishing (structure, frontmatter, deps).
    Check(check::CheckArgs),

    /// Diagnose problems in an installed mars project (config, lock, files, targets).
    Doctor(doctor::DoctorArgs),

    /// Rebuild state from lock + sources.
    Repair(repair::RepairArgs),

    /// Manage the global source cache.
    Cache(cache::CacheArgs),

    /// Manage aliases and the last-known-good catalog (24h refresh-after by default).
    Models(models::ModelsArgs),

    /// Build derived artifacts from static project state.
    Build(build::BuildArgs),

    /// List and inspect agents.
    Agents(agents::AgentsArgs),

    /// List and inspect skills.
    Skills(skills::SkillsArgs),

    /// Run a translated hook; written into a harness's hook file by `mars sync`.
    #[command(hide = true)]
    Hook(crate::hook_run::HookArgs),
}

pub fn dispatch(cli: Cli) -> i32 {
    let json = cli.json;
    output::set_json_mode(json);
    let code = match dispatch_result(cli) {
        Ok(code) => code,
        Err(err) => {
            let cause = err.root_cause();
            if json {
                let (code, message, report) = match cause {
                    MarsError::Selection {
                        code,
                        message,
                        report,
                    } => (*code, message.clone(), Some(report)),
                    MarsError::Config(ConfigError::ProjectRootNotFound { .. }) => {
                        ("project_not_found", cause.to_string(), None)
                    }
                    MarsError::Config(_) => ("invalid_config", cause.to_string(), None),
                    MarsError::NotFound { code, message } => (*code, message.clone(), None),
                    _ => ("command_failed", cause.to_string(), None),
                };
                let mut payload =
                    serde_json::json!({ "error": { "code": code, "message": message } });
                if !err.diagnostics().is_empty() {
                    payload["diagnostics"] = serde_json::json!(err.diagnostics());
                }
                if let Some(report) = report {
                    payload["route_trace"] = serde_json::json!(report);
                }
                output::print_json(&payload);
            } else {
                output::print_notices();
                output::print_diagnostics(err.diagnostics(), false);
                eprintln!("error: {cause}");
            }
            if !json && matches!(cause, MarsError::Lock(LockError::Corrupt { .. })) {
                eprintln!(
                    "hint: run `{}` to rebuild from mars.toml + dependencies",
                    crate::layout::root_cmd("repair")
                );
            }
            err.exit_code()
        }
    };
    output::print_notices();
    code
}

fn dispatch_result(cli: Cli) -> Result<i32, MarsError> {
    let shape = Shape::of(&cli.command);
    if cli.user
        && let Some(reason) = &shape.not_at_user_level
    {
        return Err(refused_at_user_level(reason));
    }
    let Some(opens) = &shape.opens else {
        return dispatch_without_root(&cli);
    };
    let ctx = open_root(&cli, opens)?;
    // A walk-up from inside the user folder reaches the user level without `--user`.
    if !cli.user
        && ctx.layout.kind() == crate::layout::RootKind::User
        && let Some(reason) = &shape.not_at_user_level
    {
        return Err(MarsError::Config(ConfigError::Invalid {
            message: format!(
                "{} is your user level's folder, where this command does not run: {reason}",
                ctx.layout.root().display()
            ),
        }));
    }
    if fallback_root(&cli.command).is_some() {
        output::warn(
            "fallback-root-deprecated",
            format!(
                "`--fallback-root` is deprecated and goes in the next minor release: install those items at your user level (`{}`), which a lookup outside a project reads",
                managed_cmd("mars --user add <source>")
            ),
        );
    }
    dispatch_with_root(&cli, &ctx)
}

fn refused_at_user_level(reason: &str) -> MarsError {
    MarsError::Config(ConfigError::Invalid {
        message: format!("`--user` cannot be used here: {reason}"),
    })
}

/// The root a command runs against, the one place a root is opened: a write to the folder the
/// user level and the home project share is refused here, whichever command asks.
fn open_root(cli: &Cli, opens: &Opens) -> Result<MarsContext, MarsError> {
    let in_place = || MarsContext::new(cli.root.clone().unwrap_or(std::env::current_dir()?));
    let ctx = if cli.user {
        MarsContext::user()?
    } else {
        match find_agents_root(cli.root.as_deref()) {
            Ok(ctx) => ctx,
            Err(err) if is_project_not_found(&err) => match opens.without_project {
                WithoutProject::UserLevel => match MarsContext::user() {
                    Ok(user) => user,
                    // No user level can be located: the lookup runs in place, where no
                    // project is, as an ad-hoc launch bundle did before the user level.
                    Err(_) => in_place()?,
                },
                WithoutProject::InitsProject => {
                    let initialized = init::initialize_project(cli.root.as_deref(), None)?;
                    if !cli.json {
                        output::print_info(&format!(
                            "auto-initialized {} with mars.toml",
                            initialized.project_root.display()
                        ));
                    }
                    MarsContext::new(initialized.project_root.clone())?
                }
                WithoutProject::InPlace => in_place()?,
                WithoutProject::Fails => return Err(err),
            },
            Err(err) => return Err(err),
        }
    };
    crate::layout::set_run_root(&ctx.layout);
    if opens.effect == Effect::Writes {
        refuse_shared_folder_write(&ctx.layout)?;
    }
    Ok(ctx)
}

/// The user level and the home project share `~/.mars` until the home project is moved: a write
/// to either could take the other's files as its own. The user level is refused every write
/// while the home project is there, and the home project every write but `unlink`.
fn refuse_shared_folder_write(layout: &crate::layout::Layout) -> Result<(), MarsError> {
    use crate::home_project::HomeProject;
    let refusal = if layout.kind() == crate::layout::RootKind::User {
        HomeProject::in_user_folder().map(|home| home.blocks_user_level())
    } else if crate::home_project::is_home_project(layout) {
        HomeProject::find().map(|home| home.refuses_writes())
    } else {
        None
    };
    match refusal {
        Some(message) => Err(MarsError::Config(ConfigError::Invalid { message })),
        None => Ok(()),
    }
}

fn is_project_not_found(err: &MarsError) -> bool {
    matches!(
        err,
        MarsError::Config(ConfigError::ProjectRootNotFound { .. })
    )
}

/// What a command does with a root: the one place each command is placed, so a new one must be.
struct Shape {
    /// How it uses the root it opens, or `None` when it opens none: it is dispatched before a
    /// root is looked for, so nothing it does can reach the shared-folder guard. `init` makes a
    /// project at a path and refuses the home and user folders itself; `check` reads the
    /// package at a path.
    opens: Option<Opens>,
    /// Why the command has no meaning at the user level, refusing `--user`.
    not_at_user_level: Option<String>,
}

/// How a command uses the root it opens.
struct Opens {
    effect: Effect,
    without_project: WithoutProject,
}

/// What a command does to the root it runs against.
#[derive(PartialEq, Eq)]
enum Effect {
    /// Writes no root file.
    Reads,
    /// Removes a target's outputs and records, and nothing its lock does not record: what the
    /// home project is opened for.
    Unlinks,
    /// Writes the root's manifest, lock, own items or store.
    Writes,
}

/// Where a command runs when no project is found.
enum WithoutProject {
    /// It needs a project, or `--user` to say the user level is meant.
    Fails,
    /// It reads the user level: a lookup of installed items.
    UserLevel,
    /// It initializes a project where it runs.
    InitsProject,
    /// It runs in place: a plugin build of an explicit package path needs no project around it.
    InPlace,
}

impl Shape {
    fn of(cmd: &Command) -> Self {
        use Effect::{Reads, Unlinks, Writes};
        use WithoutProject::{Fails, InPlace, InitsProject, UserLevel};
        let authors = || Some("it authors a package, which the user level is not".to_string());
        let opens = |effect, without_project| {
            Some(Opens {
                effect,
                without_project,
            })
        };
        let (opens, not_at_user_level) = match cmd {
            Command::Init(_) => (None, Some(init::user_level_needs_no_init())),
            Command::Check(_) => (
                None,
                Some("it checks a package you are authoring, which the user level is not".into()),
            ),
            Command::Cache(_) | Command::Hook(_) => (None, None),
            Command::Provide(_) => (
                opens(Writes, Fails),
                Some("a host's content is provided to projects, not the user level".into()),
            ),
            Command::Link(_) => (opens(Writes, InitsProject), no_targets()),
            Command::Unlink(_) => (opens(Unlinks, Fails), no_targets()),
            Command::Add(_) => (opens(Writes, InitsProject), None),
            Command::Version(_) => (opens(Writes, Fails), authors()),
            Command::Adopt(_)
            | Command::Remove(_)
            | Command::Sync(_)
            | Command::Upgrade(_)
            | Command::Rename(_)
            | Command::Resolve(_)
            | Command::Override(_)
            | Command::Repair(_) => (opens(Writes, Fails), None),
            Command::Outdated(_)
            | Command::Validate(_)
            | Command::Export(_)
            | Command::Doctor(_) => (opens(Reads, Fails), None),
            Command::List(_) | Command::Why(_) | Command::Agents(_) | Command::Skills(_) => {
                (opens(Reads, UserLevel), None)
            }
            Command::Models(args) => match args.command {
                // A catalog refresh writes a shared cache, not a root's.
                models::ModelsCommand::RefreshCatalog(_) => (None, None),
                // It writes `mars.toml`.
                models::ModelsCommand::Alias(_) => (opens(Writes, Fails), None),
                _ => (opens(Reads, UserLevel), None),
            },
            Command::Build(args) => match &args.command {
                build::BuildCommand::LaunchBundle(_) => (opens(Reads, UserLevel), None),
                build::BuildCommand::Plugin(plugin) => (
                    opens(
                        Writes,
                        if plugin.path.is_some() {
                            InPlace
                        } else {
                            Fails
                        },
                    ),
                    authors(),
                ),
            },
        };
        Self {
            opens,
            not_at_user_level,
        }
    }
}

fn no_targets() -> Option<String> {
    Some("the user level has no targets; its items are installed by each project's sync".into())
}

fn fallback_root(cmd: &Command) -> Option<&Path> {
    match cmd {
        Command::Agents(args) => args.fallback_root.as_deref(),
        Command::Skills(args) => args.fallback_root.as_deref(),
        Command::Build(build::BuildArgs {
            command: build::BuildCommand::LaunchBundle(args),
        }) => args.fallback_root.as_deref(),
        _ => None,
    }
}

fn dispatch_with_root(cli: &Cli, ctx: &MarsContext) -> Result<i32, MarsError> {
    let json = cli.json;
    let explicit_root = cli.root.is_some().then_some(ctx.layout.root());
    match &cli.command {
        Command::Provide(args) => provide::run(args, ctx, cli.root.as_deref(), json),
        Command::Validate(args) => validate::run(args, ctx, json),
        Command::Export(args) => export::run(args, ctx, json),
        Command::Add(args) => add::run(args, ctx, explicit_root, json),
        Command::Adopt(args) => adopt::run(args, ctx, explicit_root, json),
        Command::Remove(args) => remove::run(args, ctx, json),
        Command::Sync(args) => sync::run(args, ctx, json),
        Command::Upgrade(args) => upgrade::run(args, ctx, json),
        Command::Outdated(args) => outdated::run(args, ctx, json),
        Command::Version(args) => version::run(args, ctx, json),
        Command::List(args) => list::run(args, ctx, json),
        Command::Why(args) => why::run(args, ctx, json),
        Command::Rename(args) => rename::run(args, ctx, json),
        Command::Resolve(args) => resolve_cmd::run(args, ctx, json),
        Command::Override(args) => override_cmd::run(args, ctx, explicit_root, json),
        Command::Link(args) => link::run(args, ctx, json),
        Command::Unlink(args) => unlink::run(args, ctx, json),
        Command::Doctor(args) => doctor::run(args, ctx, json),
        Command::Repair(args) => repair::run(args, ctx, json),
        Command::Models(args) => models::run(args, ctx, json),
        Command::Build(args) => build::run(args, ctx, explicit_root, json),
        Command::Agents(args) => agents::run(args, ctx, json),
        Command::Skills(args) => skills::run(args, ctx, json),
        Command::Init(_) | Command::Check(_) | Command::Cache(_) | Command::Hook(_) => {
            unreachable!("`Shape::of` opens no root for it")
        }
    }
}

/// The commands `Shape::of` opens no root for.
fn dispatch_without_root(cli: &Cli) -> Result<i32, MarsError> {
    match &cli.command {
        Command::Init(args) => init::run(args, cli.root.as_deref(), cli.json),
        Command::Check(args) => check::run(args, cli.root.as_deref(), cli.json),
        Command::Cache(args) => cache::run(args, cli.json),
        Command::Hook(args) => Ok(crate::hook_run::run(args)),
        Command::Models(models::ModelsArgs {
            command: models::ModelsCommand::RefreshCatalog(args),
        }) => models::run_refresh_catalog(args),
        _ => unreachable!("`Shape::of` opens a root for it"),
    }
}

/// Absolute form of a path typed on the command line. A relative path resolves against the
/// working directory when it exists there, since that is where the user typed it. Otherwise,
/// with an explicit `--root`, it resolves against the project root that `--root` led to
/// (`explicit_root`; `--root` is a walk-up start, so it can name a subdirectory). A host such as
/// `meridian mars` always appends `--root`, so the working directory must win when it can.
pub(crate) fn resolve_cli_path(
    path: &Path,
    explicit_root: Option<&Path>,
) -> Result<PathBuf, MarsError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let from_cwd = std::env::current_dir()?.join(path);
    match explicit_root {
        Some(root) if from_cwd.symlink_metadata().is_err() => Ok(root.join(path)),
        _ => Ok(from_cwd),
    }
}

/// A path typed on the command line, in the form a manifest in `manifest_dir` stores it. A
/// relative path resolves as [`resolve_cli_path`] says and is rewritten relative to
/// `manifest_dir`, where the manifest's relative paths resolve; a rooted path stays as typed.
/// The rewrite uses `/`, which every platform reads, so a committed manifest works on all of them.
pub(crate) fn manifest_cli_path(
    path: &Path,
    explicit_root: Option<&Path>,
    manifest_dir: &Path,
) -> Result<PathBuf, MarsError> {
    if path.has_root() || matches!(path.components().next(), Some(Component::Prefix(_))) {
        return Ok(path.to_path_buf());
    }
    // Both sides are compared in canonical form (case, junctions, `subst` drives, UNC prefixes
    // on Windows). The typed path's last component stays as typed, so a symlinked package
    // directory is stored by its link.
    let typed = crate::fs::lexical_absolute(&resolve_cli_path(path, explicit_root)?)?;
    let target = match (typed.parent(), typed.file_name()) {
        (Some(parent), Some(name)) => crate::fs::canonicalize_existing_prefix(parent)?.join(name),
        _ => crate::fs::canonicalize_existing_prefix(&typed)?,
    };
    let base = crate::fs::canonicalize_existing_prefix(manifest_dir)?;
    let target: Vec<_> = target.components().collect();
    let base: Vec<_> = base.components().collect();
    let common = target
        .iter()
        .zip(&base)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        // Another drive: no relative path reaches it.
        return Ok(target.iter().collect());
    }
    let mut parts: Vec<String> = vec![if common == base.len() { "." } else { ".." }.to_string()];
    parts.extend((common + 1..base.len()).map(|_| "..".to_string()));
    parts.extend(
        target[common..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().into_owned()),
    );
    Ok(PathBuf::from(parts.join("/")))
}

pub fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Find mars project root by walking up from start path to filesystem root.
///
/// For context commands (`add`, `sync`, etc.), this walks from the start path
/// (cwd or `--root`) until it finds a `mars.toml`. Walk-up continues to filesystem
/// root — git boundaries do not stop the walk.
///
/// If `--root` is provided, it sets the walk-up start path, not a direct target.
///
/// The home directory is never a project by walk-up: its `mars.toml` was the user level before
/// Mars had one. For one minor release (`.context/TODO`) a `--root` naming home exactly still
/// opens it (its `mars.toml` or, once that is gone, its `mars.lock`: `crate::home_project`) to
/// read it and to `unlink` its targets; each run that meets one ends with how to move it.
pub fn find_agents_root(explicit: Option<&Path>) -> Result<MarsContext, MarsError> {
    let start = if let Some(root) = explicit {
        if let Some(basename) = root.file_name().and_then(|f| f.to_str())
            && is_recognized_target(basename)
        {
            return Err(MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "`--root {basename}` looks like a managed output directory.\n  \
                     --root takes the project root (containing mars.toml), not the output directory.\n  \
                     Try: mars init  (auto-detects project root)\n  \
                     Or:  mars init {basename}  (specify output directory name)"
                ),
            }));
        }

        root.to_path_buf()
    } else {
        std::env::current_dir()?
    };

    let user = crate::layout::Layout::user_level();
    find_agents_root_from(
        &start,
        explicit.is_some(),
        crate::platform::home::canonical_home().as_deref(),
        user.as_ref().map(|user| user.root()),
    )
}

/// `user_root` is the user level's folder: its `mars.toml` is the user level's, so a walk-up
/// that reaches it is at the user level, not in a project.
fn find_agents_root_from(
    start: &Path,
    explicit: bool,
    home: Option<&Path>,
    user_root: Option<&Path>,
) -> Result<MarsContext, MarsError> {
    let start_canon = dunce::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let mut dir = start_canon.as_path();

    loop {
        if Some(dir) == user_root {
            return MarsContext::user();
        }
        if Some(dir) == home {
            if crate::home_project::holds_project(dir) {
                crate::home_project::raise_notice();
                if explicit && dir == start_canon {
                    return MarsContext::new(dir.to_path_buf());
                }
            }
        } else if crate::layout::Layout::project(dir).manifest().exists() {
            return MarsContext::new(dir.to_path_buf());
        }

        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }

    Err(MarsError::Config(ConfigError::ProjectRootNotFound {
        start: start.to_path_buf(),
        refusal: init::no_project_in(&start_canon, home, user_root),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    #[test]
    fn package_manifest_without_dependencies_is_valid_project_root() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[package]\nschema = 1\nname = \"pkg\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let ctx = find_agents_root(Some(dir.path())).unwrap();
        assert_eq!(ctx.layout.root(), dunce::canonicalize(dir.path()).unwrap());
    }
    #[cfg(unix)]
    #[test]
    fn context_rejects_symlinked_mars_dir_outside_project() {
        let project_dir = TempDir::new().unwrap();
        let external_dir = TempDir::new().unwrap();
        std::fs::write(project_dir.path().join("mars.toml"), "[dependencies]\n").unwrap();

        let external_mars = external_dir.path().join(".mars");
        std::fs::create_dir_all(&external_mars).unwrap();
        std::os::unix::fs::symlink(&external_mars, project_dir.path().join(".mars")).unwrap();

        let result = MarsContext::new(project_dir.path().to_path_buf());
        assert!(result.is_err());
    }

    #[test]
    fn init_rejects_root_that_looks_like_managed_dir() {
        let result = find_agents_root(Some(Path::new(".agents")));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("managed output directory"),
            "should reject .agents as --root: {err}"
        );
    }

    #[test]
    fn walk_up_crosses_git_boundary_to_find_config() {
        let dir = TempDir::new().unwrap();
        let outer = dir.path().join("outer");
        std::fs::create_dir_all(outer.join(".agents")).unwrap();
        std::fs::write(outer.join("mars.toml"), "[dependencies]\n").unwrap();

        let inner = outer.join("inner");
        std::fs::create_dir_all(inner.join(".git")).unwrap();

        let ctx = find_agents_root_from(&inner, false, None, None).unwrap();
        assert_eq!(
            ctx.layout.root(),
            dunce::canonicalize(&outer).unwrap(),
            "should find outer config even when inner has .git"
        );
    }

    #[test]
    fn walk_up_finds_config_in_ancestor() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(root.join(".agents")).unwrap();
        std::fs::write(root.join("mars.toml"), "[dependencies]\n").unwrap();

        let subdir = root.join("src").join("lib");
        std::fs::create_dir_all(&subdir).unwrap();

        let ctx = find_agents_root_from(&subdir, false, None, None).unwrap();
        assert_eq!(ctx.layout.root(), dunce::canonicalize(&root).unwrap());
    }

    #[test]
    fn walk_up_prefers_nearest_mars_toml() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("parent");
        std::fs::create_dir_all(parent.join(".agents")).unwrap();
        std::fs::write(parent.join("mars.toml"), "[dependencies]\n").unwrap();

        let child = parent.join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(
            child.join("mars.toml"),
            "[package]\nschema = 1\nname = \"pkg\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let ctx = find_agents_root_from(&child, false, None, None).unwrap();
        assert_eq!(ctx.layout.root(), dunce::canonicalize(&child).unwrap());
    }

    #[test]
    fn walk_up_from_deep_subdirectory() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join(".agents")).unwrap();
        std::fs::write(root.join("mars.toml"), "[dependencies]\n").unwrap();

        let deep = root.join("src").join("foo").join("bar");
        std::fs::create_dir_all(&deep).unwrap();

        let ctx = find_agents_root_from(&deep, false, None, None).unwrap();
        assert_eq!(ctx.layout.root(), dunce::canonicalize(&root).unwrap());
    }

    #[test]
    fn walk_up_crosses_submodule_boundary() {
        let dir = TempDir::new().unwrap();
        let outer = dir.path().join("outer");
        std::fs::create_dir_all(outer.join(".agents")).unwrap();
        std::fs::write(outer.join("mars.toml"), "[dependencies]\n").unwrap();

        let submodule = outer.join("submodule");
        std::fs::create_dir_all(&submodule).unwrap();
        std::fs::write(
            submodule.join(".git"),
            "gitdir: ../../.git/modules/submodule\n",
        )
        .unwrap();

        let ctx = find_agents_root_from(&submodule, false, None, None).unwrap();
        assert_eq!(
            ctx.layout.root(),
            dunce::canonicalize(&outer).unwrap(),
            "should find outer config through submodule .git file boundary"
        );
    }

    #[test]
    fn walk_up_errors_when_no_config_found() {
        let dir = TempDir::new().unwrap();
        let deep = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).unwrap();

        let result = find_agents_root_from(&deep, false, None, None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no mars.toml found"),
            "should report no config found: {err}"
        );
        assert!(
            err.contains("filesystem root"),
            "should mention filesystem root: {err}"
        );
    }

    #[test]
    fn walk_up_with_root_flag_starts_from_specified_path() {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join(".agents")).unwrap();
        std::fs::write(project.join("mars.toml"), "[dependencies]\n").unwrap();

        let subdir = project.join("src");
        std::fs::create_dir_all(&subdir).unwrap();

        let ctx = find_agents_root(Some(&subdir)).unwrap();
        assert_eq!(ctx.layout.root(), dunce::canonicalize(&project).unwrap());
    }
}
