//! Authored, display-only selection of Possible rows. No routing consumer imports this module.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::targets::HarnessScope;
use crate::error::{ConfigError, MarsError};
use crate::harness::registry::{self, HarnessId};
use crate::models::{self, possible::PossibleRow};
use crate::routing::slug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TierId {
    User,
    Project,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Shown { tier: Option<TierId> },
    Hidden { tier: Option<TierId> },
    Unmatched,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct RowKey {
    pub harness: HarnessId,
    pub harness_model_id: String,
    pub provider: Option<String>,
    pub model_id: String,
}

impl From<&PossibleRow> for RowKey {
    fn from(row: &PossibleRow) -> Self {
        Self {
            harness: row.harness,
            harness_model_id: row.harness_model_id.clone(),
            provider: row.provider.clone(),
            model_id: row.model_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowOrigin {
    Possible,
    Both,
    Declared,
}

#[derive(Debug, Clone, Serialize)]
pub struct CuratedRow {
    pub key: RowKey,
    pub possible: Option<PossibleRow>,
    pub decision: Decision,
    pub origin: RowOrigin,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CuratedView {
    pub rows: Vec<CuratedRow>,
    pub diagnostics: Vec<String>,
}

impl CuratedView {
    pub fn shown_rows(&self) -> impl Iterator<Item = &CuratedRow> {
        self.rows
            .iter()
            .filter(|row| matches!(row.decision, Decision::Shown { .. }))
    }
}

#[derive(Debug, Clone, Default)]
pub struct CuratedRules {
    tiers: Vec<Tier>,
}

#[derive(Debug, Clone)]
struct Tier {
    id: TierId,
    path: PathBuf,
    inherit: bool,
    default: Option<DefaultAction>,
    shows: Vec<Rule>,
    hides: Vec<Rule>,
}

#[derive(Debug, Clone)]
struct Rule {
    harness: Option<HarnessId>, // None means wildcard
    model: String,
    provider: Option<String>,
    glob: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DefaultAction {
    Show,
    Hide,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TierFile {
    #[serde(default = "default_inherit")]
    inherit: bool,
    default: Option<DefaultAction>,
    #[serde(default)]
    show: Vec<RuleFile>,
    #[serde(default)]
    hide: Vec<RuleFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    harness: String,
    model: String,
    provider: Option<String>,
}

fn default_inherit() -> bool {
    true
}

impl CuratedRules {
    pub fn load(project_root: &Path) -> Result<Self, MarsError> {
        let user = if let Some(dir) = std::env::var_os("MARS_CONFIG_DIR") {
            PathBuf::from(dir).join("curated.toml")
        } else {
            dirs::config_dir()
                .ok_or_else(|| invalid("cannot determine config directory; set MARS_CONFIG_DIR"))?
                .join("mars")
                .join("curated.toml")
        };
        Self::load_paths(
            &user,
            &project_root.join("mars.curated.toml"),
            &project_root.join("mars.curated.local.toml"),
        )
    }

    fn load_paths(user: &Path, project: &Path, local: &Path) -> Result<Self, MarsError> {
        let tiers = [
            (TierId::User, user),
            (TierId::Project, project),
            (TierId::Local, local),
        ]
        .into_iter()
        .filter_map(|(id, path)| match load_tier(id, path) {
            Ok(tier) => tier.map(Ok),
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { tiers })
    }

    fn reachable(&self) -> &[Tier] {
        let start = self
            .tiers
            .iter()
            .rposition(|tier| !tier.inherit)
            .unwrap_or(0);
        &self.tiers[start..]
    }

    /// Rule decision only. Call `default_for_unmatched` for an unmatched row.
    pub fn decide(&self, row: &RowKey, possible: &[PossibleRow]) -> Decision {
        let mut state = Decision::Unmatched;
        for tier in self.reachable() {
            let verdict = tier
                .hides
                .iter()
                .filter(|rule| rule.matches(row, possible))
                .map(|rule| if rule.glob { 2 } else { 4 })
                .chain(
                    tier.shows
                        .iter()
                        .filter(|rule| rule.matches(row, possible))
                        .map(|rule| if rule.glob { 1 } else { 3 }),
                )
                .max()
                .unwrap_or(0);
            state = match verdict {
                4 | 2 => Decision::Hidden {
                    tier: Some(tier.id),
                },
                3 => Decision::Shown {
                    tier: Some(tier.id),
                },
                1 if !matches!(state, Decision::Hidden { .. }) => Decision::Shown {
                    tier: Some(tier.id),
                },
                _ => state,
            };
        }
        state
    }

    pub fn default_for_unmatched(&self) -> Decision {
        if let Some(tier) = self
            .reachable()
            .iter()
            .rev()
            .find(|tier| tier.default.is_some())
        {
            return match tier.default.expect("checked") {
                DefaultAction::Show => Decision::Shown {
                    tier: Some(tier.id),
                },
                DefaultAction::Hide => Decision::Hidden {
                    tier: Some(tier.id),
                },
            };
        }
        if let Some(tier) = self
            .reachable()
            .iter()
            .rev()
            .find(|tier| tier.id != TierId::User && !tier.shows.is_empty())
        {
            Decision::Hidden {
                tier: Some(tier.id),
            }
        } else {
            Decision::Shown { tier: None }
        }
    }

    /// Build both the default curated view and the `--all` input without changing
    /// any routing or alias resolution state.
    pub fn project(
        &self,
        possible: &[PossibleRow],
        scope: &HarnessScope,
        installed: &HashSet<HarnessId>,
    ) -> CuratedView {
        let mut view = CuratedView::default();
        for row in possible {
            let key = RowKey::from(row);
            let raw = self.decide(&key, possible);
            view.rows.push(CuratedRow {
                key,
                possible: Some(row.clone()),
                decision: if raw == Decision::Unmatched {
                    self.default_for_unmatched()
                } else {
                    raw
                },
                origin: if raw == Decision::Unmatched {
                    RowOrigin::Possible
                } else {
                    RowOrigin::Both
                },
            });
        }
        for tier in self.reachable() {
            for rule in &tier.shows {
                let Some(harness) = rule.harness else {
                    continue;
                };
                if rule.glob {
                    continue;
                }
                if possible
                    .iter()
                    .any(|row| rule.matches(&RowKey::from(row), possible))
                {
                    continue;
                }
                if !scope.permits(harness.as_str()) {
                    view.diagnostics.push(format!(
                        "{}: declared {} model `{}` is outside the enabled harness scope",
                        tier.path.display(),
                        harness,
                        rule.model
                    ));
                    continue;
                }
                if harness == HarnessId::Cursor && rule.provider.is_some() {
                    view.diagnostics.push(format!(
                        "{}: Cursor declaration `{}` cannot have a provider",
                        tier.path.display(),
                        rule.model
                    ));
                    continue;
                }
                if let Some(problem) = rule.declaration_issue(harness) {
                    view.diagnostics.push(format!(
                        "{}: declared {} model `{}` {problem}",
                        tier.path.display(),
                        harness,
                        rule.model
                    ));
                    continue;
                }
                let key = rule.declared_key(harness);
                if !installed.contains(&harness) {
                    view.diagnostics.push(format!(
                        "{}: declared {} model `{}` has no installed harness",
                        tier.path.display(),
                        harness,
                        rule.model
                    ));
                }
                let decision = self.decide(&key, possible);
                if matches!(decision, Decision::Shown { .. })
                    && !view.rows.iter().any(|row| row.key.equivalent_to(&key))
                {
                    view.rows.push(CuratedRow {
                        key,
                        possible: None,
                        decision,
                        origin: RowOrigin::Declared,
                    });
                }
            }
        }
        view
    }
}

impl RowKey {
    fn equivalent_to(&self, other: &Self) -> bool {
        self.harness == other.harness
            && slug::model_ids_match(&self.harness_model_id, &other.harness_model_id)
            && match (&self.provider, &other.provider) {
                (Some(a), Some(b)) => slug::providers_match(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Rule {
    fn declaration_issue(&self, harness: HarnessId) -> Option<&'static str> {
        if let Some(native) = harness.native_provider()
            && self
                .provider
                .as_deref()
                .is_some_and(|provider| !slug::providers_match(provider, native))
        {
            return Some("has a provider incompatible with its native harness");
        }
        if matches!(harness, HarnessId::Pi | HarnessId::OpenCode)
            && let (Some(provider), Some(parts)) =
                (self.provider.as_deref(), slug::parse(&self.model))
            && !slug::providers_match(provider, parts.provider)
        {
            return Some("has conflicting provider and qualified model values");
        }
        None
    }

    fn matches(&self, row: &RowKey, possible: &[PossibleRow]) -> bool {
        if self.harness.is_some_and(|harness| harness != row.harness) {
            return false;
        }
        if let Some(provider) = &self.provider
            && !row
                .provider
                .as_deref()
                .is_some_and(|row_provider| slug::providers_match(provider, row_provider))
        {
            return false;
        }
        if self.glob {
            let pattern = self.model.to_lowercase();
            return models::glob_match(&pattern, &row.harness_model_id.to_lowercase())
                || models::glob_match(&pattern, &row.model_id.to_lowercase());
        }
        let full_match_exists = slug::model_ids_match(&self.model, &row.harness_model_id)
            || possible.iter().any(|candidate| {
                candidate.harness == row.harness
                    && self.provider.as_ref().is_none_or(|provider| {
                        candidate
                            .provider
                            .as_deref()
                            .is_some_and(|value| slug::providers_match(provider, value))
                    })
                    && slug::model_ids_match(&self.model, &candidate.harness_model_id)
            });
        if full_match_exists {
            slug::model_ids_match(&self.model, &row.harness_model_id)
        } else {
            slug::model_ids_match(&self.model, &row.model_id)
        }
    }

    fn declared_key(&self, harness: HarnessId) -> RowKey {
        let parsed = if matches!(harness, HarnessId::Pi | HarnessId::OpenCode) {
            slug::parse(&self.model)
        } else {
            None
        };
        let provider = parsed
            .as_ref()
            .map(|parts| parts.provider.to_string())
            .or_else(|| self.provider.clone())
            .or_else(|| harness.native_provider().map(str::to_string));
        let model_id = parsed
            .as_ref()
            .map_or(self.model.as_str(), |parts| parts.model_id)
            .to_string();
        let launch = crate::models::harness_model::resolve_harness_model(
            harness,
            &self.model,
            None,
            None,
            self.provider.as_deref(),
            provider.as_deref(),
        );
        RowKey {
            harness,
            harness_model_id: launch.harness_model_id,
            provider,
            model_id,
        }
    }
}

fn load_tier(id: TierId, path: &Path) -> Result<Option<Tier>, MarsError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(invalid(format!("{}: {error}", path.display()))),
    }
    let content = std::fs::read_to_string(path)
        .map_err(|error| invalid(format!("{}: {error}", path.display())))?;
    let parsed: TierFile = toml::from_str(&content)
        .map_err(|error| invalid(format!("{}: {error}", path.display())))?;
    Ok(Some(Tier {
        id,
        path: path.to_path_buf(),
        inherit: parsed.inherit,
        default: parsed.default,
        shows: validate_rules(path, "show", parsed.show)?,
        hides: validate_rules(path, "hide", parsed.hide)?,
    }))
}

fn validate_rules(path: &Path, action: &str, rules: Vec<RuleFile>) -> Result<Vec<Rule>, MarsError> {
    rules
        .into_iter()
        .enumerate()
        .map(|(index, rule)| {
            let harness = if rule.harness == "*" {
                None
            } else {
                Some(registry::parse(&rule.harness).ok_or_else(|| {
                    invalid(format!(
                        "{}: [[{action}]] #{} has unknown harness `{}`",
                        path.display(),
                        index + 1,
                        rule.harness
                    ))
                })?)
            };
            let model = rule.model.trim().to_string();
            if model.is_empty() {
                return Err(invalid(format!(
                    "{}: [[{action}]] #{} has an empty model",
                    path.display(),
                    index + 1
                )));
            }
            let provider = rule.provider.map(|value| value.trim().to_string());
            if provider.as_ref().is_some_and(String::is_empty) {
                return Err(invalid(format!(
                    "{}: [[{action}]] #{} has an empty provider",
                    path.display(),
                    index + 1
                )));
            }
            Ok(Rule {
                harness,
                glob: model.contains('*'),
                model,
                provider,
            })
        })
        .collect()
}

fn invalid(message: impl Into<String>) -> MarsError {
    MarsError::Config(ConfigError::Invalid {
        message: message.into(),
    })
}

#[cfg(test)]
mod tests;
