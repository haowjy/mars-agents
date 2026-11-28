//! `mars doctor` — validate state consistency.

use crate::diagnostic::{DiagnosticCollector, DiagnosticLevel};
use crate::error::MarsError;
use crate::hash;

use super::output;

/// Arguments for `mars doctor`.
#[derive(Debug, clap::Args)]
pub struct DoctorArgs {}

pub fn run(_args: &DoctorArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    let mut diag = DiagnosticCollector::new();
    if ctx.layout.kind() == crate::layout::RootKind::User {
        check_user_folder(&ctx.layout, &mut diag);
    }
    // Read the curation files as `mars models` does, so their problems are reported here too.
    if let Err(err) = crate::curation::CuratedRules::load(&ctx.layout) {
        diag.error("curated-error", format!("curation error: {err}"));
    }

    let project_config = match crate::config::load_project_config_layers(&ctx.layout) {
        Ok(loaded) => Some(loaded),
        Err(e) => {
            diag.error("config-error", format!("config error: {e}"));
            None
        }
    };

    let lock = match crate::lock::load(&ctx.layout) {
        Ok(l) => l,
        Err(e) => {
            diag.error("lock-error", format!("lock file error: {e}"));
            output::print_doctor(&diag.drain(), json);
            return Ok(2);
        }
    };

    // Judge the half of the lock that sync maintains. A target a config file names but the sync
    // does not manage (retired `.cursor`) keeps its records and its
    // `.mars/<surface>/<target>/` copies, which nothing reinstalls; one no config file names
    // is removed by the next sync.
    let lock = match &project_config {
        Some(loaded) => {
            let managed = crate::config::targets::managed_targets(&loaded.effective.settings);
            lock.split_targets(|target| managed.iter().any(|m| m == target))
                .0
        }
        None => lock,
    };

    let mars_dir = ctx.layout.store();
    for (dest_path_str, item) in lock.canonical_flat_items() {
        let disk_path = dest_path_str.resolve(mars_dir);

        if !disk_path.exists() {
            diag.error(
                "item-missing",
                format!(
                    "{dest_path_str} missing from disk. Run `{}` to reinstall or `{}` to rebuild",
                    ctx.layout.cmd("sync"),
                    ctx.layout.cmd("repair"),
                ),
            );
            continue;
        }

        if item.kind == mars_schema::item::ItemKind::Agent
            && super::conflict_markers::file_has_conflict_markers(&disk_path)
        {
            diag.error(
                "conflict-markers",
                format!("{dest_path_str} has unresolved conflict markers"),
            );
        }

        // A differing checksum is a local modification, not a fault; only an unreadable item is.
        if let Err(e) = hash::compute_hash(&disk_path, item.kind) {
            diag.error(
                "item-unreadable",
                format!("can't hash {dest_path_str}: {e}"),
            );
        }
    }

    if let Some(project_config) = &project_config {
        match crate::config::merge_with_root(
            project_config.config.clone(),
            project_config.local.clone(),
            &project_config.user,
            ctx.layout.root(),
        ) {
            Ok((effective, _diagnostics)) => {
                for source_name in effective.dependencies.keys() {
                    if !lock.dependencies.contains_key(source_name) {
                        diag.error(
                            "dependency-not-locked",
                            format!(
                                "dependency `{source_name}` is in config but not in lock — run `{cmd}`",
                                cmd = ctx.layout.cmd("sync"),
                            ),
                        );
                    }
                }
            }
            Err(err) => diag.error("config-error", format!("config error: {err}")),
        }
    }

    'installed: {
        use std::collections::HashSet;

        let installed = match installed_items(mars_dir) {
            Ok(installed) => installed,
            Err(e) => {
                diag.error("installed-items", e.to_string());
                break 'installed;
            }
        };

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
                diag.warn(
                    "legacy-symlink",
                    format!(
                        "legacy symlinked {kind} `{}` detected in managed dir — run `{cmd}` to normalize to copied content",
                        item.name,
                        cmd = ctx.layout.cmd("sync"),
                    ),
                );
            }
        }

        let available_skills: HashSet<String> =
            installed.skills.iter().map(|s| s.name.clone()).collect();

        // The rule `mars sync` applies to a ref, against what is installed. Doctor has no
        // resolved graph: the owner comes from the lock, and the only `[package]` it reads is the
        // project's, so a dependency's refs are judged as if it declared nothing.
        let provided = crate::provided::ProvidedNames::load(
            &ctx.layout,
            &mut crate::diagnostic::DiagnosticCollector::new(),
        )?;
        let own_source = crate::sync::validate::OwnSource::new(
            &ctx.layout,
            project_config
                .as_ref()
                .and_then(|layers| layers.config.package.as_ref())
                .map(|package| package.uses_providers.as_slice()),
        );
        let own = crate::types::SourceOrigin::LocalPackage.to_string();
        let path_dirs: std::collections::HashMap<&str, std::path::PathBuf> = lock
            .dependencies
            .iter()
            .filter_map(|(name, row)| {
                Some((name.as_str(), ctx.layout.root().join(row.path.as_deref()?)))
            })
            .collect();
        for agent in &installed.agents {
            let skills = crate::validate::parse_agent_skills(&agent.path).unwrap_or_default();
            let source = lock
                .items
                .get(&format!("agent/{}", agent.name))
                .map_or(own.as_str(), |locked| locked.source.as_str());
            let (owner, declared) = if source == own {
                (
                    crate::diagnostic::Owner::Own {
                        dir: &own_source.dir,
                    },
                    own_source.declared,
                )
            } else {
                let dir = path_dirs.get(source).map(std::path::PathBuf::as_path);
                (crate::diagnostic::Owner::of(source, dir), None)
            };
            for skill_name in &skills {
                let reference = crate::validate::SkillRef {
                    agent: &agent.name,
                    skill_name,
                    owner,
                    declared,
                    provided_here: provided.provides(skill_name),
                };
                if let Some(found) =
                    crate::validate::classify_skill_ref(&reference, &available_skills)
                {
                    diag.push(found);
                }
            }
        }
    }

    check_mars_gitignore(&ctx.layout, &mut diag);
    check_legacy_agents_directory(
        &ctx.layout,
        project_config
            .as_ref()
            .map(|loaded| &loaded.effective.settings),
        &lock,
        &mut diag,
    );

    if let Some(project_config) = &project_config {
        let divergence = check_target_divergence(
            &ctx.layout,
            &lock,
            &crate::config::targets::managed_targets(&project_config.effective.settings),
            &mut diag,
        );
        if let Some(summary) = divergence.summary() {
            diag.warn("target-divergence", summary);
        }
    }

    // What this run read outside the root (a home project, a 0.40 config file, a renamed
    // variable) is part of the diagnosis, not a note after it.
    let diagnostics: Vec<_> = diag
        .drain()
        .into_iter()
        .chain(crate::diagnostic::take_notices())
        .collect();
    output::print_doctor(&diagnostics, json);

    if diagnostics
        .iter()
        .any(|d| d.level == DiagnosticLevel::Error)
    {
        Ok(2)
    } else {
        Ok(0)
    }
}

