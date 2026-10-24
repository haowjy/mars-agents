//! `mars version <bump|X.Y.Z> [--push]` — bump package version, commit, and tag.

use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use semver::{BuildMetadata, Prerelease, Version};

use crate::diagnostic::DiagnosticLevel;
use crate::error::{ConfigError, MarsError};
use crate::sync::{ResolutionMode, SyncOptions, SyncRequest};
use crate::types::MarsContext;

use super::{check, output};

/// Arguments for `mars version`.
#[derive(Debug, clap::Args)]
pub struct VersionArgs {
    /// Version bump: patch, minor, major, or explicit X.Y.Z
    pub bump: String,
    /// Push branch and tag to origin after versioning
    #[arg(long)]
    pub push: bool,
    /// Force version even if package check fails (bypass publish gate)
    #[arg(long)]
    pub force: bool,
}

/// Run `mars version`.
pub fn run(args: &VersionArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    require_clean_working_tree(&ctx.project_root)?;
    require_package_check(&ctx.project_root, args.force)?;

    let config = crate::config::load(&ctx.project_root)?;
    let package = config
        .package
        .as_ref()
        .ok_or_else(|| ConfigError::Invalid {
            message: "mars.toml must contain [package] with name and version".to_string(),
        })?;

    if package.name.trim().is_empty() {
        return Err(ConfigError::Invalid {
            message: "[package].name must not be empty".to_string(),
        }
        .into());
    }

    require_validate_pass(&ctx.project_root, args.force)?;

    let current = parse_release_version(&package.version, "[package].version")?;
    let next = resolve_next_version(&args.bump, &current)?;

    if next == current {
        return Err(ConfigError::Invalid {
            message: format!(
                "new version `{}` matches current version `{}`",
                next, package.version
            ),
        }
        .into());
    }

    let next_version = next.to_string();
    let tag = format!("v{next_version}");

    ensure_tag_not_exists(&ctx.project_root, &tag)?;

    let snapshot = ReleaseSnapshot::capture(&ctx.project_root)?;
    let review_filled_changelog = !json
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal();
    let release_result = (|| {
        update_changelog_if_present(&ctx.project_root, &next_version, review_filled_changelog)?;

        update_package_version(&ctx.project_root, &next_version)?;

        crate::platform::process::run_git(
            &["add", "mars.toml"],
            &ctx.project_root,
            "git add mars.toml",
        )?;
        if snapshot.changelog.is_some() {
            crate::platform::process::run_git(
                &["add", "CHANGELOG.md"],
                &ctx.project_root,
                "git add CHANGELOG.md",
            )?;
        }
        crate::platform::process::run_git(
            &["commit", "-m", &tag],
            &ctx.project_root,
            &format!("git commit -m {tag}"),
        )?;
        crate::platform::process::run_git(
            &["tag", "-a", &tag, "-m", &tag],
            &ctx.project_root,
            &format!("git tag -a {tag} -m {tag}"),
        )?;

        Ok::<(), MarsError>(())
    })();

    if let Err(error) = release_result {
        if let Err(rollback) = snapshot.rollback(&ctx.project_root) {
            return Err(MarsError::ReleaseRollback {
                release: Box::new(error),
                rollback,
            });
        }
        return Err(error);
    }

    if args.push {
        let branch = current_branch(&ctx.project_root)?;
        crate::platform::process::run_git(
            &["push", "origin", &branch],
            &ctx.project_root,
            &format!("git push origin {branch}"),
        )?;
        crate::platform::process::run_git(
            &["push", "origin", &tag],
            &ctx.project_root,
            &format!("git push origin {tag}"),
        )?;
    }

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "version": next_version,
            "tag": tag,
            "pushed": args.push,
        }));
    } else {
        println!("{tag}");
    }

    Ok(0)
}

