//! `mars init [TARGET] [--link DIR...]` — scaffold a mars project.
//!
//! Creates `<project-root>/mars.toml` and `<project-root>/.mars`.
//! If TARGET is provided, also creates `<project-root>/TARGET` as a managed output dir.
//! Use `--root` to select an explicit project root.
//!
//! Init does NOT walk up — it creates a project at cwd or the `--root` target, never in the
//! home directory (the user level is `mars --user`).
//! Idempotent: re-running is a no-op for initialization but still processes
//! `--link` flags.

use crate::layout::Layout;
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
    pub already_initialized: bool,
}

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

fn ensure_consumer_config(layout: &Layout) -> Result<bool, MarsError> {
    let config_path = layout.manifest();
    if config_path.exists() {
        return Ok(true);
    }

    crate::fs::atomic_write(&config_path, b"[dependencies]\n")?;
    Ok(false)
}

/// Why no project can be started at `dir`, or `None` when one can. The walk-up never opens a
/// home project, so one made there would be invisible; the user config folder's `mars.toml` is
/// the user level's.
pub(super) fn no_project_at(dir: &Path) -> Option<String> {
    let user = Layout::user_level().ok();
    no_project_in(
        &dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()),
        crate::platform::config::canonical_home().as_deref(),
        user.as_ref().map(Layout::root),
    )
}

/// [`no_project_at`] for a canonical `dir`, with the home and user config folders given.
pub(super) fn no_project_in(
    dir: &Path,
    home: Option<&Path>,
    user_config: Option<&Path>,
) -> Option<String> {
    if Some(dir) == user_config {
        Some(format!(
            "{} is your user level's config folder: {}",
            dir.display(),
            user_level_needs_no_init()
        ))
    } else if Some(dir) == home {
        Some(format!(
            "{} is your home directory, which is never a Mars project; use `{}` for your user level, or start a project in a folder of its own",
            dir.display(),
            crate::types::managed_cmd("mars --user <command>"),
        ))
    } else {
        None
    }
}

pub(super) fn user_level_needs_no_init() -> String {
    format!(
        "the user level needs no init; `{}` creates its mars.toml",
        crate::types::managed_cmd("mars --user add <source>")
    )
}

pub(super) fn initialize_project(
    explicit_root: Option<&Path>,
    target_override: Option<&str>,
) -> Result<InitializedProject, MarsError> {
    let project_root = explicit_root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().expect("cannot determine current directory"));
    if let Some(message) = no_project_at(&project_root) {
        return Err(MarsError::Config(ConfigError::Invalid { message }));
    }

    let layout = Layout::project(&project_root);
    std::fs::create_dir_all(layout.store())?;

    let already_initialized = ensure_consumer_config(&layout)?;

    if let Some(target) = target_override {
        validate_target(target)?;
        std::fs::create_dir_all(layout.target(target))?;
        persist_target(&layout, target)?;
    }

    Ok(InitializedProject {
        project_root,
        already_initialized,
    })
}

/// Init creates a project at cwd or `--root` target. It does NOT walk up.
pub fn run(args: &InitArgs, explicit_root: Option<&Path>, json: bool) -> Result<i32, MarsError> {
    let initialized = initialize_project(explicit_root, args.target.as_deref())?;
    let project_root = initialized.project_root;
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

    if !args.link.is_empty() {
        let ctx = super::MarsContext::new(project_root.clone())?;
        for link_target in &args.link {
            let link_args = super::link::LinkArgs {
                target: link_target.clone(),
                force: false,
                local: false,
                host: Default::default(),
            };
            super::link::run(&link_args, &ctx, json)?;
        }
    }

    // Only a project that declares `[package]` publishes anything to check; a consumer
    // has no package to read through the publish lens.
    if crate::config::load(&Layout::project(&project_root))?
        .package
        .is_some()
    {
        let mut publish_diag = crate::diagnostic::DiagnosticCollector::new();
        let (lens, prepared) = crate::resolve::PublishLens::load(
            &project_root,
            crate::package_prepare::FileFindings::Print,
            &mut publish_diag,
        )?;
        let mut diagnostics = publish_diag.drain();
        diagnostics.extend(
            crate::compiler::lossiness_preview::collect_prepared_source_lossiness_diagnostics(
                &lens,
                &prepared,
                &project_root,
                crate::diagnostic::LossinessMode::Surface,
                "mars check --verbose",
            )?,
        );
        if !json && !diagnostics.is_empty() {
            output::print_diagnostics(&diagnostics, false);
        }
    }

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "project_root": project_root.to_string_lossy(),
            "already_initialized": already_initialized,
            "links": args.link,
        }));
    }

    Ok(0)
}

