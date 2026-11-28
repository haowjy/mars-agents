use std::collections::HashSet;
use std::path::Path;

use crate::diagnostic::{Diagnostic, DiagnosticLevel, Owner};
use crate::error::MarsError;

/// The callers read files Mars or a provider wrote — an installed Agent under `.mars/`. Returns the `skills` list, or an empty vec if there is no
/// frontmatter, no `skills` field, or malformed YAML, and reads only the frontmatter block
/// between `---` delimiters, not the full markdown body.
pub fn parse_agent_skills(agent_path: &Path) -> Result<Vec<String>, MarsError> {
    let content = std::fs::read_to_string(agent_path)?;
    Ok(mars_schema::agent::read(&content)
        .fields
        .map(|profile| profile.skills.all())
        .unwrap_or_default())
}

/// What an Agent's skill ref needs checked: `declared` is the `uses-providers` of the Agent's
/// source, `Some` exactly when that source has a `[package]` table; a source without one (a
/// consumer project's own items, the user level's `.mars-src`) needs no declaration.
/// `provided_here` is whether a host makes the name available on this machine
/// (`sync::layers::HostSkills`).
pub(crate) struct SkillRef<'a> {
    pub agent: &'a str,
    pub skill_name: &'a str,
    pub owner: Owner<'a>,
    pub declared: Option<&'a [String]>,
    pub provided_here: bool,
}

/// The report for one skill ref the installed skills do not satisfy, or `None` when they do.
/// The one place a ref is judged, so sync, validate and plugin builds word it alike.
pub(crate) fn classify_skill_ref(
    reference: &SkillRef<'_>,
    available_skills: &HashSet<String>,
) -> Option<Diagnostic> {
    let SkillRef {
        agent,
        skill_name,
        owner,
        declared,
        provided_here,
    } = *reference;
    if available_skills.contains(skill_name) {
        return None;
    }
    let own_level = match owner {
        Owner::Own { .. } | Owner::Path { .. } => DiagnosticLevel::Warning,
        Owner::Dependency(_) => DiagnosticLevel::Info,
    };
    let whose = owner.whose();
    Some(match mars_schema::provider::provider_of(skill_name) {
        // The package declares the provider: the ref is valid wherever it supplies the skill, and
        // says so where it does not.
        Some(provider)
            if mars_schema::provider::deferred_provider(skill_name, declared).is_some() =>
        {
            if provided_here {
                return None;
            }
            Diagnostic::new(
                DiagnosticLevel::Warning,
                "provided-skill-unavailable",
                format!(
                    "agent `{agent}`{whose} references skill `{skill_name}`, which provider \
                     `{provider}` supplies; it is not provided here, so the agent runs without it"
                ),
            )
        }
        // Provided here. An item whose source has no `[package]` table (a consumer project's own,
        // the user level's) needs nothing declared, so the ref is satisfied; a package that does not declare the provider
        // would not resolve it for consumers.
        Some(provider) if provided_here => {
            declared?;
            Diagnostic::new(
                own_level,
                "provided-skill-undeclared",
                format!(
                    "agent `{agent}`{whose} references skill `{skill_name}`, provided here by \
                     `{provider}`, but this package's `[package] uses-providers` does not list \
                     `{provider}`; consumers will not resolve it"
                ),
            )
        }
        _ => {
            let suggestion = find_suggestion(skill_name, available_skills)
                .map_or(String::new(), |name| format!(" (did you mean `{name}`?)"));
            let fix = match owner {
                Owner::Dependency(source) => format!(
                    "; its author has to ship the skill, and {} takes a newer release",
                    crate::diagnostic::newer_release(source)
                ),
                _ if !suggestion.is_empty() => String::new(),
                Owner::Own { dir } | Owner::Path { dir, .. } => {
                    match mars_schema::provider::provider_of(skill_name) {
                        // A package's consumers resolve the ref only if it is declared, as
                        // `mars check` says.
                        Some(provider) if declared.is_some() => format!(
                            "; {}",
                            mars_schema::provider::undeclared_provider_hint(provider)
                        ),
                        Some(provider) => format!(
                            "; no package ships it, and `{provider}` has not provided it here"
                        ),
                        None => format!(
                            "; create it in {}, or remove it from the agent's `skills`",
                            dir.join("skills").join(skill_name).display()
                        ),
                    }
                }
            };
            // The agent runs without a skill it declares, so this is a warning whoever shipped it.
            Diagnostic::new(
                DiagnosticLevel::Warning,
                "missing-skill",
                format!(
                    "agent `{agent}`{whose} references missing skill `{skill_name}`{suggestion}{fix}"
                ),
            )
        }
    })
}

pub(crate) fn find_suggestion(missing: &str, available: &HashSet<String>) -> Option<String> {
    let missing_lower = missing.to_lowercase();

    let mut candidates: Vec<&String> = available.iter().collect();
    candidates.sort();

    for name in candidates {
        let name_lower = name.to_lowercase();
        if name_lower.contains(&missing_lower) || missing_lower.contains(&name_lower) {
            return Some(name.clone());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_agent(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(format!("{name}.md"));
        fs::write(&path, content).unwrap();
        path
    }
    #[test]
    fn parse_agent_skills_reads_frontmatter() {
        let dir = TempDir::new().unwrap();
        let path = write_agent(
            dir.path(),
            "coder",
            "---\nskills:\n  - planning\n  - review\n---\n# Coder\n",
        );

        let skills = parse_agent_skills(&path).unwrap();
        assert_eq!(skills, vec!["planning", "review"]);
    }
    #[test]
    fn suggestion_reverse_substring() {
        let available: HashSet<String> = ["planning"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            find_suggestion("plan", &available),
            Some("planning".to_string())
        );
    }

    #[test]
    fn suggestion_forward_substring() {
        let available: HashSet<String> = ["review"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            find_suggestion("review-pr", &available),
            Some("review".to_string())
        );
    }

    #[test]
    fn suggestion_case_insensitive() {
        let available: HashSet<String> = ["Planning"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            find_suggestion("plan", &available),
            Some("Planning".to_string())
        );
    }

    #[test]
    fn no_suggestion_when_no_match() {
        let available: HashSet<String> = ["review"].iter().map(|s| s.to_string()).collect();
        assert_eq!(find_suggestion("completely-different", &available), None);
    }
}
