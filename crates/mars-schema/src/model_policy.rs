//! Model-policy definition and validation shared by profiles and consumer config.

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum ModelPolicyMatchType {
    Model,
    Alias,
    ModelGlob,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelPolicyOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
}

impl ModelPolicyOverride {
    pub fn is_empty(&self) -> bool {
        self.effort.is_none() && self.sandbox.is_none() && self.approval.is_none()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        match key {
            "effort" => self.effort.as_deref(),
            "sandbox" => self.sandbox.as_deref(),
            "approval" => self.approval.as_deref(),
            _ => None,
        }
    }
}

/// Shared model-policy rule type used by profile frontmatter, agent overlays,
/// and settings-level model policies.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPolicyRule {
    pub match_type: ModelPolicyMatchType,
    pub match_value: String,
    pub no_fallback: bool,
    pub overrides: ModelPolicyOverride,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPolicyRuleParseError {
    RuleMustBeMapping {
        found: String,
    },
    MatchMissing,
    MatchMustBeMapping {
        found: String,
    },
    MatchMustContainExactlyOne {
        found: String,
    },
    MatchKeyMustBeString {
        found: String,
    },
    UnknownMatchKey {
        key: String,
    },
    MatchValueMustBeString {
        key: String,
        found: String,
    },
    MatchValueEmpty {
        key: String,
    },
    OverrideMustBeMapping {
        found: String,
    },
    UnknownOverrideKey {
        key: String,
    },
    OverrideValueInvalid {
        key: String,
        found: String,
        allowed: &'static str,
    },
    NoFallbackMustBeBoolean {
        found: String,
    },
}

impl ModelPolicyRuleParseError {
    fn deserialize_message(&self) -> String {
        match self {
            Self::MatchMustContainExactlyOne { .. }
            | Self::MatchMissing
            | Self::MatchMustBeMapping { .. } => {
                "model policy `match` must contain exactly one of model, alias, model-glob"
                    .to_string()
            }
            Self::MatchKeyMustBeString { .. } => {
                "model policy `match` key must be a string".to_string()
            }
            Self::MatchValueMustBeString { .. } => {
                "model policy `match` value must be a string".to_string()
            }
            Self::MatchValueEmpty { .. } => {
                "model policy `match` value must be a non-empty string".to_string()
            }
            Self::UnknownMatchKey { key } => format!(
                "unknown model policy match key `{key}`; expected model, alias, or model-glob"
            ),
            Self::OverrideMustBeMapping { .. } => {
                "model policy `override` must be a mapping".to_string()
            }
            Self::UnknownOverrideKey { key } => format!(
                "unknown model policy override `{key}`; expected effort, sandbox, or approval"
            ),
            Self::OverrideValueInvalid {
                key,
                found,
                allowed,
            } => format!(
                "model policy override `{key}` has invalid value `{found}`; allowed: {allowed}"
            ),
            Self::NoFallbackMustBeBoolean { .. } => {
                "model policy `no-fallback` must be a boolean".to_string()
            }
            Self::RuleMustBeMapping { .. } => "model policy rule must be a mapping".to_string(),
        }
    }
}

pub fn parse_model_policy_rule_value(
    value: &serde_yaml::Value,
) -> Result<ModelPolicyRule, ModelPolicyRuleParseError> {
    let rule = value
        .as_mapping()
        .ok_or_else(|| ModelPolicyRuleParseError::RuleMustBeMapping {
            found: format!("{value:?}"),
        })?;

    let match_value = rule.get(serde_yaml::Value::String("match".to_string()));
    let match_mapping = match match_value {
        Some(value) => {
            value
                .as_mapping()
                .ok_or_else(|| ModelPolicyRuleParseError::MatchMustBeMapping {
                    found: format!("{value:?}"),
                })?
        }
        None => return Err(ModelPolicyRuleParseError::MatchMissing),
    };

    let mut entries = match_mapping.iter();
    let Some((match_key, match_value)) = entries.next() else {
        return Err(ModelPolicyRuleParseError::MatchMustContainExactlyOne {
            found: format!("{match_mapping:?}"),
        });
    };
    if entries.next().is_some() {
        return Err(ModelPolicyRuleParseError::MatchMustContainExactlyOne {
            found: format!("{match_mapping:?}"),
        });
    }

    let key =
        match_key
            .as_str()
            .ok_or_else(|| ModelPolicyRuleParseError::MatchKeyMustBeString {
                found: format!("{match_key:?}"),
            })?;
    let value =
        match_value
            .as_str()
            .ok_or_else(|| ModelPolicyRuleParseError::MatchValueMustBeString {
                key: key.to_string(),
                found: format!("{match_value:?}"),
            })?;
    let match_value = value.to_string();
    if match_value.is_empty() {
        return Err(ModelPolicyRuleParseError::MatchValueEmpty {
            key: key.to_string(),
        });
    }

    let match_type = match key {
        "model" => ModelPolicyMatchType::Model,
        "alias" => ModelPolicyMatchType::Alias,
        "model-glob" => ModelPolicyMatchType::ModelGlob,
        _ => {
            return Err(ModelPolicyRuleParseError::UnknownMatchKey {
                key: key.to_string(),
            });
        }
    };

    let overrides = parse_overrides(rule.get(serde_yaml::Value::String("override".to_string())))?;

    let no_fallback = match rule.get(serde_yaml::Value::String("no-fallback".to_string())) {
        None | Some(serde_yaml::Value::Null) => false,
        Some(serde_yaml::Value::Bool(value)) => *value,
        Some(value) => {
            return Err(ModelPolicyRuleParseError::NoFallbackMustBeBoolean {
                found: format!("{value:?}"),
            });
        }
    };

    Ok(ModelPolicyRule {
        match_type,
        match_value,
        no_fallback,
        overrides,
    })
}

