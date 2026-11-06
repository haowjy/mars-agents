//! `mars doctor` — validate state consistency.

use crate::error::MarsError;
use crate::hash;
use crate::types::managed_cmd;

use super::output;

/// Arguments for `mars doctor`.
#[derive(Debug, clap::Args)]
pub struct DoctorArgs {}

/// Run `mars doctor`.
pub fn run(_args: &DoctorArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // Check config is valid
    let project_config = match crate::config::load_project_config_layers(&ctx.project_root) {
        Ok(loaded) => Some(loaded),
        Err(e) => {
            errors.push(format!("config error: {e}"));
            None
        }
    };

    // Check lock file
    let lock = match crate::lock::load(&ctx.project_root) {
        Ok(l) => l,
        Err(e) => {
            errors.push(format!("lock file error: {e}"));
            output::print_doctor(&errors, &warnings, json);
            return Ok(2);
        }
    };

    // Check each locked item against .mars/ canonical store
    let mars_dir = ctx.project_root.join(".mars");
    for (dest_path_str, item) in lock.canonical_flat_items() {
        let disk_path = dest_path_str.resolve(&mars_dir);

        // Check file exists
        if !disk_path.exists() {
            errors.push(format!(
                "{dest_path_str} missing from disk. Run `mars sync` to reinstall or `mars repair` to rebuild"
            ));
            continue;
        }

        // Check for conflict markers
        if item.kind == mars_schema::item::ItemKind::Agent
            && super::conflict_markers::file_has_conflict_markers(&disk_path)
        {
            errors.push(format!("{dest_path_str} has unresolved conflict markers"));
        }

        // A differing checksum is a local modification, not a fault; only an unreadable item is.
        if let Err(e) = hash::compute_hash(&disk_path, item.kind) {
            errors.push(format!("can't hash {dest_path_str}: {e}"));
        }
    }

    // Check agent→skill references
    if let Some(project_config) = &project_config {
        match crate::config::merge_with_root(
            project_config.config.clone(),
            project_config.local.clone(),
            &project_config.user,
            &ctx.project_root,
        ) {
            Ok((effective, _diagnostics)) => {
                // Check that all sources in config have corresponding lock entries
                for source_name in effective.dependencies.keys() {
                    if !lock.dependencies.contains_key(source_name) {
                        errors.push(format!(
                            "dependency `{source_name}` is in config but not in lock — run `{cmd}`",
                            cmd = managed_cmd("mars sync"),
                        ));
                    }
                }
            }
            Err(err) => errors.push(format!("config error: {err}")),
        }
    }

    // Check skill dependencies — every agent's declared skills must exist on disk.
    // Uses discover_installed() to scan the actual filesystem, catching both
    // mars-managed and user-created local agents/skills.
    {
        use std::collections::HashSet;

        let installed = installed_items(&mars_dir)?;

        // Warn on legacy symlinks found in managed directories.
        for item in installed.agents.iter().chain(installed.skills.iter()) {
            if item
                .path
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
            {
                let kind = if item.kind == mars_schema::item::ItemKind::Agent {
                    "agent"
                } else {
                    "skill"
                };
                warnings.push(format!(
                    "legacy symlinked {kind} `{}` detected in managed dir — run `{cmd}` to normalize to copied content",
                    item.name,
                    cmd = managed_cmd("mars sync"),
                ));
            }
        }

        let available_skills: HashSet<String> =
            installed.skills.iter().map(|s| s.name.clone()).collect();

        // The rule `mars sync` applies to a ref, against what is installed. Doctor has no
        // resolved graph, so a package's declaration is the project's own `[package]`.
        let provided = crate::provided::ProvidedNames::load(
            &ctx.project_root,
            &mut crate::diagnostic::DiagnosticCollector::new(),
        )?;
        let declared = project_config
            .as_ref()
            .and_then(|layers| layers.config.package.as_ref())
            .map(|package| package.uses_providers.as_slice());
        for agent in &installed.agents {
            // An unreadable agent file has no skills to check.
            let skills = crate::validate::parse_agent_skills(&agent.path).unwrap_or_default();
            for skill_name in skills {
                let provided_here = provided.provides(&skill_name);
                let Some(warning) = crate::validate::classify_skill_ref(
                    crate::lock::ItemId {
                        kind: mars_schema::item::ItemKind::Agent,
                        name: agent.name.as_str().into(),
                    },
                    skill_name.clone(),
                    &available_skills,
                    declared,
                    provided_here,
                ) else {
                    continue;
                };
                let mut msg = warning.message();
                // A plain missing name is an error; a provider-shaped one could be a typo
                // (`nope__meridain`) or a host's skill that is not installed, so it warns.
                let is_error = matches!(
                    warning,
                    crate::validate::ValidationWarning::MissingSkill { .. }
                ) && mars_schema::provider::provider_of(&skill_name).is_none();
                let no_suggestion = matches!(
                    warning,
                    crate::validate::ValidationWarning::MissingSkill {
                        suggestion: None,
                        ..
                    }
                );
                if is_error {
                    if no_suggestion {
                        msg.push_str(&format!(
                        " — add a source that provides it, or create it locally in skills/{skill_name}/"
                        ));
                    }
                    errors.push(msg);
                } else {
                    warnings.push(msg);
                }
            }
        }
    }

    // Check .mars/ gitignore as warning only.
    check_mars_gitignore(&ctx.project_root, &mut warnings);
    check_legacy_agents_directory(
        &ctx.project_root,
        project_config
            .as_ref()
            .map(|loaded| &loaded.effective.settings),
        &mut warnings,
    );

    // Check explicitly managed native targets against lock checksums.
    if let Some(project_config) = &project_config {
        let divergence = check_target_divergence(
            &ctx.project_root,
            &lock,
            &project_config.effective.settings.managed_targets(),
            &mut warnings,
        );
        if let Some(summary) = divergence.summary() {
            warnings.push(summary);
        }
    }

    output::print_doctor(&errors, &warnings, json);

    if errors.is_empty() { Ok(0) } else { Ok(2) }
}

