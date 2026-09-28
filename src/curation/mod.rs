//! Authored, display-only selection of Possible rows. No routing consumer imports this module.

use std::collections::{BTreeMap, HashSet};
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

/// Reusable literal/full-launch context for deciding many rows in one pass.
pub struct CuratedMatcher<'a> {
    tiers: Vec<PreparedTier<'a>>,
}

impl CuratedMatcher<'_> {
    pub fn decide(&self, row: &RowKey) -> Decision {
        CuratedRules::decide_prepared(&self.tiers, &MatchRow::new(row.clone()))
    }
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

/// Per-projection matching plan. Literal/full-launch precedence is computed
/// once, rather than searching the entire Possible inventory for every row.
struct PreparedTier<'a> {
    tier: &'a Tier,
    shows: Vec<PreparedRule<'a>>,
    hides: Vec<PreparedRule<'a>>,
}

struct PreparedRule<'a> {
    rule: &'a Rule,
    provider: Option<String>,
    glob: Option<String>,
    literal_by_harness: BTreeMap<HarnessId, String>,
    full_match_harnesses: HashSet<HarnessId>,
}

struct MatchRow {
    key: RowKey,
    provider: Option<String>,
    full_norm: String,
    bare_norm: String,
    full_lower: String,
    bare_lower: String,
}

impl MatchRow {
    fn new(key: RowKey) -> Self {
        Self {
            provider: key.provider.as_deref().map(slug::normalize_provider),
            full_norm: slug::normalize_model_id(&key.harness_model_id),
            bare_norm: slug::normalize_model_id(&key.model_id),
            full_lower: key.harness_model_id.to_lowercase(),
            bare_lower: key.model_id.to_lowercase(),
            key,
        }
    }
}

impl<'a> PreparedRule<'a> {
    fn new(rule: &'a Rule, possible: &[MatchRow]) -> Self {
        let provider = rule.provider.as_deref().map(slug::normalize_provider);
        let glob = rule.glob.then(|| rule.model.to_lowercase());
        let mut literal_by_harness = BTreeMap::new();
        let mut full_match_harnesses = HashSet::new();
        if !rule.glob {
            for harness in registry::all() {
                if rule.harness.is_some_and(|id| id != *harness) {
                    continue;
                }
                let Some(target) = rule.literal_target(*harness) else {
                    continue;
                };
                if possible.iter().any(|row| {
                    row.key.harness == *harness
                        && provider
                            .as_ref()
                            .is_none_or(|value| row.provider.as_ref() == Some(value))
                        && row.full_norm == target
                }) {
                    full_match_harnesses.insert(*harness);
                }
                literal_by_harness.insert(*harness, target);
            }
        }
        Self {
            rule,
            provider,
            glob,
            literal_by_harness,
            full_match_harnesses,
        }
    }

    fn matches(&self, row: &MatchRow) -> bool {
        if self
            .rule
            .harness
            .is_some_and(|harness| harness != row.key.harness)
        {
            return false;
        }
        if self
            .provider
            .as_ref()
            .is_some_and(|value| row.provider.as_ref() != Some(value))
        {
            return false;
        }
        if let Some(glob) = &self.glob {
            return models::glob_match(glob, &row.full_lower)
                || models::glob_match(glob, &row.bare_lower);
        }
        let Some(target) = self.literal_by_harness.get(&row.key.harness) else {
            return false;
        };
        if self.full_match_harnesses.contains(&row.key.harness) || target == &row.full_norm {
            target == &row.full_norm
        } else {
            target == &row.bare_norm
        }
    }
}

impl<'a> PreparedTier<'a> {
    fn new(tier: &'a Tier, possible: &[MatchRow]) -> Self {
        Self {
            tier,
            shows: tier
                .shows
                .iter()
                .map(|rule| PreparedRule::new(rule, possible))
                .collect(),
            hides: tier
                .hides
                .iter()
                .map(|rule| PreparedRule::new(rule, possible))
                .collect(),
        }
    }
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

