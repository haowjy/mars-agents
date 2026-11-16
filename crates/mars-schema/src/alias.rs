//! A `[models.<alias>]` table of `mars.toml`: how an alias names a model, and the pure
//! resolution of it over models.dev rows.
//!
//! Two kinds: a pinned alias names a model id (with optional `match`/`exclude` filters that
//! widen what it accepts), an `auto` alias selects from the catalog by `match`, `exclude` and
//! `provider`. Resolution is `models::auto_resolve_all`, so the CLI's snapshot and a host's rows
//! choose the same id.

use serde::{Deserialize, Serialize};

use crate::agent::EffortRequest;
use crate::models::{CachedModel, auto_resolve_all, infer_provider_from_model_id};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelAlias {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompting: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<EffortRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autocompact: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autocompact_pct: Option<u8>,
    #[serde(flatten)]
    pub spec: ModelSpec,
}

impl ModelAlias {
    pub fn pinned_model_id(&self) -> Option<&str> {
        match &self.spec {
            ModelSpec::Pinned { model, .. } | ModelSpec::PinnedWithMatch { model, .. } => {
                Some(model.as_str())
            }
            ModelSpec::AutoResolve { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelSpec {
    Pinned {
        model: String,
        provider: Option<String>,
    },
    PinnedWithMatch {
        model: String,
        provider: Option<String>,
        match_patterns: Vec<String>,
        exclude_patterns: Vec<String>,
    },
    AutoResolve {
        provider: Option<String>,
        match_patterns: Vec<String>,
        exclude_patterns: Vec<String>,
    },
}

impl Serialize for ModelSpec {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            ModelSpec::Pinned { model, provider } => {
                let mut count = 1;
                if provider.is_some() {
                    count += 1;
                }
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("model", model)?;
                if let Some(provider) = provider {
                    map.serialize_entry("provider", provider)?;
                }
                map.end()
            }
            ModelSpec::PinnedWithMatch {
                model,
                provider,
                match_patterns,
                exclude_patterns,
            } => {
                let mut count = 2; // model + match
                if provider.is_some() {
                    count += 1;
                }
                if !exclude_patterns.is_empty() {
                    count += 1;
                }
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("model", model)?;
                map.serialize_entry("match", match_patterns)?;
                if let Some(provider) = provider {
                    map.serialize_entry("provider", provider)?;
                }
                if !exclude_patterns.is_empty() {
                    map.serialize_entry("exclude", exclude_patterns)?;
                }
                map.end()
            }
            ModelSpec::AutoResolve {
                provider,
                match_patterns,
                exclude_patterns,
            } => {
                let mut count = 1; // match
                if provider.is_some() {
                    count += 1;
                }
                if !exclude_patterns.is_empty() {
                    count += 1;
                }
                let mut map = serializer.serialize_map(Some(count))?;
                if let Some(provider) = provider {
                    map.serialize_entry("provider", provider)?;
                }
                map.serialize_entry("match", match_patterns)?;
                if !exclude_patterns.is_empty() {
                    map.serialize_entry("exclude", exclude_patterns)?;
                }
                map.end()
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawModelAlias {
    harness: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    prompting: Option<String>,
    #[serde(default)]
    native: Option<toml::Value>,
    #[serde(default)]
    default_effort: Option<String>,
    #[serde(default)]
    autocompact: Option<toml::Value>,
    #[serde(default)]
    autocompact_pct: Option<toml::Value>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default, rename = "match")]
    match_patterns: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
}

impl<'de> Deserialize<'de> for ModelAlias {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawModelAlias::deserialize(deserializer)?;
        let normalized_harness = if let Some(ref harness_name) = raw.harness {
            Some(crate::harness::normalize_name(harness_name).ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "invalid harness '{harness_name}'; valid harnesses: {}",
                    crate::harness::names()
                ))
            })?)
        } else {
            None
        };
        if raw.native.is_some() {
            return Err(serde::de::Error::custom(
                "[models.<alias>.native] is no longer supported; Cursor model adaptation is internal",
            ));
        }
        let default_effort = raw
            .default_effort
            .filter(|value| !value.trim().is_empty())
            .map(|effort| {
                EffortRequest::parse(&effort).ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "invalid default_effort '{effort}'; accepted values: {}",
                        EffortRequest::ALLOWED
                    ))
                })
            })
            .transpose()?;
        let autocompact: Option<u32> = match raw.autocompact {
            Some(toml::Value::Integer(value)) => match u32::try_from(value) {
                Ok(v) => Some(v),
                Err(_) => {
                    return Err(serde::de::Error::custom(format!(
                        "autocompact {value} is out of u32 range (0–4294967295)"
                    )));
                }
            },
            Some(other) => {
                return Err(serde::de::Error::custom(format!(
                    "autocompact must be an integer (token count), got {other:?}"
                )));
            }
            None => None,
        };
        let autocompact_pct: Option<u8> = match raw.autocompact_pct {
            Some(toml::Value::Integer(value)) if (1..=100).contains(&value) => Some(value as u8),
            Some(toml::Value::Integer(value)) => {
                return Err(serde::de::Error::custom(format!(
                    "autocompact_pct {value} is out of range 1-100"
                )));
            }
            Some(other) => {
                return Err(serde::de::Error::custom(format!(
                    "autocompact_pct must be an integer 1-100, got {other:?}"
                )));
            }
            None => None,
        };

        let has_match = raw.match_patterns.is_some();

        let spec = if let Some(model) = raw.model {
            if !has_match && raw.exclude.is_some() {
                return Err(serde::de::Error::custom(
                    "model alias with 'exclude' must also include 'match'",
                ));
            }
            if let Some(match_patterns) = raw.match_patterns {
                ModelSpec::PinnedWithMatch {
                    model,
                    provider: raw.provider,
                    match_patterns,
                    exclude_patterns: raw.exclude.unwrap_or_default(),
                }
            } else {
                ModelSpec::Pinned {
                    model,
                    provider: raw.provider,
                }
            }
        } else if let Some(match_patterns) = raw.match_patterns {
            ModelSpec::AutoResolve {
                provider: raw.provider,
                match_patterns,
                exclude_patterns: raw.exclude.unwrap_or_default(),
            }
        } else {
            return Err(serde::de::Error::custom(
                "model alias must have either 'model' (pinned) or 'match' (auto-resolve)",
            ));
        };

        Ok(ModelAlias {
            harness: normalized_harness,
            description: raw.description,
            prompting: raw.prompting,
            default_effort,
            autocompact,
            autocompact_pct,
            spec,
        })
    }
}