/// Warn if a legacy `.agents/` tree remains but is no longer configured as an
/// explicit target.
fn check_legacy_agents_directory(
    project_root: &std::path::Path,
    settings: Option<&crate::config::Settings>,
    warnings: &mut Vec<String>,
) {
    let agents_dir = project_root.join(".agents");
    if agents_dir.symlink_metadata().is_err() {
        return;
    }

    let is_explicit_target = settings.is_some_and(|settings| {
        settings
            .managed_targets()
            .iter()
            .any(|target| target == ".agents")
    });

    if !is_explicit_target {
        warnings.push(
            "legacy `.agents/` directory exists but is not an explicit target — `.mars/` is the canonical store; remove `.agents/` after confirming no local-only content remains"
                .to_string(),
        );
    }
}

/// Check if .mars/ is properly gitignored (D29).
///
/// Mars does NOT auto-edit .gitignore — it only warns via `mars doctor`.
fn check_mars_gitignore(project_root: &std::path::Path, warnings: &mut Vec<String>) {
    let mars_dir = project_root.join(".mars");
    if !mars_dir.exists() {
        return;
    }

    let gitignore_path = project_root.join(".gitignore");
    let is_ignored = match std::fs::read_to_string(&gitignore_path) {
        Ok(content) => content.lines().any(|line| {
            let trimmed = line.trim();
            trimmed == ".mars" || trimmed == ".mars/" || trimmed == "/.mars" || trimmed == "/.mars/"
        }),
        Err(_) => false,
    };

    if !is_ignored {
        warnings.push(
            ".mars/ is not in .gitignore — add `.mars/` to your .gitignore to avoid committing cached data"
                .to_string(),
        );
    }
}

/// How many target items differ from the lock, by what a reader should do about each.
#[derive(Debug, Default, PartialEq, Eq)]
struct TargetDivergence {
    /// Hand-edited native agents and skills: the next plain sync overwrites the edit.
    overwritten: usize,
    /// Any other hand-edited item (hooks): sync keeps the edit until `--force`.
    kept: usize,
    /// Absent from the target.
    missing: usize,
    /// Present, but Mars could not read it to compare (permissions, I/O).
    unchecked: usize,
    /// A file where a directory belongs, the reverse, or a symlink to nothing: remove it and
    /// sync reinstalls (`--force` cannot replace a directory with a file).
    wrong_kind: usize,
}