fn check_legacy_agents_directory(
    layout: &crate::layout::Layout,
    settings: Option<&crate::config::Settings>,
    lock: &crate::lock::LockFile,
    diag: &mut DiagnosticCollector,
) {
    let agents_dir = layout.target(".agents");
    if agents_dir.symlink_metadata().is_err() {
        return;
    }

    let is_explicit_target = settings.is_some_and(|settings| {
        crate::config::targets::managed_targets(settings)
            .iter()
            .any(|target| target == ".agents")
    });

    // An empty directory is harmless, and so is a file holding only entries Mars wrote (an
    // `.agy` plugin's pointer in `plugins.json`); only the rest is worth a word.
    let has_content = std::fs::read_dir(&agents_dir)
        .map(|entries| {
            entries.flatten().any(|entry| {
                let rel = format!(".agents/{}", entry.file_name().to_string_lossy());
                !crate::target::keyed_entries::holds_only_owned(layout, &rel, lock)
            })
        })
        // A directory we cannot read may hold content; a file or dangling link holds none.
        .unwrap_or_else(|err| err.kind() == std::io::ErrorKind::PermissionDenied);

    if !is_explicit_target && has_content {
        diag.warn(
            "agents-directory-unread",
            "`.agents/` has content, but Mars no longer reads it (`.mars/` is the canonical store) and will not touch it. Before removing it, check that nothing in it is yours or tracked by git",
        );
    }
}

/// The user level's folder: a home project still sharing it, and what an old project store
/// left in it, which nothing serves or deletes.
fn check_user_folder(layout: &crate::layout::Layout, diag: &mut DiagnosticCollector) {
    if crate::home_project::HomeProject::in_user_folder().is_some() {
        crate::home_project::raise_notice();
        return;
    }
    let leftovers = crate::home_project::store_leftovers(layout);
    if !leftovers.is_empty() {
        diag.warn(
            "old-store-left",
            format!(
                "{} holds files the user level did not install, and does not serve or remove: {}. A Mars 0.40 home project's store left them unless they are your own copies: delete them, or move what you want to keep into {}/",
                layout.root().display(),
                leftovers.join(", "),
                layout.local_source().display(),
            ),
        );
    }
}