fn update_package_version(project_root: &Path, next_version: &str) -> Result<(), MarsError> {
    let path = project_root.join("mars.toml");
    let mut content = std::fs::read_to_string(&path)?;
    let (version_span, current_version) = {
        let document = toml_edit::ImDocument::parse(content.as_str()).map_err(|error| {
            ConfigError::Invalid {
                message: format!("failed to edit {}: {error}", path.display()),
            }
        })?;
        let version = document
            .get("package")
            .and_then(|package| package.get("version"))
            .and_then(toml_edit::Item::as_value)
            .ok_or_else(|| ConfigError::Invalid {
                message: "mars.toml must contain [package] with name and version".to_string(),
            })?;
        let current_version = version.as_str().ok_or_else(|| ConfigError::Invalid {
            message: "[package].version must be a string".to_string(),
        })?;
        let span = version.span().ok_or_else(|| ConfigError::Invalid {
            message: "could not locate [package].version in mars.toml".to_string(),
        })?;
        (span, current_version.to_string())
    };

    // Splice the parsed value span instead of rendering the document: rendering would normalize
    // CRLF, while versioning must leave every byte outside the version value untouched.
    let raw_version = &content[version_span.clone()];
    if let Some(offset) = raw_version.find(&current_version) {
        let value_start = version_span.start + offset;
        content.replace_range(
            value_start..value_start + current_version.len(),
            next_version,
        );
    } else {
        content.replace_range(
            version_span,
            &toml_edit::Value::from(next_version).to_string(),
        );
    }

    crate::fs::atomic_write(&path, content.as_bytes())
}

fn require_clean_working_tree(project_root: &Path) -> Result<(), MarsError> {
    let output = crate::platform::process::run_git(
        &["status", "--porcelain"],
        project_root,
        "git status --porcelain",
    )?;

    if !output.is_empty() {
        return Err(ConfigError::Invalid {
            message: "working tree must be clean before running `mars version`".to_string(),
        }
        .into());
    }

    Ok(())
}

