//! `mars init [TARGET] [--link DIR...]` — scaffold a mars project.
//!
//! Creates `<project-root>/mars.toml` and `<project-root>/.mars`.
//! If TARGET is provided, also creates `<project-root>/TARGET` as a managed output dir.
//! Use `--root` to select an explicit project root.
//!
//! Init does NOT walk up — it creates a project at cwd or the `--root` target.
//! Idempotent: re-running is a no-op for initialization but still processes
//! `--link` flags.

use std::path::{Path, PathBuf};

use crate::error::{ConfigError, MarsError};

use super::output;

/// Arguments for `mars init`.
#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Optional directory name to create for managed output.
    pub target: Option<String>,

    /// Directories to link after initialization. Repeatable.
    #[arg(long, value_name = "DIR")]
    pub link: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InitializedProject {
    pub project_root: PathBuf,
    pub managed_root: Option<PathBuf>,
    pub already_initialized: bool,
}

/// Validate that a target is a simple directory name, not a path.
fn validate_target(target: &str) -> Result<(), MarsError> {
    if target.contains('/') || target.contains('\\') {
        return Err(MarsError::Config(ConfigError::Invalid {
            message: format!(
                "`{target}` looks like a path — TARGET should be a directory name \
                 like `.claude` or `.codex`. Use `--root` to specify project root."
            ),
        }));
    }
    if target == "." || target == ".." || target.is_empty() {
        return Err(MarsError::Config(ConfigError::Invalid {
            message: format!(
                "`{target}` is not a valid target name — use a directory name like `.claude` or `.codex`."
            ),
        }));
    }
    Ok(())
}

fn ensure_consumer_config(project_root: &Path) -> Result<bool, MarsError> {
    let config_path = project_root.join("mars.toml");
    if config_path.exists() {
        return Ok(true);
    }

    crate::fs::atomic_write(&config_path, b"[dependencies]\n")?;
    Ok(false)
}

pub(super) fn initialize_project(
    explicit_root: Option<&Path>,
    target_override: Option<&str>,
) -> Result<InitializedProject, MarsError> {
    let project_root = explicit_root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().expect("cannot determine current directory"));

    std::fs::create_dir_all(project_root.join(".mars"))?;

    let already_initialized = ensure_consumer_config(&project_root)?;

    let managed_root = if let Some(target) = explicit_init_target(&project_root, target_override)? {
        validate_target(&target)?;
        let managed_root = project_root.join(&target);
        std::fs::create_dir_all(&managed_root)?;
        persist_managed_root(&project_root, Some(&target))?;
        Some(managed_root)
    } else {
        persist_managed_root(&project_root, None)?;
        None
    };

    Ok(InitializedProject {
        project_root,
        managed_root,
        already_initialized,
    })
}

/// Run `mars init`.
///
/// Init creates a project at cwd or `--root` target. It does NOT walk up.
pub fn run(args: &InitArgs, explicit_root: Option<&Path>, json: bool) -> Result<i32, MarsError> {
    let initialized = initialize_project(explicit_root, args.target.as_deref())?;
    let project_root = initialized.project_root;
    let managed_root = initialized.managed_root;
    let already_initialized = initialized.already_initialized;

    if !json {
        if already_initialized {
            output::print_info(&format!("{} already initialized", project_root.display()));
        } else {
            output::print_success(&format!(
                "initialized {} with mars.toml",
                project_root.display()
            ));
        }
    }

    // 5. Process --link flags
    if !args.link.is_empty() {
        let context_managed_root = managed_root
            .clone()
            .unwrap_or_else(|| project_root.join(".mars"));
        let ctx = super::MarsContext::from_roots(project_root.clone(), context_managed_root)?;
        for link_target in &args.link {
            let link_args = super::link::LinkArgs {
                target: link_target.clone(),
                force: false,
            };
            super::link::run(&link_args, &ctx, json)?;
        }
    }

    // Only a project that declares `[package]` publishes anything to check; a consumer
    // has no package to read through the publish lens.
    if crate::config::load(&project_root)?.package.is_some() {
        let mut publish_diag = crate::diagnostic::DiagnosticCollector::new();
        let (lens, prepared) = crate::resolve::PublishLens::load(&project_root, &mut publish_diag)?;
        let mut diagnostics = publish_diag.drain();
        diagnostics.extend(
            crate::compiler::lossiness_preview::collect_prepared_source_lossiness_diagnostics(
                &lens,
                &prepared,
                &project_root,
                crate::diagnostic::LossinessMode::Surface,
            )?,
        );
        if !json && !diagnostics.is_empty() {
            output::print_diagnostics(&diagnostics);
        }
    }

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "project_root": project_root.to_string_lossy(),
            "managed_root": managed_root.as_ref().map(|path| path.to_string_lossy().to_string()),
            "already_initialized": already_initialized,
            "links": args.link,
        }));
    }

    Ok(0)
}

fn explicit_init_target(
    project_root: &Path,
    target_override: Option<&str>,
) -> Result<Option<String>, MarsError> {
    if let Some(target) = target_override {
        return Ok(Some(target.to_string()));
    }

    match crate::config::load(project_root) {
        Ok(config) => Ok(config.settings.managed_root),
        Err(MarsError::Config(ConfigError::NotFound { .. })) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Persist managed_root in mars.toml [settings].
fn persist_managed_root(project_root: &Path, target: Option<&str>) -> Result<(), MarsError> {
    match crate::config::load(project_root) {
        Ok(mut config) => {
            config.settings.managed_root = target.map(str::to_string);
            crate::config::save(project_root, &config)?;
        }
        Err(MarsError::Config(ConfigError::NotFound { .. })) => {
            // Config will be created by ensure_consumer_config — skip
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn validate_target_accepts_simple_names() {
        for target in [".agents", ".claude", "my-agents"] {
            assert!(validate_target(target).is_ok(), "{target}");
        }
        for target in ["./foo", "foo/bar", "/absolute/path", ".", "..", ""] {
            assert!(validate_target(target).is_err(), "{target:?}");
        }
    }

    #[test]
    fn initialize_project_with_explicit_target_persists_managed_root() {
        // `.agents` is the deprecated target, but an explicit one is still persisted.
        for target in [".claude", ".agents"] {
            let dir = TempDir::new().unwrap();

            let initialized = initialize_project(Some(dir.path()), Some(target)).unwrap();

            assert!(dir.path().join(".mars").exists());
            assert!(dir.path().join(target).exists());
            assert_eq!(initialized.managed_root, Some(dir.path().join(target)));

            let config = crate::config::load(dir.path()).unwrap();
            assert_eq!(config.settings.managed_root.as_deref(), Some(target));
        }
    }

    #[test]
    fn initialize_project_preserves_existing_managed_root_when_no_target_given() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[settings]\nmanaged_root = \".claude\"\n",
        )
        .unwrap();

        let initialized = initialize_project(Some(dir.path()), None).unwrap();

        assert!(dir.path().join(".claude").exists());
        assert_eq!(initialized.managed_root, Some(dir.path().join(".claude")));
    }
}