fn check_mars_gitignore(layout: &crate::layout::Layout, diag: &mut DiagnosticCollector) {
    // The user level's store is its own folder, in no repo.
    if layout.kind() != crate::layout::RootKind::Project
        || layout.store().symlink_metadata().is_err()
    {
        return;
    }

    let gitignore_path = layout.root().join(".gitignore");
    let is_ignored = match std::fs::read_to_string(&gitignore_path) {
        Ok(content) => content.lines().any(|line| {
            let trimmed = line.trim();
            trimmed == ".mars" || trimmed == ".mars/" || trimmed == "/.mars" || trimmed == "/.mars/"
        }),
        Err(_) => false,
    };

    if !is_ignored {
        diag.warn(
            "mars-not-gitignored",
            ".mars/ is not in .gitignore — add `.mars/` to your .gitignore to avoid committing cached data",
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
                crate::layout::root_cmd("sync")
            ));
        }
        if self.kept > 0 {
            steps.push(format!(
                "run `{}` to reset edited hooks (it resets every diverged item in the project)",
                crate::layout::root_cmd("sync --force")
            ));
        }
        if self.missing > 0 {
            steps.push(format!(
                "run `{}` to restore missing files",
                crate::layout::root_cmd("sync")
            ));
        }
        if self.wrong_kind > 0 {
            steps.push(format!(
                "move or delete the items of the wrong kind, then run `{}`",
                crate::layout::root_cmd("sync")
            ));
        }
        if self.unchecked > 0 {
            steps.push(format!(
                "fix the cause reported above for the items Mars could not check, then run `{}` again",
                crate::layout::root_cmd("doctor")
            ));
        }
        (!steps.is_empty()).then(|| format!("target divergence detected; {}", steps.join("; ")))
    }
}

