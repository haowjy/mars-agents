//! Which of a package's items a dependency entry selects: the authored `agents`, `skills`,
//! `exclude`, `only_skills`, `only_agents` and `rename` keys, their consistency rule, and the
//! `FilterMode` they resolve to.

use serde::{Deserialize, Serialize};

use crate::names::{ItemName, RenameMap};

fn is_false(v: &bool) -> bool {
    !*v
}

/// Shared include/exclude/rename filter configuration for a source.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FilterConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<ItemName>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<ItemName>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<ItemName>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<RenameMap>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub only_skills: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub only_agents: bool,
}

/// How items are filtered from a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FilterMode {
    /// Install everything from the source.
    All,
    /// Only install specific agents and/or skills.
    Include {
        agents: Vec<ItemName>,
        skills: Vec<ItemName>,
    },
    /// Install everything except these items.
    Exclude(Vec<ItemName>),
    /// Install only skills, no agents.
    OnlySkills,
    /// Install only agents plus their transitive skill dependencies.
    OnlyAgents,
}

/// A filter combination no mode can express. The message names the dependency.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FilterError {
    #[error("dependency `{name}`: only_skills and only_agents are mutually exclusive")]
    BothCategories { name: String },
    #[error("dependency `{name}`: only_skills/only_agents cannot combine with agents/skills lists")]
    CategoryWithInclude { name: String },
    #[error("dependency `{name}`: only_skills/only_agents cannot combine with exclude")]
    CategoryWithExclude { name: String },
    #[error("source `{name}` uses both agents/skills and exclude — pick one")]
    IncludeWithExclude { name: String },
}

impl FilterError {
    /// A stable identifier for callers that map failures to their own codes.
    pub fn code(&self) -> &'static str {
        "filter.inconsistent"
    }
}

impl FilterConfig {
    /// Reject inconsistent combinations: both category flags, a category flag with an include
    /// list or `exclude`, and an include list with `exclude`.
    pub fn validate(&self, dep_name: &str) -> Result<(), FilterError> {
        let name = || dep_name.to_string();
        let has_include = self.agents.is_some() || self.skills.is_some();
        let has_exclude = self.exclude.is_some();
        let has_category = self.only_skills || self.only_agents;

        if self.only_skills && self.only_agents {
            return Err(FilterError::BothCategories { name: name() });
        }
        if has_category && has_include {
            return Err(FilterError::CategoryWithInclude { name: name() });
        }
        if has_category && has_exclude {
            return Err(FilterError::CategoryWithExclude { name: name() });
        }
        if has_include && has_exclude {
            return Err(FilterError::IncludeWithExclude { name: name() });
        }
        Ok(())
    }

    /// Convert to the resolved FilterMode enum.
    pub fn to_mode(&self) -> FilterMode {
        if self.only_skills {
            FilterMode::OnlySkills
        } else if self.only_agents {
            FilterMode::OnlyAgents
        } else if self.agents.is_some() || self.skills.is_some() {
            FilterMode::Include {
                agents: self.agents.clone().unwrap_or_default(),
                skills: self.skills.clone().unwrap_or_default(),
            }
        } else if self.exclude.is_some() {
            FilterMode::Exclude(self.exclude.clone().unwrap_or_default())
        } else {
            FilterMode::All
        }
    }

    /// Returns true if any filter field is set (not default).
    pub fn has_any_filter(&self) -> bool {
        self.agents.is_some()
            || self.skills.is_some()
            || self.exclude.is_some()
            || self.only_skills
            || self.only_agents
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_config_has_any_filter() {
        assert!(!FilterConfig::default().has_any_filter());
        assert!(
            FilterConfig {
                only_skills: true,
                ..FilterConfig::default()
            }
            .has_any_filter()
        );
        assert!(
            FilterConfig {
                agents: Some(vec!["coder".into()]),
                ..FilterConfig::default()
            }
            .has_any_filter()
        );
    }

    #[test]
    fn filter_config_to_mode() {
        assert!(matches!(FilterConfig::default().to_mode(), FilterMode::All));
        assert!(matches!(
            FilterConfig {
                only_skills: true,
                ..FilterConfig::default()
            }
            .to_mode(),
            FilterMode::OnlySkills
        ));
        assert!(matches!(
            FilterConfig {
                only_agents: true,
                ..FilterConfig::default()
            }
            .to_mode(),
            FilterMode::OnlyAgents
        ));
        assert!(matches!(
            FilterConfig {
                agents: Some(vec!["coder".into()]),
                ..FilterConfig::default()
            }
            .to_mode(),
            FilterMode::Include { .. }
        ));
        assert!(matches!(
            FilterConfig {
                exclude: Some(vec!["old".into()]),
                ..FilterConfig::default()
            }
            .to_mode(),
            FilterMode::Exclude(_)
        ));
    }
}
