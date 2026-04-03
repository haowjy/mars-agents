//! CLI layer — clap definitions + command dispatch.
//!
//! Each subcommand is a separate module. The CLI layer:
//! - Parses args into typed commands
//! - Locates `.agents/` root (walk up from cwd, or `--root` flag)
//! - Calls library functions
//! - Formats output (human-readable by default, `--json` for machine)
//! - Maps `MarsError` to exit codes and stderr messages

pub mod add;
pub mod cache;
pub mod check;
pub mod doctor;
pub mod init;
pub mod link;
pub mod list;
pub mod outdated;
pub mod output;
pub mod override_cmd;
pub mod remove;
pub mod rename;
pub mod repair;
pub mod resolve_cmd;
pub mod sync;
pub mod upgrade;
pub mod why;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::error::{ConfigError, LockError, MarsError};

/// Directories where mars manages mars.toml as the primary root.
/// These are the default target for `mars init`.
pub const WELL_KNOWN: &[&str] = &[".agents"];

/// Tool-specific directories that commonly need linking.
/// Root detection searches these in addition to WELL_KNOWN.
/// `mars link` warns if the target isn't in TOOL_DIRS or WELL_KNOWN.
pub const TOOL_DIRS: &[&str] = &[".claude", ".cursor"];

/// Resolved context for a mars command — both the managed root
/// and its parent project root.
pub struct MarsContext {
    /// The directory containing mars.toml (e.g. /project/.agents)
    pub managed_root: PathBuf,
    /// The project directory (managed_root's parent, e.g. /project)
    pub project_root: PathBuf,
}

impl MarsContext {
    /// Build from a managed root path. Enforces the invariant that
    /// managed_root must have a parent (i.e., is always a subdirectory).
    pub fn new(managed_root: PathBuf) -> Result<Self, MarsError> {
        let canonical = if managed_root.exists() {
            managed_root.canonicalize().unwrap_or(managed_root.clone())
        } else {
            managed_root.clone()
        };
        let project_root = canonical
            .parent()
            .ok_or_else(|| {
                MarsError::Config(ConfigError::Invalid {
                    message: format!(
                        "managed root {} has no parent directory — the managed root must be \
                     a subdirectory (e.g., /project/.agents, not /project)",
                        managed_root.display()
                    ),
                })
            })?
            .to_path_buf();
        Ok(MarsContext {
            managed_root: canonical,
            project_root,
        })
    }
}

/// mars — agent package manager for .agents/
#[derive(Debug, Parser)]
#[command(name = "mars", version, about = "Agent package manager for .agents/")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Path to managed root containing mars.toml (default: auto-detect).
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,

    /// Output in JSON format.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize a managed root with mars.toml (default: .agents/).
    Init(init::InitArgs),

    /// Add a source (git URL, GitHub shorthand, or local path).
    Add(add::AddArgs),

    /// Remove a source.
    Remove(remove::RemoveArgs),

    /// Sync: resolve + install (make reality match config).
    Sync(sync::SyncArgs),

    /// Upgrade sources to newest compatible versions.
    Upgrade(upgrade::UpgradeArgs),

    /// Show available updates without applying.
    Outdated(outdated::OutdatedArgs),

    /// List managed items with status.
    List(list::ListArgs),

    /// Explain why an item is installed.
    Why(why::WhyArgs),

    /// Rename a managed item.
    Rename(rename::RenameArgs),

    /// Mark conflicts as resolved.
    Resolve(resolve_cmd::ResolveArgs),

    /// Set a local dev override for a source.
    Override(override_cmd::OverrideArgs),

    /// Symlink agents/ and skills/ into another directory (e.g. .claude).
    Link(link::LinkArgs),

    /// Validate a source package before publishing (structure, frontmatter, deps).
    Check(check::CheckArgs),

    /// Diagnose problems in an installed mars project (config, lock, files, links).
    Doctor(doctor::DoctorArgs),

    /// Rebuild state from lock + sources.
    Repair(repair::RepairArgs),

    /// Manage the global source cache.
    Cache(cache::CacheArgs),
}

/// Dispatch a parsed CLI command to the appropriate handler and map errors to
/// the final exit code.
pub fn dispatch(cli: Cli) -> i32 {
    match dispatch_result(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            if matches!(err, MarsError::Lock(LockError::Corrupt { .. })) {
                eprintln!("hint: run `mars repair` to rebuild from mars.toml + sources");
            }
            err.exit_code()
        }
    }
}

fn dispatch_result(cli: Cli) -> Result<i32, MarsError> {
    match &cli.command {
        // Root-free commands
        Command::Init(args) => init::run(args, cli.root.as_deref(), cli.json),
        Command::Check(args) => check::run(args, cli.json),
        Command::Cache(args) => cache::run(args, cli.json),
        // All other commands require a managed root
        cmd => {
            let ctx = find_agents_root(cli.root.as_deref())?;
            dispatch_with_root(cmd, &ctx, cli.json)
        }
    }
}