/// Check managed target directories against lockfile-installed checksums.
fn check_target_divergence(
    layout: &crate::layout::Layout,
    lock: &crate::lock::LockFile,
    targets: &[String],
    diag: &mut DiagnosticCollector,
) -> TargetDivergence {
    let mut divergence = TargetDivergence::default();

    for target_name in targets {
        for (dest_path, item) in lock.flat_items_for_target(target_name) {
            let target_path = layout.in_target(target_name, dest_path.as_str());
            let shown = format!("{target_name}/{}", dest_path.as_str());

            match hash::compute_output_hash(&target_path, item.kind, dest_path.as_str()) {
                Ok(target_hash) if target_hash == item.installed_checksum => {}
                Ok(_) => {
                    // What plain sync does with the edit, by kind (see `target_sync` for
                    // skills, `compiler::native_agents` for agents, `file_hooks` for hooks).
                    if matches!(
                        item.kind,
                        mars_schema::item::ItemKind::Agent | mars_schema::item::ItemKind::Skill
                    ) {
                        diag.warn(
                            "target-divergent",
                            format!(
                                "divergent in target: {shown} (local modifications; the next sync overwrites them)"
                            ),
                        );
                        divergence.overwritten += 1;
                    } else {
                        diag.warn(
                            "target-divergent",
                            format!(
                                "divergent in target: {shown} (local modifications; sync keeps them)"
                            ),
                        );
                        divergence.kept += 1;
                    }
                }
                // The error kind alone says nothing about the item's own path (a tree walk can
                // fail on something inside it), so look at the item itself.
                Err(error) => match target_path.symlink_metadata() {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        diag.warn("target-missing", format!("missing in target: {shown}"));
                        divergence.missing += 1;
                    }
                    Ok(_) if !target_path.exists() => {
                        diag.warn(
                            "target-wrong-kind",
                            format!("wrong kind of file in target: {shown} (a symlink to nothing)"),
                        );
                        divergence.wrong_kind += 1;
                    }
                    _ if matches!(
                        io_error_kind(&error),
                        Some(std::io::ErrorKind::NotADirectory | std::io::ErrorKind::IsADirectory)
                    ) =>
                    {
                        diag.warn(
                            "target-wrong-kind",
                            format!("wrong kind of file in target: {shown} ({error})"),
                        );
                        divergence.wrong_kind += 1;
                    }
                    _ => {
                        diag.warn(
                            "target-unchecked",
                            format!("could not check target item: {shown} ({error})"),
                        );
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
    // A project with `mars.toml` and no `.mars/` has not synced yet. Say so here; the generic
    // IO error from the package reader is right for its other callers.
    if mars_dir.symlink_metadata().is_err() {
        return Err(MarsError::Io {
            operation: "read compiled store".to_string(),
            path: mars_dir.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "no compiled store yet — run `{}` to create it",
                    crate::layout::root_cmd("sync")
                ),
            ),
        });
    }
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
    use crate::diagnostic::DiagnosticCollector;

    fn messages(diag: &mut DiagnosticCollector) -> Vec<String> {
        diag.drain().into_iter().map(|d| d.message).collect()
    }

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

        let mut diag = DiagnosticCollector::new();
        let divergence = check_target_divergence(
            &crate::layout::Layout::project(root),
            &lock,
            &[".agents".to_string()],
            &mut diag,
        );

        let warnings = messages(&mut diag);
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

        let mut diag = DiagnosticCollector::new();
        let divergence = check_target_divergence(
            &crate::layout::Layout::project(root),
            &lock,
            &[".agents".to_string(), ".claude".to_string()],
            &mut diag,
        );

        let warnings = messages(&mut diag);
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

        let mut diag = DiagnosticCollector::new();
        check_legacy_agents_directory(
            &crate::layout::Layout::project(temp.path()),
            Some(&crate::config::Settings::default()),
            &crate::lock::LockFile::empty(),
            &mut diag,
        );
        assert!(
            messages(&mut diag).is_empty(),
            "an empty `.agents/` is not worth a warning"
        );

        fs::write(temp.path().join(".agents/notes.md"), "mine").expect("write content");
        check_legacy_agents_directory(
            &crate::layout::Layout::project(temp.path()),
            Some(&crate::config::Settings::default()),
            &crate::lock::LockFile::empty(),
            &mut diag,
        );

        assert!(
            messages(&mut diag)
                .iter()
                .any(|w| w.contains("`.agents/` has content"))
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
        let mut diag = DiagnosticCollector::new();
        check_legacy_agents_directory(
            &crate::layout::Layout::project(temp.path()),
            Some(&settings),
            &crate::lock::LockFile::empty(),
            &mut diag,
        );

        let warnings = messages(&mut diag);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    fn agy_lock() -> crate::lock::LockFile {
        let mut lock = crate::lock::LockFile::empty();
        let data = crate::hash::hash_bytes(b"data");
        lock.items.insert(
            "hook/guard@agy".into(),
            crate::lock::LockedItemV2 {
                source: "_self".into(),
                kind: mars_schema::item::ItemKind::Hook,
                version: None,
                source_checksum: data.clone().into(),
                source_path: None,
                outputs: vec![crate::lock::OutputRecord::installed(
                    ".agy".to_string(),
                    "hooks/mars-guard.hooks.json".into(),
                    data.into(),
                )],
            },
        );
        lock.config_entries
            .entry(".agy".into())
            .or_default()
            .insert(
                "pointer:.agy".into(),
                crate::lock::ConfigEntryRecord {
                    emitted_json: r#"{"path":".agy"}"#.into(),
                },
            );
        lock
    }

    #[test]
    fn agy_files_mars_wrote_are_not_reported_but_user_files_are() {
        let temp = TempDir::new().expect("create temp dir");
        let root = temp.path();
        fs::create_dir_all(root.join(".agy/hooks")).unwrap();
        fs::create_dir_all(root.join(".agents")).unwrap();
        fs::write(root.join(".agy/hooks/mars-guard.hooks.json"), "data").unwrap();
        fs::write(
            root.join(".agents/plugins.json"),
            r#"{"entries":[{"path":".agy"}]}"#,
        )
        .unwrap();
        let lock = agy_lock();
        let targets = [".agy".to_string()];
        let settings = crate::config::Settings::default();

        let mut diag = DiagnosticCollector::new();
        check_legacy_agents_directory(
            &crate::layout::Layout::project(root),
            Some(&settings),
            &lock,
            &mut diag,
        );
        check_target_divergence(
            &crate::layout::Layout::project(root),
            &lock,
            &targets,
            &mut diag,
        );
        let warnings = messages(&mut diag);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        fs::write(
            root.join(".agents/plugins.json"),
            r#"{"entries":[{"path":".agy"},{"path":"mine"}]}"#,
        )
        .unwrap();
        fs::write(root.join(".agy/hooks/stray.txt"), "mine").unwrap();
        check_legacy_agents_directory(
            &crate::layout::Layout::project(root),
            Some(&settings),
            &lock,
            &mut diag,
        );
        assert!(
            messages(&mut diag)
                .iter()
                .any(|w| w.contains("`.agents/` has content")),
            "a user's plugin entry is unread content"
        );
    }
}