    /// Prepare once for bulk decisions. `project` uses the same context.
    pub fn matcher(&self, possible: &[PossibleRow]) -> CuratedMatcher<'_> {
        let indexed = possible
            .iter()
            .map(|row| MatchRow::new(RowKey::from(row)))
            .collect::<Vec<_>>();
        CuratedMatcher {
            tiers: self.prepare(&indexed),
        }
    }

    fn prepare<'a>(&'a self, possible: &[MatchRow]) -> Vec<PreparedTier<'a>> {
        self.reachable()
            .iter()
            .map(|tier| PreparedTier::new(tier, possible))
            .collect()
    }

    fn decide_prepared(prepared: &[PreparedTier<'_>], row: &MatchRow) -> Decision {
        let mut state = Decision::Unmatched;
        for prepared_tier in prepared {
            let tier = prepared_tier.tier;
            let verdict = prepared_tier
                .hides
                .iter()
                .filter(|rule| rule.matches(row))
                .map(|rule| if rule.rule.glob { 2 } else { 4 })
                .chain(
                    prepared_tier
                        .shows
                        .iter()
                        .filter(|rule| rule.matches(row))
                        .map(|rule| if rule.rule.glob { 1 } else { 3 }),
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
    pub fn project(&self, possible: &[PossibleRow], scope: &HarnessScope) -> CuratedView {
        let mut view = CuratedView::default();
        let indexed = possible
            .iter()
            .map(|row| MatchRow::new(RowKey::from(row)))
            .collect::<Vec<_>>();
        let prepared = self.prepare(&indexed);
        for (row, indexed_row) in possible.iter().zip(&indexed) {
            let raw = Self::decide_prepared(&prepared, indexed_row);
            view.rows.push(CuratedRow {
                key: indexed_row.key.clone(),
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
        let mut scoped_diagnostics = HashSet::new();
        for prepared_tier in &prepared {
            let tier = prepared_tier.tier;
            for prepared_rule in &prepared_tier.shows {
                let rule = prepared_rule.rule;
                let Some(harness) = rule.harness else {
                    continue;
                };
                if rule.glob {
                    continue;
                }
                if indexed.iter().any(|row| prepared_rule.matches(row)) {
                    continue;
                }
                if !scope.permits(harness.as_str()) {
                    let key = (
                        harness,
                        slug::normalize_model_id(&rule.model),
                        rule.provider.as_deref().map(slug::normalize_provider),
                    );
                    if scoped_diagnostics.insert(key) {
                        view.diagnostics.push(format!(
                            "{}: declared {} model `{}` is outside the enabled harness scope",
                            tier.path.display(),
                            harness,
                            rule.model
                        ));
                    }
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
                let decision = Self::decide_prepared(&prepared, &MatchRow::new(key.clone()));
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
    fn literal_target(&self, harness: HarnessId) -> Option<String> {
        if let Some(native) = harness.native_provider()
            && let Some(parts) = slug::parse(&self.model)
        {
            if !slug::providers_match(parts.provider, native)
                || self
                    .provider
                    .as_deref()
                    .is_some_and(|provider| !slug::providers_match(provider, parts.provider))
            {
                return None;
            }
            return Some(slug::normalize_model_id(parts.model_id));
        }
        Some(slug::normalize_model_id(&self.model))
    }

    fn declaration_issue(&self, harness: HarnessId) -> Option<&'static str> {
        if let Some(native) = harness.native_provider() {
            if self
                .provider
                .as_deref()
                .is_some_and(|provider| !slug::providers_match(provider, native))
            {
                return Some("has a provider incompatible with its native harness");
            }
            if let Some(parts) = slug::parse(&self.model)
                && !slug::providers_match(parts.provider, native)
            {
                return Some("has a qualified provider incompatible with its native harness");
            }
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

    fn declared_key(&self, harness: HarnessId) -> RowKey {
        let parsed = if harness.native_provider().is_some()
            || matches!(harness, HarnessId::Pi | HarnessId::OpenCode)
        {
            slug::parse(&self.model)
        } else {
            None
        };
        let provider = harness
            .native_provider()
            .map(str::to_string)
            .or_else(|| parsed.as_ref().map(|parts| parts.provider.to_string()))
            .or_else(|| self.provider.clone());
        let model_id = parsed
            .as_ref()
            .map_or(self.model.as_str(), |parts| parts.model_id)
            .to_string();
        let launch = crate::models::harness_model::resolve_harness_model(
            harness,
            if harness.native_provider().is_some() {
                &model_id
            } else {
                &self.model
            },
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