fn parse_overrides(
    value: Option<&serde_yaml::Value>,
) -> Result<ModelPolicyOverride, ModelPolicyRuleParseError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(ModelPolicyOverride::default());
    };
    let mapping =
        value
            .as_mapping()
            .ok_or_else(|| ModelPolicyRuleParseError::OverrideMustBeMapping {
                found: format!("{value:?}"),
            })?;
    let mut overrides = ModelPolicyOverride::default();
    for (key, value) in mapping {
        let key = key
            .as_str()
            .ok_or_else(|| ModelPolicyRuleParseError::UnknownOverrideKey {
                key: format!("{key:?}"),
            })?;
        let raw =
            value
                .as_str()
                .ok_or_else(|| ModelPolicyRuleParseError::OverrideValueInvalid {
                    key: key.to_string(),
                    found: format!("{value:?}"),
                    allowed: allowed_override_values(key),
                })?;
        match key {
            "effort" => {
                let normalized = raw.to_ascii_lowercase();
                if !matches!(
                    normalized.as_str(),
                    "none" | "low" | "medium" | "high" | "xhigh" | "max"
                ) {
                    return Err(ModelPolicyRuleParseError::OverrideValueInvalid {
                        key: key.to_string(),
                        found: raw.to_string(),
                        allowed: allowed_override_values(key),
                    });
                }
                overrides.effort = Some(if normalized == "max" {
                    "xhigh".to_string()
                } else {
                    normalized
                });
            }
            "sandbox"
                if matches!(
                    raw,
                    "default" | "read-only" | "workspace-write" | "danger-full-access"
                ) =>
            {
                overrides.sandbox = Some(raw.to_string());
            }
            "approval" if matches!(raw, "default" | "auto" | "confirm" | "never") => {
                overrides.approval = Some(raw.to_string());
            }
            "sandbox" | "approval" => {
                return Err(ModelPolicyRuleParseError::OverrideValueInvalid {
                    key: key.to_string(),
                    found: raw.to_string(),
                    allowed: allowed_override_values(key),
                });
            }
            _ => {
                return Err(ModelPolicyRuleParseError::UnknownOverrideKey {
                    key: key.to_string(),
                });
            }
        }
    }
    Ok(overrides)
}

fn allowed_override_values(key: &str) -> &'static str {
    match key {
        "effort" => "none, low, medium, high, xhigh",
        "sandbox" => "default, read-only, workspace-write, danger-full-access",
        "approval" => "default, auto, confirm, never",
        _ => "effort, sandbox, approval",
    }
}

impl<'de> Deserialize<'de> for ModelPolicyRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        parse_model_policy_rule_value(&value)
            .map_err(|err| serde::de::Error::custom(err.deserialize_message()))
    }
}

impl Serialize for ModelPolicyRule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        let match_key = match self.match_type {
            ModelPolicyMatchType::Model => "model",
            ModelPolicyMatchType::Alias => "alias",
            ModelPolicyMatchType::ModelGlob => "model-glob",
        };

        let mut match_clause = serde_yaml::Mapping::new();
        match_clause.insert(
            serde_yaml::Value::String(match_key.to_string()),
            serde_yaml::Value::String(self.match_value.clone()),
        );
        map.serialize_entry("match", &match_clause)?;
        if !self.overrides.is_empty() {
            map.serialize_entry("override", &self.overrides)?;
        }
        if self.no_fallback {
            map.serialize_entry("no-fallback", &self.no_fallback)?;
        }
        map.end()
    }
}