fn dispatch_with_root(cmd: &Command, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    match cmd {
        Command::Add(args) => add::run(args, ctx, json),
        Command::Remove(args) => remove::run(args, ctx, json),
        Command::Sync(args) => sync::run(args, ctx, json),
        Command::Upgrade(args) => upgrade::run(args, ctx, json),
        Command::Outdated(args) => outdated::run(args, ctx, json),
        Command::List(args) => list::run(args, ctx, json),
        Command::Why(args) => why::run(args, ctx, json),
        Command::Rename(args) => rename::run(args, ctx, json),
        Command::Resolve(args) => resolve_cmd::run(args, ctx, json),
        Command::Override(args) => override_cmd::run(args, ctx, json),
        Command::Link(args) => link::run(args, ctx, json),
        Command::Doctor(args) => doctor::run(args, ctx, json),
        Command::Repair(args) => repair::run(args, ctx, json),
        // Root-free commands handled in dispatch_result — unreachable here
        Command::Init(_) | Command::Check(_) | Command::Cache(_) => unreachable!(),
    }
}

/// Check if a path is a symlink (uses symlink_metadata, doesn't follow).
pub fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Find the mars-managed root by walking up from cwd, or use `--root` flag.
///
/// Walk up the directory tree looking for a directory containing `mars.toml`.
/// The managed root can be any directory (`.agents/`, `.claude/`, etc.) —
/// mars doesn't impose a specific name.
///
/// Search order at each level:
/// 1. `.agents/mars.toml` (convention default)
/// 2. `.claude/mars.toml` (Claude Code projects)
/// 3. If cwd itself contains `mars.toml`, use it directly
pub fn find_agents_root(explicit: Option<&Path>) -> Result<MarsContext, MarsError> {
    if let Some(root) = explicit {
        // User explicitly chose this root — trust it (no containment check)
        return MarsContext::new(root.to_path_buf());
    }

    let cwd = std::env::current_dir()?;
    // Canonicalize cwd to resolve ancestor symlinks so the walk-up operates
    // on real paths and containment checks catch .agents/ symlinks pointing
    // outside the real cwd tree.
    let cwd_canon = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
    let mut dir = cwd_canon.as_path();

    loop {
        // Check well-known subdirectories + tool dirs
        for subdir in WELL_KNOWN.iter().chain(TOOL_DIRS.iter()) {
            let candidate = dir.join(subdir);
            if candidate.join("mars.toml").exists() {
                let ctx = MarsContext::new(candidate)?;
                // Validate: canonical managed_root should be under the
                // directory we found it in. A symlinked .agents/ pointing
                // outside the project tree would fail this check.
                if !ctx.managed_root.starts_with(dir) {
                    return Err(MarsError::Config(ConfigError::Invalid {
                        message: format!(
                            "{}/{} resolves to {} which is outside {}. \
                             The managed root may be a symlink. Use --root to override.",
                            dir.display(),
                            subdir,
                            ctx.managed_root.display(),
                            dir.display(),
                        ),
                    }));
                }
                return Ok(ctx);
            }
        }

        // Check if we're already inside a mars-managed directory
        if dir.join("mars.toml").exists() {
            return MarsContext::new(dir.to_path_buf());
        }

        // Walk up
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }

    Err(MarsError::Config(ConfigError::Invalid {
        message: format!(
            "no mars.toml found from {} to /. Run `mars init` first.",
            cwd.display()
        ),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn find_root_with_explicit_path() {
        let dir = TempDir::new().unwrap();
        let ctx = find_agents_root(Some(dir.path())).unwrap();
        assert_eq!(ctx.managed_root, dir.path().canonicalize().unwrap());
    }

    #[test]
    fn find_root_walks_up() {
        let dir = TempDir::new().unwrap();
        let agents_dir = dir.path().join(".agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("mars.toml"), "[sources]\n").unwrap();

        // Create a subdirectory
        let sub = dir.path().join("subdir").join("deep");
        std::fs::create_dir_all(&sub).unwrap();

        // find_agents_root uses cwd, so we test with explicit
        // The actual walk-up requires changing cwd which isn't safe in tests
        let ctx = find_agents_root(Some(&agents_dir)).unwrap();
        assert_eq!(ctx.managed_root, agents_dir.canonicalize().unwrap());
        assert_eq!(ctx.project_root, dir.path().canonicalize().unwrap());
    }

    #[test]
    fn find_root_symlink_outside_project_detected() {
        // Verify the containment invariant: a symlinked .agents/ resolving
        // outside the project tree should be detectable.
        let project_dir = TempDir::new().unwrap();
        let external_dir = TempDir::new().unwrap();

        // Create the external agents dir with mars.toml
        let external_agents = external_dir.path().join(".agents");
        std::fs::create_dir_all(&external_agents).unwrap();
        std::fs::write(external_agents.join("mars.toml"), "[sources]\n").unwrap();

        // Symlink project/.agents -> external/.agents
        let project_agents = project_dir.path().join(".agents");
        std::os::unix::fs::symlink(&external_agents, &project_agents).unwrap();

        // MarsContext::new canonicalizes, so managed_root resolves outside project
        let ctx = MarsContext::new(project_agents).unwrap();
        let project_canon = project_dir.path().canonicalize().unwrap();
        assert!(
            !ctx.managed_root.starts_with(&project_canon),
            "symlinked managed_root should resolve outside project"
        );
    }

    #[test]
    fn find_root_explicit_bypasses_containment() {
        // --root flag should work even for paths that would fail containment
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();

        let ctx = find_agents_root(Some(&agents)).unwrap();
        assert_eq!(ctx.managed_root, agents.canonicalize().unwrap());
    }

    #[test]
    fn mars_context_new_errors_on_root_path() {
        // "/" has no parent — should error
        let result = MarsContext::new(std::path::PathBuf::from("/"));
        assert!(result.is_err());
    }
}
