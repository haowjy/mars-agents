//! Discovery and validation of the skills a provider ships.
//!
//! `<dir>/skills/` is read one level deep. Entries are copied as authored — never normalized —
//! so the digest describes exactly the bytes that land in `.mars/skills/` and a byte-equal
//! directory can be adopted instead of replaced. The schema reader is here only to refuse a
//! skill the rest of Mars would silently drop; its warnings are reported, not enforced.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::MarsError;
use mars_schema::provider::provider_of;
use mars_schema::skill::SkillProfile;

use super::spec::ProvideSpec;

/// One skill directory, with the file digests that define its bytes.
#[derive(Debug)]
pub(crate) struct ProvidedSkill {
    pub name: String,
    pub dir: PathBuf,
    /// SHA-256 of this skill's tree, in the same format package skills use.
    pub digest: String,
    files: BTreeMap<String, String>,
}

/// Everything one `mars provide` call offers, validated and digested.
pub(crate) struct ProvidedEntries {
    pub skills: Vec<ProvidedSkill>,
    /// SHA-256 over `<name>/<path>` entries: it moves when any skill's bytes or the entry list do.
    pub digest: String,
}

/// Read, validate and digest `<dir>/skills/`, with the warnings its readers produced.
///
/// Fails before the caller writes anything when the source could not be installed as named:
/// a name that is not `<skill>__<provider>`, or a `SKILL.md` the schema reader rejects.
pub(crate) fn discover(spec: &ProvideSpec) -> Result<(ProvidedEntries, Vec<String>), MarsError> {
    let skills_dir = spec.package_dir.join("skills");
    if !skills_dir.is_dir() {
        return Err(invalid_skill(format!(
            "{} has no `skills/` directory",
            spec.package_dir.display()
        )));
    }

    let mut names = BTreeSet::new();
    for entry in fs::read_dir(&skills_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        // `_`/`.` entries are package plumbing, not skills: `__init__.py`, `__pycache__/`,
        // `AGENTS.md` beside Meridian's bundle. Only a real subdirectory is a skill entry: a
        // file or a symlink is left out rather than copied through from outside the bundle.
        if name.starts_with('_') || name.starts_with('.') {
            continue;
        }
        if entry.file_type()?.is_dir() {
            names.insert(name.to_string());
        }
    }

    let mut warnings = Vec::new();
    let mut skills = Vec::new();
    for name in names {
        let dir = skills_dir.join(&name);
        let profile = read_skill(&name, &dir, &mut warnings)?;
        check_name(spec, &name, &profile)?;
        let files = crate::hash::strict_file_hashes(&dir, &[])?;
        let digest = crate::hash::hash_file_manifest(files.clone().into_iter().collect());
        skills.push(ProvidedSkill {
            name,
            dir,
            digest,
            files,
        });
    }

    let mut entries = Vec::new();
    for skill in &skills {
        for (path, hash) in &skill.files {
            entries.push((format!("{}/{path}", skill.name), hash.clone()));
        }
    }
    Ok((
        ProvidedEntries {
            digest: crate::hash::hash_file_manifest(entries),
            skills,
        },
        warnings,
    ))
}

/// Read `SKILL.md` through the schema contract, failing on error-level diagnostics.
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

fn invalid_skill(message: String) -> MarsError {
    MarsError::InvalidRequest {
        message: format!("provided-skill-invalid: {message}"),
    }
}