impl TargetDivergence {
    /// The closing line: one next step per kind, each one that works for that kind.
    /// Plain sync overwrites an edited native agent or skill, so that step is a warning, not a
    /// command; only an edited hook needs `--force` (which is project-wide). A plain sync also
    /// restores a missing item, and an unreadable or wrong-kind one needs its cause removed
    /// first, not an overwrite.
    fn summary(&self) -> Option<String> {
        let mut steps = Vec::new();
        if self.overwritten > 0 {
            steps.push(format!(
                "`{}` overwrites edited agents and skills, so copy any edit you want to keep into its source first",
                managed_cmd("mars sync")
            ));
        }
        if self.kept > 0 {
            steps.push(format!(
                "run `{}` to reset edited hooks (it resets every diverged item in the project)",
                managed_cmd("mars sync --force")
            ));
        }
        if self.missing > 0 {
            steps.push(format!(
                "run `{}` to restore missing files",
                managed_cmd("mars sync")
            ));
        }
        if self.wrong_kind > 0 {
            steps.push(format!(
                "move or delete the items of the wrong kind, then run `{}`",
                managed_cmd("mars sync")
            ));
        }
        if self.unchecked > 0 {
            steps.push(format!(
                "fix the cause reported above for the items Mars could not check, then run `{}` again",
                managed_cmd("mars doctor")
            ));
        }
        (!steps.is_empty()).then(|| format!("target divergence detected; {}", steps.join("; ")))
    }
}

/// Check managed target directories against lockfile-installed checksums.
fn check_target_divergence(
    project_root: &std::path::Path,
    lock: &crate::lock::LockFile,
    targets: &[String],
    warnings: &mut Vec<String>,
) -> TargetDivergence {
    let mut divergence = TargetDivergence::default();

    for target_name in targets {
        for (dest_path, item) in lock.flat_items_for_target(target_name) {
            let target_path = project_root.join(target_name).join(dest_path.as_str());
            let shown = format!("{target_name}/{}", dest_path.as_str());

            // A generated hook file is one file; the lock records its plain content hash.
            let hashed = if crate::lock::is_generated_hook_file(item.kind, dest_path.as_str()) {
                hash::hash_file(&target_path)
            } else {
                hash::compute_hash(&target_path, item.kind)
            };
            match hashed {
                Ok(target_hash) if target_hash == item.installed_checksum => {}
                Ok(_) => {
                    // What plain sync does with the edit, by kind (see `target_sync` for
                    // skills, `compiler::native_agents` for agents, `file_hooks` for hooks).
                    if matches!(
                        item.kind,
                        mars_schema::item::ItemKind::Agent | mars_schema::item::ItemKind::Skill
                    ) {
                        warnings.push(format!(
                            "divergent in target: {shown} (local modifications; the next sync overwrites them)"
                        ));
                        divergence.overwritten += 1;
                    } else {
                        warnings.push(format!(
                            "divergent in target: {shown} (local modifications; sync keeps them)"
                        ));
                        divergence.kept += 1;
                    }
                }
                // The error kind alone says nothing about the item's own path (a tree walk can
                // fail on something inside it), so look at the item itself.
                Err(error) => match target_path.symlink_metadata() {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        warnings.push(format!("missing in target: {shown}"));
                        divergence.missing += 1;
                    }
                    Ok(_) if !target_path.exists() => {
                        warnings.push(format!(
                            "wrong kind of file in target: {shown} (a symlink to nothing)"
                        ));
                        divergence.wrong_kind += 1;
                    }
                    _ if matches!(
                        io_error_kind(&error),
                        Some(std::io::ErrorKind::NotADirectory | std::io::ErrorKind::IsADirectory)
                    ) =>
                    {
                        warnings.push(format!("wrong kind of file in target: {shown} ({error})"));
                        divergence.wrong_kind += 1;
                    }
                    _ => {
                        warnings.push(format!("could not check target item: {shown} ({error})"));
                        divergence.unchecked += 1;
                    }
                },
            }
        }
    }

    divergence
}

fn io_error_kind(error: &MarsError) -> Option<std::io::ErrorKind> {
    match error {
        MarsError::Io { source, .. } => Some(source.kind()),
        _ => None,
    }
}

struct InstalledItem {
    kind: mars_schema::item::ItemKind,
    name: String,
    path: std::path::PathBuf,
}

struct InstalledItems {
    agents: Vec<InstalledItem>,
    skills: Vec<InstalledItem>,
}

