//! Discovery and validation of what a provider ships: skills, hooks and MCP servers.
//!
//! Hooks and MCP servers are validated by the readers sync uses, on a copy of only the entries
//! that qualify (`_x` and `.x` never reach sync), and that copy is what `provide` stages under
//! `.mars/provided/<provider>/`. Discovery only digests them; [`ProvidedEntries::stage`] makes the
//! copy, so a launch that changes nothing copies and parses nothing.
//!
//! `<dir>/skills/` is read one level deep. Entries are copied as authored — never normalized —
//! so the digest describes exactly what lands in `.mars/skills/` (each file's bytes and
//! executable bit) and a byte-equal directory can be adopted instead of replaced. The schema
//! reader is here only to refuse a skill the rest of Mars would silently drop; its warnings are
//! reported, not enforced.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::MarsError;
use mars_schema::provider::provider_of;
use mars_schema::skill::SkillProfile;

use super::spec::ProvideSpec;

#[derive(Debug)]
pub(crate) struct ProvidedSkill {
    pub name: String,
    pub dir: PathBuf,
    /// SHA-256 of this skill's tree: the manifest format package skills use, with each file's
    /// executable bit folded in.
    pub digest: String,
}

pub(crate) struct ProvidedEntries {
    pub skills: Vec<ProvidedSkill>,
    /// The hook and MCP directories that qualify, as `(hooks|mcp, name, source dir)`.
    staged_sources: Vec<(&'static str, String, PathBuf)>,
    /// The [`tree_digest`] the staged tree has once [`Self::stage`] copies the sources, `None`
    /// when the bundle has none.
    pub staged_digest: Option<String>,
    /// SHA-256 over `(name, skill digest)` pairs and the staged tree's digest: it moves when any
    /// skill's bytes, executable bits or the entry list do, and leaves the per-skill digests as
    /// the one description of a tree.
    pub digest: String,
}

/// The subdirectories of `<dir>/<kind>/` that are entries: not `_`/`.` plumbing (`__pycache__/`,
/// `AGENTS.md` beside Meridian's bundle), and a real directory, so a file or a symlink is left
/// out rather than copied through from outside the bundle. Sorted.
fn entry_names(kind_dir: &Path) -> Result<BTreeSet<String>, MarsError> {
    let mut names = BTreeSet::new();
    if !kind_dir.is_dir() {
        return Ok(names);
    }
    for entry in fs::read_dir(kind_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('_') || name.starts_with('.') {
            continue;
        }
        if entry.file_type()?.is_dir() {
            names.insert(name.to_string());
        }
    }
    Ok(names)
}

/// Fails before the caller writes anything when the source could not be installed as named:
/// a name that is not `<skill>__<provider>`, a `SKILL.md` the schema reader rejects, or a hook
/// or MCP server the readers sync uses reject. `skills/`, `hooks/` and `mcp/` are each optional.
pub(crate) fn discover(spec: &ProvideSpec) -> Result<(ProvidedEntries, Vec<String>), MarsError> {
    if !spec.package_dir.is_dir() {
        return Err(invalid_skill(format!(
            "{} is not a directory",
            spec.package_dir.display()
        )));
    }
    let skills_dir = spec.package_dir.join("skills");

    let mut warnings = Vec::new();
    let mut skills = Vec::new();
    for name in entry_names(&skills_dir)? {
        let dir = skills_dir.join(&name);
        let profile = read_skill(&name, &dir, &mut warnings)?;
        check_name(spec, &name, &profile)?;
        let digest = tree_digest(&dir)?;
        skills.push(ProvidedSkill { name, dir, digest });
    }
    let staged_sources = qualifying_sources(spec)?;
    let staged_digest = sources_digest(&staged_sources)?;

    let mut entries: Vec<(String, String)> = skills
        .iter()
        .map(|skill| (skill.name.clone(), skill.digest.clone()))
        .collect();
    if let Some(digest) = &staged_digest {
        // `/` cannot appear in a skill's directory name, so this key cannot collide with one.
        entries.push(("staged/".to_string(), digest.clone()));
    }
    Ok((
        ProvidedEntries {
            digest: crate::hash::hash_file_manifest(entries),
            skills,
            staged_sources,
            staged_digest,
        },
        warnings,
    ))
}

impl ProvidedEntries {
    /// Copies the qualifying hooks and MCP servers into a scratch tree (`hooks/<name>/`,
    /// `mcp/<name>/`) and reads it as sync will; installing the tree is staging. `None` when the
    /// bundle has none. Fails before the caller writes anything if sync would reject an entry.
    pub(crate) fn stage(
        &self,
        spec: &ProvideSpec,
        warnings: &mut Vec<String>,
    ) -> Result<Option<tempfile::TempDir>, MarsError> {
        if self.staged_sources.is_empty() {
            return Ok(None);
        }
        let scratch = tempfile::tempdir()?;
        for (kind, name, source) in &self.staged_sources {
            crate::fs::copy_dir_filtered(source, &scratch.path().join(kind).join(name), &[])?;
        }
        validate_staged(scratch.path(), spec, warnings)?;
        Ok(Some(scratch))
    }
}

/// The hook and MCP directories that carry a manifest.
fn qualifying_sources(
    spec: &ProvideSpec,
) -> Result<Vec<(&'static str, String, PathBuf)>, MarsError> {
    let mut sources = Vec::new();
    for (kind, manifest) in [("hooks", "hook.toml"), ("mcp", "mcp.toml")] {
        let kind_dir = spec.package_dir.join(kind);
        for name in entry_names(&kind_dir)? {
            let source = kind_dir.join(&name);
            if source.join(manifest).is_file() {
                sources.push((kind, name, source));
            }
        }
    }
    Ok(sources)
}

/// What [`tree_digest`] gives the scratch tree [`ProvidedEntries::stage`] copies the sources to.
fn sources_digest(sources: &[(&str, String, PathBuf)]) -> Result<Option<String>, MarsError> {
    if sources.is_empty() {
        return Ok(None);
    }
    let mut entries = Vec::new();
    for (kind, name, source) in sources {
        file_digests(source, &format!("{kind}/{name}/"), &mut entries)?;
    }
    Ok(Some(crate::hash::hash_file_manifest(entries)))
}

fn validate_staged(
    scratch: &Path,
    spec: &ProvideSpec,
    warnings: &mut Vec<String>,
) -> Result<(), MarsError> {
    let source_name = super::source::source_name(&spec.provider);
    let mut diag = crate::diagnostic::DiagnosticCollector::new();
    let hooks = crate::compiler::hooks::discover_hook_items(
        scratch,
        &source_name,
        crate::compiler::hooks::HookSource::Dependency {
            version: &spec.version,
        },
        1,
        0,
        &mut diag,
    )
    .map_err(|error| {
        invalid(
            "provided-hook-invalid",
            in_bundle(&crate::compiler::hooks::error_message(error), scratch, spec),
        )
    })?;
    // The dependency reader skips a hook that is not exported, so one that lacks it would install
    // nowhere.
    if let Some(hook) = hooks
        .iter()
        .find(|hook| hook.def.visibility != mars_schema::visibility::Visibility::Exported)
    {
        return Err(invalid(
            "provided-hook-invalid",
            format!(
                "hook `{}` must set `visibility = \"exported\"` in its hook.toml",
                hook.def.name
            ),
        ));
    }
    // The host reads its own bundle's paths, so the server is read as the host's, not the
    // staged source sync will know it by.
    crate::compiler::mcp::discover_mcp_items(scratch, &spec.provider, 1, 0, &mut diag).map_err(
        |error| {
            invalid(
                "provided-mcp-invalid",
                in_bundle(&error.to_string(), scratch, spec),
            )
        },
    )?;
    warnings.extend(
        diag.drain()
            .into_iter()
            .map(|diagnostic| diagnostic.message),
    );
    Ok(())
}

/// An install is bytes plus the mode the copy path gives them (`0o644` and the source's execute
/// bits), so freshness must see an execute-bit change: a host that fixes a script's execute bit needs a
/// replacement without a dummy content edit. "Executable" is
/// [`crate::platform::fs::is_executable`], the same notion the copy path installs by.
///
/// Strict: a directory holding a symlink or a special file is not a tree Mars installed, so a
/// caller comparing a destination reads the error as "not byte-equal" rather than failing.
pub(crate) fn tree_digest(dir: &Path) -> Result<String, MarsError> {
    let mut entries = Vec::new();
    file_digests(dir, "", &mut entries)?;
    Ok(crate::hash::hash_file_manifest(entries))
}

/// The files of `dir` as `(prefix + relative path, hash)`, an execute bit folded into the hash.
fn file_digests(
    dir: &Path,
    prefix: &str,
    entries: &mut Vec<(String, String)>,
) -> Result<(), MarsError> {
    for (relative, hash) in crate::hash::strict_file_hashes(dir, &[])? {
        let metadata = fs::symlink_metadata(dir.join(&relative))?;
        let hash = if crate::platform::fs::is_executable(&metadata) {
            format!("{hash} executable")
        } else {
            hash
        };
        entries.push((format!("{prefix}{relative}"), hash));
    }
    Ok(())
}

fn read_skill(
    name: &str,
    dir: &Path,
    warnings: &mut Vec<String>,
) -> Result<SkillProfile, MarsError> {
    let path = dir.join("SKILL.md");
    let content = fs::read_to_string(&path)
        .map_err(|error| invalid_skill(format!("{} is unreadable: {error}", path.display())))?;
    let reading = mars_schema::skill::read(&content);
    let errors = reading
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.is_error())
        .map(|diagnostic| diagnostic.message().to_string())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(invalid_skill(format!(
            "{}/SKILL.md is invalid: {}",
            name,
            errors.join("; ")
        )));
    }
    warnings.extend(
        reading
            .diagnostics
            .iter()
            .filter(|diagnostic| !diagnostic.is_error())
            .map(|diagnostic| format!("{name}/SKILL.md: {}", diagnostic.message())),
    );
    reading.fields.ok_or_else(|| {
        invalid_skill(format!(
            "{}/SKILL.md could not be read as a skill definition",
            name
        ))
    })
}