/// Add `target` to `settings.targets`, as `mars link` records it.
fn persist_target(layout: &Layout, target: &str) -> Result<(), MarsError> {
    let target = crate::config::migrations::link::normalize_link(target).target;
    let mut config = crate::config::load(layout)?;
    let settings = &mut config.settings;
    // The key is retired; leaving it beside the `targets` just written would only warn.
    settings.managed_root = None;
    let targets = settings.targets.get_or_insert_with(Vec::new);
    if !targets.contains(&target) {
        targets.push(target);
        crate::config::save(layout, &config)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_on_empty_project_runs_lossiness_pass_without_error() {
        let dir = TempDir::new().unwrap();
        let args = super::InitArgs {
            target: None,
            link: Vec::new(),
        };
        let code = super::run(&args, Some(dir.path()), false).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn validate_target_accepts_simple_names() {
        assert!(validate_target(".agents").is_ok());
        assert!(validate_target(".claude").is_ok());
        assert!(validate_target("my-agents").is_ok());
    }

    #[test]
    fn validate_target_rejects_paths() {
        assert!(validate_target("./foo").is_err());
        assert!(validate_target("foo/bar").is_err());
        assert!(validate_target("/absolute/path").is_err());
    }

    #[test]
    fn validate_target_rejects_dots() {
        assert!(validate_target(".").is_err());
        assert!(validate_target("..").is_err());
    }

    #[test]
    fn validate_target_rejects_empty() {
        assert!(validate_target("").is_err());
    }

    #[test]
    fn ensure_consumer_config_creates_root_mars_toml() {
        let dir = TempDir::new().unwrap();

        let already = ensure_consumer_config(&crate::layout::Layout::project(dir.path())).unwrap();
        assert!(!already);

        let content = std::fs::read_to_string(dir.path().join("mars.toml")).unwrap();
        assert!(content.contains("[dependencies]"));
    }

    #[test]
    fn ensure_consumer_config_accepts_existing_mars_toml() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[package]\nschema = 1\nname = \"pkg\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let already = ensure_consumer_config(&crate::layout::Layout::project(dir.path())).unwrap();
        assert!(already);
    }

    #[test]
    fn initialize_project_without_target_creates_mars_only() {
        let dir = TempDir::new().unwrap();

        initialize_project(Some(dir.path()), None).unwrap();

        assert!(dir.path().join(".mars").exists());
        assert!(!dir.path().join(".agents").exists());

        let config = crate::config::load(&crate::layout::Layout::project(dir.path())).unwrap();
        assert!(config.settings.targets.is_none());
    }

    #[test]
    fn initialize_project_with_explicit_target_persists_targets() {
        let dir = TempDir::new().unwrap();

        initialize_project(Some(dir.path()), Some(".claude")).unwrap();
        initialize_project(Some(dir.path()), Some(".claude")).unwrap();

        assert!(dir.path().join(".claude").exists());
        let config = crate::config::load(&crate::layout::Layout::project(dir.path())).unwrap();
        assert_eq!(config.settings.targets, Some(vec![".claude".to_string()]));
    }

    #[test]
    fn initialize_project_with_explicit_agents_persists_deprecated_target() {
        let dir = TempDir::new().unwrap();

        initialize_project(Some(dir.path()), Some(".agents")).unwrap();

        assert!(dir.path().join(".agents").exists());
        let config = crate::config::load(&crate::layout::Layout::project(dir.path())).unwrap();
        assert_eq!(config.settings.targets, Some(vec![".agents".to_string()]));
    }
}