fn require_package_check(project_root: &Path, force: bool) -> Result<(), MarsError> {
    // Skip check if this isn't a source package (no agents/, skills/, or SKILL.md)
    let has_agents = project_root.join("agents").is_dir();
    let has_skills = project_root.join("skills").is_dir();
    let has_root_skill = project_root.join("SKILL.md").is_file();
    if !has_agents && !has_skills && !has_root_skill {
        return Ok(());
    }

    match check::check_dir(project_root) {
        Ok(report) if report.errors.is_empty() => Ok(()),
        Ok(report) if force => {
            for error in &report.errors {
                eprintln!("warning (--force): {error}");
            }
            Ok(())
        }
        Ok(report) => {
            let mut message = "package check failed:".to_string();
            for error in &report.errors {
                message.push_str(&format!("\n  - {error}"));
            }
            Err(ConfigError::Invalid { message }.into())
        }
        Err(e) if force => {
            eprintln!("warning (--force): check failed: {e}");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Run the full sync pipeline in dry-run mode and fail on errors.
///
/// This catches integration issues that `mars check` cannot see:
/// dependency resolution failures, cross-package skill graph inconsistencies,
/// and target generation errors.
fn require_validate_pass(project_root: &Path, force: bool) -> Result<(), MarsError> {
    let ctx = MarsContext {
        project_root: project_root.to_path_buf(),
        meridian_managed: false,
    };
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions {
            dry_run: true,
            ..SyncOptions::default()
        },
        recovery: Default::default(),
        lossiness_mode: crate::diagnostic::LossinessMode::Hidden,
    };

    match crate::sync::execute(&ctx, &request) {
        Ok(report)
            if report
                .diagnostics
                .iter()
                .all(|d| d.level != DiagnosticLevel::Error) =>
        {
            Ok(())
        }
        Ok(report) if force => {
            for d in &report.diagnostics {
                if d.level == DiagnosticLevel::Error {
                    eprintln!("warning (--force): [{}] {}", d.code, d.message);
                }
            }
            Ok(())
        }
        Ok(report) => {
            let mut message = "validate failed:".to_string();
            for d in &report.diagnostics {
                if d.level == DiagnosticLevel::Error {
                    message.push_str(&format!("\n  - [{}] {}", d.code, d.message));
                }
            }
            Err(ConfigError::Invalid { message }.into())
        }
        Err(e) if force => {
            eprintln!("warning (--force): validate failed: {e}");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn parse_release_version(value: &str, field_name: &str) -> Result<Version, MarsError> {
    let version = Version::parse(value).map_err(|_| ConfigError::Invalid {
        message: format!("{field_name} must be valid semver (X.Y.Z), got `{value}`"),
    })?;

    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err(ConfigError::Invalid {
            message: format!("{field_name} must be plain X.Y.Z (no prerelease/build): `{value}`"),
        }
        .into());
    }

    Ok(version)
}

fn resolve_next_version(bump: &str, current: &Version) -> Result<Version, MarsError> {
    match bump {
        "patch" => Ok(Version {
            major: current.major,
            minor: current.minor,
            patch: current
                .patch
                .checked_add(1)
                .ok_or_else(|| ConfigError::Invalid {
                    message: "patch version overflow".to_string(),
                })?,
            pre: Prerelease::EMPTY,
            build: BuildMetadata::EMPTY,
        }),
        "minor" => Ok(Version {
            major: current.major,
            minor: current
                .minor
                .checked_add(1)
                .ok_or_else(|| ConfigError::Invalid {
                    message: "minor version overflow".to_string(),
                })?,
            patch: 0,
            pre: Prerelease::EMPTY,
            build: BuildMetadata::EMPTY,
        }),
        "major" => Ok(Version {
            major: current
                .major
                .checked_add(1)
                .ok_or_else(|| ConfigError::Invalid {
                    message: "major version overflow".to_string(),
                })?,
            minor: 0,
            patch: 0,
            pre: Prerelease::EMPTY,
            build: BuildMetadata::EMPTY,
        }),
        explicit => parse_release_version(explicit, "requested version"),
    }
}

fn ensure_tag_not_exists(project_root: &Path, tag: &str) -> Result<(), MarsError> {
    let output = crate::platform::process::run_git(
        &["tag", "--list", tag],
        project_root,
        &format!("git tag --list {tag}"),
    )?;

    let exists = output.lines().any(|line| line.trim() == tag);

    if exists {
        return Err(ConfigError::Invalid {
            message: format!("tag `{tag}` already exists"),
        }
        .into());
    }

    Ok(())
}

fn current_branch(project_root: &Path) -> Result<String, MarsError> {
    let branch = crate::platform::process::run_git(
        &["rev-parse", "--abbrev-ref", "HEAD"],
        project_root,
        "git rev-parse --abbrev-ref HEAD",
    )?;
    if branch.is_empty() || branch == "HEAD" {
        return Err(ConfigError::Invalid {
            message: "cannot push from detached HEAD".to_string(),
        }
        .into());
    }

    Ok(branch)
}

struct ReleaseSnapshot {
    head: String,
    mars_toml: Vec<u8>,
    changelog: Option<Vec<u8>>,
}

impl ReleaseSnapshot {
    fn capture(project_root: &Path) -> Result<Self, MarsError> {
        let head = crate::platform::process::run_git(
            &["rev-parse", "HEAD"],
            project_root,
            "capture HEAD before release",
        )?;
        let mars_toml = std::fs::read(project_root.join("mars.toml"))?;
        let changelog_path = project_root.join("CHANGELOG.md");
        let changelog = changelog_path
            .is_file()
            .then(|| std::fs::read(changelog_path))
            .transpose()?;

        Ok(Self {
            head,
            mars_toml,
            changelog,
        })
    }

    fn rollback(&self, project_root: &Path) -> Result<(), String> {
        let mut failures = Vec::new();

        match crate::platform::process::run_git(
            &["rev-parse", "HEAD"],
            project_root,
            "read HEAD while rolling back release",
        ) {
            Ok(head) if head != self.head => {
                if let Err(error) = crate::platform::process::run_git(
                    &["reset", "--soft", &self.head],
                    project_root,
                    "roll back release commits",
                ) {
                    failures.push(error.to_string());
                }
            }
            Ok(_) => {}
            Err(error) => failures.push(error.to_string()),
        }

        if let Err(error) = restore_snapshot_file(&project_root.join("mars.toml"), &self.mars_toml)
        {
            failures.push(format!("could not restore mars.toml: {error}"));
        }
        if let Some(changelog) = &self.changelog
            && let Err(error) = restore_snapshot_file(&project_root.join("CHANGELOG.md"), changelog)
        {
            failures.push(format!("could not restore CHANGELOG.md: {error}"));
        }

        if let Err(error) = crate::platform::process::run_git(
            &["reset", "-q", "--", "mars.toml", "CHANGELOG.md"],
            project_root,
            "unstage release files",
        ) {
            failures.push(error.to_string());
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

fn restore_snapshot_file(path: &Path, snapshot: &[u8]) -> Result<(), MarsError> {
    if matches!(std::fs::read(path), Ok(current) if current == snapshot) {
        return Ok(());
    }

    crate::fs::atomic_write(path, snapshot)
}

fn update_changelog_if_present(
    project_root: &Path,
    next_version: &str,
    review_filled_section: bool,
) -> Result<(), MarsError> {
    let changelog_path = project_root.join("CHANGELOG.md");
    if !changelog_path.is_file() {
        return Ok(());
    }

    let content = std::fs::read(&changelog_path)?;
    let content = std::str::from_utf8(&content)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let Some(changelog) = UnreleasedChangelog::parse(content) else {
        return Ok(());
    };

    let commit_bullets = if changelog.is_empty() {
        match changelog_commit_bullets(project_root) {
            Ok(bullets) => bullets,
            Err(error) => {
                eprintln!("warning: could not collect commits for CHANGELOG.md: {error}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    if !commit_bullets.is_empty() {
        if review_filled_section {
            eprintln!(
                "note: filled CHANGELOG.md release section from {} commits; opening editor for review",
                commit_bullets.len()
            );
        } else {
            eprintln!(
                "note: filled CHANGELOG.md release section from {} commits; section lists raw commit subjects",
                commit_bullets.len()
            );
        }
    } else if changelog.is_empty() {
        eprintln!("warning: CHANGELOG.md has no entries under [Unreleased]");
    }

    let updated = changelog.promote(next_version, &today_iso_date(), &commit_bullets);
    crate::fs::atomic_write(&changelog_path, updated.as_bytes())?;

    if review_filled_section && !commit_bullets.is_empty() {
        open_changelog_in_git_editor(project_root, &changelog_path)?;

        let edited = std::fs::read_to_string(&changelog_path)?;
        if !release_section_has_entries(&edited, next_version) {
            return Err(MarsError::Release {
                message: format!(
                    "CHANGELOG.md must retain a non-empty `## [{next_version}]` release section"
                ),
            });
        }
    }

    Ok(())
}

fn open_changelog_in_git_editor(
    project_root: &Path,
    changelog_path: &Path,
) -> Result<(), MarsError> {
    let editor = crate::platform::process::run_git(
        &["var", "GIT_EDITOR"],
        project_root,
        "git var GIT_EDITOR",
    )?;

    // A one-shot shell alias delegates both POSIX shell quoting and Git for Windows' bundled
    // `sh` selection to Git itself. The function wrapper passes the path exactly once.
    let editor_alias = format!("alias.mars-changelog-editor=!f() {{ {editor} \"$@\"; }}; f");
    let mut command = Command::new("git");
    crate::platform::process::remove_git_local_env(&mut command);
    let status = command
        .current_dir(project_root)
        .arg("-c")
        .arg(editor_alias)
        .arg("mars-changelog-editor")
        .arg(changelog_path)
        .status()
        .map_err(|error| MarsError::GitCli {
            command: "git editor".to_string(),
            message: format!("failed to launch editor: {error}"),
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(MarsError::Release {
            message: format!("editor exited unsuccessfully ({status})"),
        })
    }
}

fn changelog_commit_bullets(project_root: &Path) -> Result<Vec<String>, MarsError> {
    let previous_tag = match crate::platform::process::run_git(
        &[
            "describe",
            "--tags",
            "--abbrev=0",
            "--match",
            "v*",
            "--exclude",
            "v*-*",
        ],
        project_root,
        "git describe latest stable release tag",
    ) {
        Ok(tag) => tag,
        Err(MarsError::GitCli { message, .. })
            if ["no names found", "no tags can describe"]
                .iter()
                .any(|text| message.to_ascii_lowercase().contains(text)) =>
        {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    let range = format!("{previous_tag}..HEAD");
    let subjects = crate::platform::process::run_git(
        &[
            "log",
            "--format=%s",
            "--no-merges",
            "--reverse",
            &range,
            "--",
            ".",
        ],
        project_root,
        "git log package commits since latest stable release",
    )?;

    Ok(subjects
        .lines()
        .filter(|subject| !is_release_commit_subject(subject))
        .map(|subject| format!("- {subject}"))
        .collect())
}

fn is_release_commit_subject(subject: &str) -> bool {
    if subject.starts_with("release:") {
        return true;
    }

    subject
        .strip_prefix('v')
        .and_then(|version| Version::parse(version).ok())
        .is_some()
}

struct UnreleasedChangelog<'a> {
    sections: Vec<&'a str>,
    unreleased_index: usize,
    next_section_index: usize,
    empty: bool,
}

impl<'a> UnreleasedChangelog<'a> {
    fn parse(content: &'a str) -> Option<Self> {
        let sections = content.split_inclusive('\n').collect::<Vec<_>>();
        let unreleased_index = sections
            .iter()
            .position(|line| is_unreleased_header(line.trim_end()))?;
        let next_section_index = sections
            .iter()
            .enumerate()
            .skip(unreleased_index + 1)
            .find_map(|(index, line)| {
                if line.trim_start().starts_with("## [") {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(sections.len());
        let empty = changelog_section_is_empty(&sections[unreleased_index + 1..next_section_index]);

        Some(Self {
            sections,
            unreleased_index,
            next_section_index,
            empty,
        })
    }

    fn is_empty(&self) -> bool {
        self.empty
    }

    fn promote(&self, next_version: &str, date: &str, empty_section_bullets: &[String]) -> String {
        let mut promoted = String::new();
        for line in &self.sections[..self.unreleased_index] {
            promoted.push_str(line);
        }
        promoted.push_str("## [Unreleased]\n\n");
        promoted.push_str(&format!("## [{next_version}] - {date}\n"));
        if self.empty && !empty_section_bullets.is_empty() {
            promoted.push('\n');
            for bullet in empty_section_bullets {
                promoted.push_str(bullet);
                promoted.push('\n');
            }
            promoted.push('\n');
            for line in &self.sections[self.next_section_index..] {
                promoted.push_str(line);
            }
        } else {
            for line in &self.sections[self.unreleased_index + 1..] {
                promoted.push_str(line);
            }
        }

        promoted
    }
}

fn is_unreleased_header(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("## [")
        && trimmed.ends_with(']')
        && trimmed
            .trim_start_matches("## [")
            .trim_end_matches(']')
            .eq_ignore_ascii_case("unreleased")
}

fn changelog_section_is_empty(lines: &[&str]) -> bool {
    lines.iter().all(|line| {
        let trimmed = line.trim();
        trimmed.is_empty() || trimmed.starts_with("###")
    })
}

fn release_section_has_entries(content: &str, version: &str) -> bool {
    let sections = content.split_inclusive('\n').collect::<Vec<_>>();
    let header = format!("## [{version}]");
    let Some(release_index) = sections.iter().position(|line| {
        let line = line.trim();
        line.strip_prefix(&header)
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(" - "))
    }) else {
        return false;
    };
    let next_section_index = sections
        .iter()
        .enumerate()
        .skip(release_index + 1)
        .find_map(|(index, line)| line.trim_start().starts_with("## [").then_some(index))
        .unwrap_or(sections.len());

    !changelog_section_is_empty(&sections[release_index + 1..next_section_index])
}

fn today_iso_date() -> String {
    let days_since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400;
    civil_date_from_days(days_since_epoch as i64)
}

fn civil_date_from_days(days_since_unix_epoch: i64) -> String {
    // Howard Hinnant's civil-from-days algorithm. Converts days since
    // 1970-01-01 to a proleptic Gregorian date without platform-specific APIs.
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };

    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    fn run_git_test<I, S>(cwd: &Path, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("git");
        crate::platform::process::remove_git_local_env(&mut command);
        command.env("GIT_AUTHOR_NAME", "Mars Test");
        command.env("GIT_AUTHOR_EMAIL", "mars@example.com");
        command.env("GIT_COMMITTER_NAME", "Mars Test");
        command.env("GIT_COMMITTER_EMAIL", "mars@example.com");
        let output = command.current_dir(cwd).args(args).output().unwrap();
        if !output.status.success() {
            panic!(
                "git command failed: {}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo_with_mars_toml(mars_toml: &str) -> (TempDir, super::super::MarsContext) {
        let repo = TempDir::new().unwrap();
        run_git_test(repo.path(), ["init", "."]);
        run_git_test(repo.path(), ["config", "user.name", "Mars Test"]);
        run_git_test(repo.path(), ["config", "user.email", "mars@example.com"]);

        std::fs::create_dir_all(repo.path().join(".agents")).unwrap();
        std::fs::create_dir_all(repo.path().join("agents")).unwrap();
        std::fs::write(
            repo.path().join("agents/test-agent.md"),
            "---\nname: test-agent\ndescription: test\n---\n# Test",
        )
        .unwrap();
        std::fs::write(repo.path().join("mars.toml"), mars_toml).unwrap();
        run_git_test(repo.path(), ["add", "."]);
        run_git_test(repo.path(), ["commit", "-m", "init"]);

        let ctx = super::super::MarsContext::for_test(repo.path().to_path_buf());
        (repo, ctx)
    }

    #[test]
    fn parse_release_version_accepts_plain_semver() {
        let parsed = parse_release_version("1.2.3", "field").unwrap();
        assert_eq!(parsed.to_string(), "1.2.3");
    }

    #[test]
    fn parse_release_version_rejects_prerelease() {
        let err = parse_release_version("1.2.3-alpha.1", "field").unwrap_err();
        assert!(err.to_string().contains("plain X.Y.Z"));
    }

    #[test]
    fn resolve_next_version_bump_kinds() {
        let current = Version::parse("1.2.3").unwrap();

        assert_eq!(
            resolve_next_version("patch", &current).unwrap().to_string(),
            "1.2.4"
        );
        assert_eq!(
            resolve_next_version("minor", &current).unwrap().to_string(),
            "1.3.0"
        );
        assert_eq!(
            resolve_next_version("major", &current).unwrap().to_string(),
            "2.0.0"
        );
    }

    #[test]
    fn resolve_next_version_explicit() {
        let current = Version::parse("1.2.3").unwrap();
        assert_eq!(
            resolve_next_version("4.5.6", &current).unwrap().to_string(),
            "4.5.6"
        );
    }

    #[test]
    fn run_patch_updates_version_commits_and_tags() {
        let (repo, ctx) = init_repo_with_mars_toml(
            "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
        );

        let args = VersionArgs {
            bump: "patch".to_string(),
            push: false,
            force: false,
        };

        let exit = run(&args, &ctx, true).unwrap();
        assert_eq!(exit, 0);
        assert!(!repo.path().join("CHANGELOG.md").exists());

        let config = crate::config::load(repo.path()).unwrap();
        assert_eq!(config.package.unwrap().version, "0.1.1");

        let subject = run_git_test(repo.path(), ["log", "-1", "--pretty=%s"]);
        assert_eq!(subject, "v0.1.1");

        let tag = run_git_test(repo.path(), ["tag", "--list", "v0.1.1"]);
        assert_eq!(tag, "v0.1.1");
    }

    #[test]
    fn run_promotes_unreleased_in_changelog() {
        let (repo, ctx) = init_repo_with_mars_toml(
            "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
        );
        std::fs::write(
            repo.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n### Added\n- New feature X\n\n### Fixed\n- Bug Y\n",
        )
        .unwrap();
        run_git_test(repo.path(), ["add", "CHANGELOG.md"]);
        run_git_test(repo.path(), ["commit", "-m", "add changelog"]);

        let args = VersionArgs {
            bump: "patch".to_string(),
            push: false,
            force: false,
        };

        let exit = run(&args, &ctx, true).unwrap();
        assert_eq!(exit, 0);

        let changelog = std::fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap();
        let today = today_iso_date();
        assert!(changelog.contains("## [Unreleased]\n\n## [0.1.1] - "));
        assert!(changelog.contains(&format!(
            "## [0.1.1] - {today}\n\n### Added\n- New feature X"
        )));
        assert!(changelog.contains("### Fixed\n- Bug Y"));

        let committed_files =
            run_git_test(repo.path(), ["show", "--name-only", "--pretty=", "HEAD"]);
        assert!(committed_files.lines().any(|line| line == "CHANGELOG.md"));
    }

    #[test]
    fn run_fills_empty_unreleased_from_commits() {
        let (repo, ctx) = init_repo_with_mars_toml(
            "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
        );
        run_git_test(repo.path(), ["tag", "-a", "v0.1.0", "-m", "v0.1.0"]);
        std::fs::write(
            repo.path().join("CHANGELOG.md"),
            "# Changelog\n\n## [Unreleased]\n\n### Added\n\n### Fixed\n",
        )
        .unwrap();
        run_git_test(repo.path(), ["add", "CHANGELOG.md"]);
        run_git_test(repo.path(), ["commit", "-m", "add empty changelog"]);

        let args = VersionArgs {
            bump: "patch".to_string(),
            push: false,
            force: false,
        };

        let exit = run(&args, &ctx, true).unwrap();
        assert_eq!(exit, 0);

        let changelog = std::fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap();
        assert!(changelog.contains("## [Unreleased]\n\n## [0.1.1] - "));
        assert!(changelog.contains("\n\n- add empty changelog\n"));
        assert!(!changelog.contains("### Added"));
        let parsed =
            UnreleasedChangelog::parse("# Changelog\n\n## [Unreleased]\n\n### Added\n\n").unwrap();
        assert!(parsed.is_empty());
        assert!(
            parsed
                .promote(
                    "0.1.1",
                    "2026-04-30",
                    &["- add empty changelog".to_string()]
                )
                .contains("## [0.1.1] - 2026-04-30\n\n- add empty changelog")
        );
    }

    #[test]
    fn run_changelog_preserves_existing_versions() {
        let (repo, ctx) = init_repo_with_mars_toml(
            "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
        );
        let prior_section = "## [0.1.0] - 2026-04-01\n\n### Added\n- Initial release\n";
        std::fs::write(
            repo.path().join("CHANGELOG.md"),
            format!("# Changelog\n\n## [Unreleased]\n\n### Fixed\n- Bug Y\n\n{prior_section}"),
        )
        .unwrap();
        run_git_test(repo.path(), ["add", "CHANGELOG.md"]);
        run_git_test(repo.path(), ["commit", "-m", "add changelog"]);

        let args = VersionArgs {
            bump: "patch".to_string(),
            push: false,
            force: false,
        };

        let exit = run(&args, &ctx, true).unwrap();
        assert_eq!(exit, 0);

        let changelog = std::fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap();
        assert!(changelog.contains("## [0.1.1] - "));
        assert!(changelog.contains("### Fixed\n- Bug Y"));
        assert!(changelog.ends_with(prior_section));
    }

    #[test]
    fn run_requires_clean_working_tree() {
        let (repo, ctx) = init_repo_with_mars_toml(
            "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
        );
        std::fs::write(repo.path().join("dirty.txt"), "dirty\n").unwrap();

        let args = VersionArgs {
            bump: "patch".to_string(),
            push: false,
            force: false,
        };

        let err = run(&args, &ctx, true).unwrap_err();
        assert!(err.to_string().contains("working tree must be clean"));

        let config = crate::config::load(repo.path()).unwrap();
        assert_eq!(config.package.unwrap().version, "0.1.0");
    }

    #[test]
    fn run_requires_package_section() {
        let (_repo, ctx) =
            init_repo_with_mars_toml("[dependencies]\nbase = { path = \"../base\" }\n");

        let args = VersionArgs {
            bump: "patch".to_string(),
            push: false,
            force: false,
        };

        let err = run(&args, &ctx, true).unwrap_err();
        assert!(err.to_string().contains("must contain [package]"));
    }

    #[test]
    fn run_rejects_existing_tag() {
        let (repo, ctx) = init_repo_with_mars_toml(
            "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
        );
        run_git_test(repo.path(), ["tag", "-a", "v0.1.1", "-m", "v0.1.1"]);

        let args = VersionArgs {
            bump: "patch".to_string(),
            push: false,
            force: false,
        };

        let err = run(&args, &ctx, true).unwrap_err();
        assert!(err.to_string().contains("tag `v0.1.1` already exists"));
    }

    #[test]
    fn run_with_push_pushes_branch_and_tag_to_origin() {
        let (repo, ctx) = init_repo_with_mars_toml(
            "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
        );

        let remote = TempDir::new().unwrap();
        run_git_test(remote.path(), ["init", "--bare", "."]);
        run_git_test(
            repo.path(),
            ["remote", "add", "origin", remote.path().to_str().unwrap()],
        );

        let args = VersionArgs {
            bump: "patch".to_string(),
            push: true,
            force: false,
        };

        let exit = run(&args, &ctx, true).unwrap();
        assert_eq!(exit, 0);

        let branch = run_git_test(repo.path(), ["rev-parse", "--abbrev-ref", "HEAD"]);
        let remote_branch = run_git_test(repo.path(), ["ls-remote", "--heads", "origin", &branch]);
        assert!(remote_branch.contains(&format!("refs/heads/{branch}")));

        let remote_tag = run_git_test(repo.path(), ["ls-remote", "--tags", "origin", "v0.1.1"]);
        assert!(remote_tag.contains("refs/tags/v0.1.1"));
    }

    // ── P5: check errors abort version ───────────────────────────────────────────

    #[test]
    fn run_aborts_when_package_check_fails() {
        // P5: an unresolvable dependency causes check to fail, version is not bumped.
        let (repo, ctx) = init_repo_with_mars_toml(
            "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep = { path = \"/nonexistent-dep-xyz-p5\" }\n",
        );

        let args = VersionArgs {
            bump: "patch".to_string(),
            push: false,
            force: false,
        };

        let err = run(&args, &ctx, true).unwrap_err();
        assert!(
            err.to_string().contains("package check failed"),
            "expected package check failure: {err}"
        );

        let config = crate::config::load(repo.path()).unwrap();
        assert_eq!(
            config.package.unwrap().version,
            "0.1.0",
            "version must not be bumped after check failure"
        );
    }

    #[test]
    fn run_aborts_when_agent_model_policy_is_malformed() {
        let (repo, ctx) = init_repo_with_mars_toml(
            "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
        );
        std::fs::write(
            repo.path().join("agents/test-agent.md"),
            "---\nname: test-agent\ndescription: test\nmodel-policies:\n  - match:\n      alias: gpt55\n      model: gpt-5.5\n---\n# Test",
        )
        .unwrap();
        run_git_test(repo.path(), ["add", "agents/test-agent.md"]);
        run_git_test(repo.path(), ["commit", "-m", "malformed agent policy"]);

        let args = VersionArgs {
            bump: "patch".to_string(),
            push: false,
            force: false,
        };

        let err = run(&args, &ctx, true).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("package check failed") && message.contains("model-policies[1].match"),
            "expected model-policies package check failure: {message}"
        );

        let config = crate::config::load(repo.path()).unwrap();
        assert_eq!(
            config.package.unwrap().version,
            "0.1.0",
            "version must not be bumped after agent profile check failure"
        );
    }

    // ── P6: --force bypasses check errors ────────────────────────────────────────

    #[test]
    fn run_force_bypasses_package_check_errors() {
        // P6: --force proceeds despite check errors, emits warnings to stderr.
        let (repo, ctx) = init_repo_with_mars_toml(
            "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep = { path = \"/nonexistent-dep-xyz-p6\" }\n",
        );

        let args = VersionArgs {
            bump: "patch".to_string(),
            push: false,
            force: true,
        };

        let exit = run(&args, &ctx, true).unwrap();
        assert_eq!(exit, 0);

        let config = crate::config::load(repo.path()).unwrap();
        assert_eq!(
            config.package.unwrap().version,
            "0.1.1",
            "version must be bumped with --force"
        );
    }
}
