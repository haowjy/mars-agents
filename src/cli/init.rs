//! `mars init [path]` — scaffold a mars-managed directory with `agents.toml`.

use std::path::{Path, PathBuf};

use crate::config::{Config, Settings};
use crate::error::MarsError;

use super::output;

/// Arguments for `mars init`.
#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Target directory to initialize (default: .agents/ in cwd).
    pub path: Option<PathBuf>,
}

/// Run `mars init`.
pub fn run(args: &InitArgs, explicit_root: Option<&Path>, json: bool) -> Result<i32, MarsError> {
    let base = match (&args.path, explicit_root) {
        (Some(path), _) => resolve_base(path)?,
        (None, Some(root)) => resolve_base(root)?,
        (None, None) => std::env::current_dir()?,
    };

    let agents_dir = if explicit_root.is_some() {
        // --root flag: use it directly
        base.clone()
    } else if let Some(ref path) = args.path {
        // Explicit path arg: if it looks like a target dir (.claude, .cursor),
        // use it directly. Otherwise treat it as a project root and append .agents/
        let path_str = path.to_string_lossy();
        if path_str.starts_with('.') || base.join("agents.toml").exists() {
            base.clone()
        } else {
            base.join(".agents")
        }
    } else if base.join("agents.toml").exists() {
        // Already inside a mars-managed directory
        base.clone()
    } else {
        // Default: create .agents/ in cwd
        base.join(".agents")
    };

    let config_path = agents_dir.join("agents.toml");
    if config_path.exists() {
        return Err(MarsError::Source {
            source_name: "init".to_string(),
            message: format!(
                "agents.toml already exists at {}. Use `mars sync` instead.",
                config_path.display()
            ),
        });
    }

    // Create directories
    std::fs::create_dir_all(&agents_dir)?;
    std::fs::create_dir_all(agents_dir.join(".mars"))?;

    // Write empty config
    let config = Config {
        sources: indexmap::IndexMap::new(),
        settings: Settings::default(),
    };
    crate::config::save(&agents_dir, &config)?;

    // Add .mars/ to .gitignore if not already there
    add_to_gitignore(&agents_dir)?;

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "path": agents_dir.to_string_lossy(),
        }));
    } else {
        output::print_success(&format!(
            "initialized {} with agents.toml",
            agents_dir.display()
        ));
    }

    Ok(0)
}

fn resolve_base(path: &Path) -> Result<PathBuf, MarsError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

/// Add `.mars/` to `.gitignore` in the agents directory if not already present.
fn add_to_gitignore(agents_dir: &Path) -> Result<(), MarsError> {
    let gitignore_path = agents_dir.join(".gitignore");
    let entry = ".mars/";

    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        if content.lines().any(|line| line.trim() == entry) {
            return Ok(());
        }
        // Append
        let mut new_content = content;
        if !new_content.ends_with('\n') && !new_content.is_empty() {
            new_content.push('\n');
        }
        new_content.push_str(entry);
        new_content.push('\n');
        crate::fs::atomic_write(&gitignore_path, new_content.as_bytes())?;
    } else {
        crate::fs::atomic_write(&gitignore_path, format!("{entry}\n").as_bytes())?;
    }

    Ok(())
}