/// The Agents and skills installed under the managed root, selected by the same preparation as
/// any package, so user-created local ones count too.
fn installed_items(mars_dir: &std::path::Path) -> Result<InstalledItems, MarsError> {
    let mut diag = crate::diagnostic::DiagnosticCollector::new();
    let prepared = crate::package_prepare::prepare_authored(
        mars_dir,
        mars_schema::prepare::PrepareContext::AuthoredLocal,
        Some(mars_schema::dialect::Dialect::MarsNative),
        &mut diag,
    )?;
    let mut installed = InstalledItems {
        agents: Vec::new(),
        skills: Vec::new(),
    };
    for definition in &prepared.definitions {
        let item = crate::discover::definition_item(definition, &definition.source_path);
        let record = InstalledItem {
            kind: item.id.kind,
            name: definition.slug.clone(),
            path: mars_dir.join(&item.source_path),
        };
        match record.kind {
            mars_schema::item::ItemKind::Agent => installed.agents.push(record),
            _ => installed.skills.push(record),
        }
    }
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{check_legacy_agents_directory, check_target_divergence};

    /// Returns (key, LockedItemV2) for insertion into LockFile.items.
    ///
    /// Creates output records for `.mars` (canonical) plus any additional targets.
    fn make_locked_agent(
        name: &str,
        dest_path: &str,
        expected_content: &str,
        extra_targets: &[&str],
    ) -> (String, crate::lock::LockedItemV2) {
        let expected_hash = crate::hash::hash_bytes(expected_content.as_bytes());
        let key = format!("agent/{name}");
        let mut outputs = vec![crate::lock::OutputRecord::installed(
            ".mars".to_string(),
            dest_path.into(),
            expected_hash.clone().into(),
        )];
        for target in extra_targets {
            outputs.push(crate::lock::OutputRecord::installed(
                (*target).to_string(),
                dest_path.into(),
                expected_hash.clone().into(),
            ));
        }
        let item = crate::lock::LockedItemV2 {
            source: "test-source".into(),
            kind: mars_schema::item::ItemKind::Agent,
            version: None,
            source_checksum: expected_hash.clone().into(),
            source_path: None,
            outputs,
        };
        (key, item)
    }

    #[test]
    fn check_target_divergence_warns_for_missing_and_modified_items() {
        let temp = TempDir::new().expect("create temp dir");
        let root = temp.path();

        let mut lock = crate::lock::LockFile::empty();
        let (k, v) = make_locked_agent(
            "missing",
            "agents/missing.md",
            "expected missing",
            &[".agents"],
        );
        lock.items.insert(k, v);
        let (k, v) = make_locked_agent(
            "modified",
            "agents/modified.md",
            "expected content",
            &[".agents"],
        );
        lock.items.insert(k, v);

        fs::create_dir_all(root.join(".agents/agents")).expect("create target dir");
        fs::write(root.join(".agents/agents/modified.md"), "local edits")
            .expect("write modified file");

        let mut warnings = Vec::new();
        let divergence =
            check_target_divergence(root, &lock, &[".agents".to_string()], &mut warnings);

        assert_eq!((divergence.overwritten, divergence.missing), (1, 1));
        assert!(
            warnings
                .iter()
                .any(|w| w == "missing in target: .agents/agents/missing.md")
        );
        assert!(
            warnings
                .iter()
                .any(|w| w
                    == "divergent in target: .agents/agents/modified.md (local modifications; the next sync overwrites them)")
        );
    }

    #[test]
    fn check_target_divergence_checks_every_managed_target() {
        let temp = TempDir::new().expect("create temp dir");
        let root = temp.path();

        let mut lock = crate::lock::LockFile::empty();
        let (k, v) = make_locked_agent(
            "test",
            "agents/test.md",
            "expected content",
            &[".agents", ".claude"],
        );
        lock.items.insert(k, v);

        fs::create_dir_all(root.join(".agents/agents")).expect("create .agents tree");
        fs::write(root.join(".agents/agents/test.md"), "expected content")
            .expect("write matching file");

        let mut warnings = Vec::new();
        let divergence = check_target_divergence(
            root,
            &lock,
            &[".agents".to_string(), ".claude".to_string()],
            &mut warnings,
        );

        assert_eq!((divergence.overwritten, divergence.missing), (0, 1));
        assert!(
            warnings
                .iter()
                .any(|w| w == "missing in target: .claude/agents/test.md")
        );
    }

    #[test]
    fn check_legacy_agents_directory_warns_when_not_explicit_target() {
        let temp = TempDir::new().expect("create temp dir");
        fs::create_dir_all(temp.path().join(".agents")).expect("create legacy dir");

        let mut warnings = Vec::new();
        check_legacy_agents_directory(
            temp.path(),
            Some(&crate::config::Settings::default()),
            &mut warnings,
        );

        assert!(
            warnings
                .iter()
                .any(|w| w.contains("legacy `.agents/` directory exists"))
        );
    }

    #[test]
    fn check_legacy_agents_directory_allows_explicit_target() {
        let temp = TempDir::new().expect("create temp dir");
        fs::create_dir_all(temp.path().join(".agents")).expect("create legacy dir");

        let settings = crate::config::Settings {
            targets: Some(vec![".agents".to_string()]),
            ..crate::config::Settings::default()
        };
        let mut warnings = Vec::new();
        check_legacy_agents_directory(temp.path(), Some(&settings), &mut warnings);

        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }
}