impl ModelAlias {
    /// The id the alias resolves to over `rows`: a pinned `model` as written, else the best row
    /// an `auto` alias selects. `None` when an `auto` alias selects nothing.
    pub fn resolve(&self, rows: &[CachedModel]) -> Option<String> {
        self.pinned_model_id()
            .map(str::to_owned)
            .or_else(|| self.selected(rows).first().map(|row| row.id.clone()))
    }

    /// Every id the alias accepts, in `resolve`'s order: a pinned `model` first, then each row
    /// `match`, `exclude` and `provider` select, best first. A pinned alias with no `match`
    /// accepts its `model` alone.
    pub fn candidates(&self, rows: &[CachedModel]) -> Vec<String> {
        let mut ids: Vec<String> = self
            .pinned_model_id()
            .map(str::to_owned)
            .into_iter()
            .collect();
        for row in self.selected(rows) {
            if !ids.contains(&row.id) {
                ids.push(row.id.clone());
            }
        }
        ids
    }

    /// The rows `match`, `exclude` and `provider` select; a pinned model's provider is inferred
    /// from its id when the alias names none.
    pub fn selected<'a>(&self, rows: &'a [CachedModel]) -> Vec<&'a CachedModel> {
        match &self.spec {
            ModelSpec::Pinned { .. } => Vec::new(),
            ModelSpec::PinnedWithMatch {
                model,
                provider,
                match_patterns,
                exclude_patterns,
            } => auto_resolve_all(
                provider
                    .as_deref()
                    .or_else(|| infer_provider_from_model_id(model)),
                match_patterns,
                exclude_patterns,
                rows,
            ),
            ModelSpec::AutoResolve {
                provider,
                match_patterns,
                exclude_patterns,
            } => auto_resolve_all(provider.as_deref(), match_patterns, exclude_patterns, rows),
        }
    }
}
