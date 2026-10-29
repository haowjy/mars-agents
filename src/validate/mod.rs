use std::collections::HashSet;
use std::path::Path;

use crate::error::MarsError;
use crate::lock::ItemId;

/// Warning from dependency validation.
///
/// Agents declare `skills: [X, Y]` in YAML frontmatter. After resolution,
/// every referenced skill must exist somewhere in the target state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationWarning {
    /// An agent references a skill that doesn't exist in target state.
    MissingSkill {
        agent: ItemId,
        skill_name: String,
        /// Fuzzy match suggestion: "did you mean X?"
        suggestion: Option<String>,
    },
    /// An agent references `x__p`, its package declares provider `p`, and `p` has not provided
    /// the skill here. Sync continues: the package works wherever the provider supplies it.
    ProvidedSkillUnavailable {
        agent: ItemId,
        skill_name: String,
        provider: String,
    },
    /// An agent in a package references a skill a provider has provided here, but the package
    /// does not declare that provider, so consumers without it would not resolve the ref.
    ProvidedSkillUndeclared {
        agent: ItemId,
        skill_name: String,
        provider: String,
    },
}

impl ValidationWarning {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::MissingSkill { .. } => "missing-skill",
            Self::ProvidedSkillUnavailable { .. } => "provided-skill-unavailable",
            Self::ProvidedSkillUndeclared { .. } => "provided-skill-undeclared",
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            Self::MissingSkill {
                agent,
                skill_name,
                suggestion,
            } => match suggestion {
                Some(s) => format!(
                    "agent `{}` references missing skill `{skill_name}` (did you mean `{s}`?)",
                    agent.name
                ),
                None => format!(
                    "agent `{}` references missing skill `{skill_name}`",
                    agent.name
                ),
            },
            Self::ProvidedSkillUnavailable {
                agent,
                skill_name,
                provider,
            } => format!(
                "agent `{}` references skill `{skill_name}`, which provider `{provider}` supplies; \
                 it is not provided here, so the agent runs without it",
                agent.name
            ),
            Self::ProvidedSkillUndeclared {
                agent,
                skill_name,
                provider,
            } => format!(
                "agent `{}` references skill `{skill_name}`, provided here by `{provider}`, but this \
                 package's `[package] uses-providers` does not list `{provider}`; consumers will not resolve it",
                agent.name
            ),
        }
    }
}

/// Parse skill dependencies from an Agent file.
///
/// The callers read files Mars or a provider wrote — an installed Agent under `.mars/` — so the
/// read tolerates a pre-v1 spelling. Returns the `skills` list, or an empty vec if there is no
/// frontmatter, no `skills` field, or malformed YAML, and reads only the frontmatter block
/// between `---` delimiters, not the full markdown body.
pub fn parse_agent_skills(agent_path: &Path) -> Result<Vec<String>, MarsError> {
    let content = std::fs::read_to_string(agent_path)?;
    Ok(mars_schema::read::installed_agent(&content)
        .fields
        .map(|profile| profile.skills.all())
        .unwrap_or_default())
}

/// How one skill ref of an Agent is judged against the skills installed: `None` when it is
/// satisfied. The one rule `mars sync` and `mars doctor` share.
///
/// `declared` is the Agent's package's `uses-providers`: `Some` for a package, `None` for a
/// consumer project's own items, which need no declaration. `provided_here` is whether this
/// machine's `.mars/provided.lock` supplies the name.
pub(crate) fn classify_skill_ref(
    agent: ItemId,
    skill_name: String,
    available_skills: &HashSet<String>,
    declared: Option<&[String]>,
    provided_here: bool,
) -> Option<ValidationWarning> {
    if available_skills.contains(&skill_name) {
        return None;
    }
    Some(match mars_schema::provider::provider_of(&skill_name) {
        // The package declares the provider: the ref is valid wherever it supplies the skill, and
        // says so where it does not.
        Some(provider)
            if mars_schema::provider::deferred_provider(&skill_name, declared).is_some() =>
        {
            if provided_here {
                return None;
            }
            ValidationWarning::ProvidedSkillUnavailable {
                agent,
                provider: provider.to_string(),
                skill_name,
            }
        }
        // Provided here. A consumer project's own items have no `[package]` table, so nothing
        // needs declaring and the ref is satisfied; a package that does not declare the provider
        // would not resolve it for consumers.
        Some(provider) if provided_here => {
            declared?;
            ValidationWarning::ProvidedSkillUndeclared {
                agent,
                provider: provider.to_string(),
                skill_name,
            }
        }
        _ => ValidationWarning::MissingSkill {
            agent,
            suggestion: find_suggestion(&skill_name, available_skills),
            skill_name,
        },
    })
}

/// Find a suggestion for a missing skill using substring matching.
///
/// Checks if any available skill name contains the missing name as a
/// substring or vice versa. No edit distance library needed for v1.
pub(crate) fn find_suggestion(missing: &str, available: &HashSet<String>) -> Option<String> {
    let missing_lower = missing.to_lowercase();

    // Sort for deterministic suggestion when multiple match
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

    #[test]
    fn find_suggestion_matches_substrings_either_way_ignoring_case() {
        let suggest = |name: &str, available: &[&str]| {
            let available: HashSet<String> = available.iter().map(|s| s.to_string()).collect();
            find_suggestion(name, &available)
        };
        // "planning" contains "plan"
        assert_eq!(suggest("plan", &["planning"]), Some("planning".to_string()));
        // "review-pr" contains "review"
        assert_eq!(
            suggest("review-pr", &["review"]),
            Some("review".to_string())
        );
        assert_eq!(suggest("plan", &["Planning"]), Some("Planning".to_string()));
        assert_eq!(suggest("completely-different", &["review"]), None);
    }

    #[test]
    fn missing_skill_message_offers_the_suggestion() {
        use crate::lock::ItemId;
        use mars_schema::item::ItemKind;
        use mars_schema::names::ItemName;
        let missing = |suggestion: Option<&str>| ValidationWarning::MissingSkill {
            agent: ItemId {
                kind: ItemKind::Agent,
                name: ItemName::from("coder".to_string()),
            },
            skill_name: "plan".to_string(),
            suggestion: suggestion.map(str::to_string),
        };
        assert!(
            missing(Some("planning"))
                .message()
                .contains("did you mean `planning`")
        );
        assert!(!missing(None).message().contains("did you mean"));
    }
}