/// A skill's directory name is the name agents list, so it carries the provider suffix.
fn check_name(spec: &ProvideSpec, name: &str, profile: &SkillProfile) -> Result<(), MarsError> {
    let invalid_name = |message: String| MarsError::InvalidRequest {
        message: format!("provided-name-invalid: {message}"),
    };
    match provider_of(name) {
        Some(provider) if provider == spec.provider => {}
        Some(provider) => {
            return Err(invalid_name(format!(
                "skill `{name}` belongs to provider `{provider}`, not `{}`",
                spec.provider
            )));
        }
        None => {
            return Err(invalid_name(format!(
                "skill `{name}` must be named `<skill>__{}`",
                spec.provider
            )));
        }
    }
    if profile.name.as_deref() != Some(name) {
        return Err(invalid_name(format!(
            "skill `{name}` has `name: {}` in SKILL.md; it must equal its directory name",
            profile.name.as_deref().unwrap_or("(missing)")
        )));
    }
    Ok(())
}

/// The readers name files in the scratch copy; the host knows the bundle.
fn in_bundle(message: &str, scratch: &Path, spec: &ProvideSpec) -> String {
    message.replace(
        &scratch.display().to_string(),
        &spec.package_dir.display().to_string(),
    )
}

fn invalid_skill(message: String) -> MarsError {
    invalid("provided-skill-invalid", message)
}

fn invalid(code: &str, message: String) -> MarsError {
    MarsError::InvalidRequest {
        message: format!("{code}: {message}"),
    }
}
