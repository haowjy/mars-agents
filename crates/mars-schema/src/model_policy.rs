//! Model-policy definition and validation shared by profiles and consumer config.

use crate::agent::{ApprovalMode, EffortLevel, SandboxMode};
use crate::network::NetworkPolicy;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
}

impl ModelPolicyOverride {
    pub fn is_empty(&self) -> bool {
        self.effort.is_none()
            && self.sandbox.is_none()
            && self.approval.is_none()
            && self.network.is_none()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        match key {
            "effort" => self.effort.as_deref(),
            "sandbox" => self.sandbox.as_deref(),
            "approval" => self.approval.as_deref(),
            "network" => self.network.as_deref(),
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
    /// A compact, authored guide explaining when this model is a good choice.
    pub when: Option<String>,
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
    /// A host set is layered by settings, profile, overlay and CLI, never by a model rule.
    NetworkHostsNotAllowed {
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
    WhenInvalid {
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
            Self::NetworkHostsNotAllowed { key } => format!(
                "model policy override `{key}` is not supported; set it on settings, the profile, an agent overlay, or the CLI"
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
            Self::WhenInvalid { .. } => {
                "model policy `when` must be a non-empty one-line string".to_string()
            }
            Self::RuleMustBeMapping { .. } => "model policy rule must be a mapping".to_string(),
        }
    }
}

/// The keys of a rule, and of its `override`, that Mars reads. A key beside them is unknown.
const RULE_KEYS: &[&str] = &["match", "override", "no-fallback", "when"];
const OVERRIDE_KEYS: &[&str] = &[
    "effort",
    "sandbox",
    "approval",
    "network",
    "network-allow",
    "network-deny",
];

/// A key of a rule the reader does not read: it stays in the file, and the caller warns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnknownRuleKey {
    /// In the rule's `override` table, not the rule itself.
    pub in_override: bool,
    pub key: String,
}

/// What reading one rule found, all in one pass: the rule when nothing in it is rejected, every
/// rejection when something is, and the unknown keys either way.
pub(crate) struct RuleReading {
    pub rule: Result<ModelPolicyRule, Vec<ModelPolicyRuleParseError>>,
    pub unknown: Vec<UnknownRuleKey>,
}

pub(crate) fn read_model_policy_rule(value: &serde_yaml::Value) -> RuleReading {
    use ModelPolicyRuleParseError as Error;
    let Some(rule) = value.as_mapping() else {
        return RuleReading {
            rule: Err(vec![Error::RuleMustBeMapping {
                found: crate::ordered::plain(value),
            }]),
            unknown: Vec::new(),
        };
    };
    let mut errors = Vec::new();
    let mut unknown = unknown_keys(rule, RULE_KEYS, false);

    let matched = read_match(rule).map_err(|error| errors.push(error)).ok();

    let overrides = match rule.get(serde_yaml::Value::String("override".to_string())) {
        Some(value) if !value.is_null() => match value.as_mapping() {
            Some(mapping) => {
                unknown.extend(unknown_keys(mapping, OVERRIDE_KEYS, true));
                read_overrides(mapping, &mut errors)
            }
            None => {
                errors.push(Error::OverrideMustBeMapping {
                    found: crate::ordered::plain(value),
                });
                ModelPolicyOverride::default()
            }
        },
        _ => ModelPolicyOverride::default(),
    };

    let no_fallback = match rule.get(serde_yaml::Value::String("no-fallback".to_string())) {
        None | Some(serde_yaml::Value::Null) => false,
        Some(serde_yaml::Value::Bool(value)) => *value,
        Some(value) => {
            errors.push(Error::NoFallbackMustBeBoolean {
                found: crate::ordered::plain(value),
            });
            false
        }
    };

    let when = match rule.get(serde_yaml::Value::String("when".to_string())) {
        None | Some(serde_yaml::Value::Null) => None,
        Some(serde_yaml::Value::String(value))
            if !value.is_empty() && !value.contains(['\n', '\r']) =>
        {
            Some(value.clone())
        }
        Some(value) => {
            errors.push(Error::WhenInvalid {
                found: crate::ordered::plain(value),
            });
            None
        }
    };

    let rule = match matched {
        Some((match_type, match_value)) if errors.is_empty() => Ok(ModelPolicyRule {
            match_type,
            match_value,
            no_fallback,
            overrides,
            when,
        }),
        _ => Err(errors),
    };
    RuleReading { rule, unknown }
}

fn unknown_keys(
    mapping: &serde_yaml::Mapping,
    known: &[&str],
    in_override: bool,
) -> Vec<UnknownRuleKey> {
    mapping
        .keys()
        .filter(|key| !key.as_str().is_some_and(|key| known.contains(&key)))
        .map(|key| UnknownRuleKey {
            in_override,
            key: key
                .as_str()
                .map_or_else(|| crate::ordered::plain(key), str::to_owned),
        })
        .collect()
}

fn read_match(
    rule: &serde_yaml::Mapping,
) -> Result<(ModelPolicyMatchType, String), ModelPolicyRuleParseError> {
    use ModelPolicyRuleParseError as Error;
    let match_mapping = match rule.get(serde_yaml::Value::String("match".to_string())) {
        Some(value) => value
            .as_mapping()
            .ok_or_else(|| Error::MatchMustBeMapping {
                found: crate::ordered::plain(value),
            })?,
        None => return Err(Error::MatchMissing),
    };
    let exactly_one = || Error::MatchMustContainExactlyOne {
        found: crate::ordered::plain(&serde_yaml::Value::Mapping(match_mapping.clone())),
    };
    let mut entries = match_mapping.iter();
    let Some((match_key, match_value)) = entries.next() else {
        return Err(exactly_one());
    };
    if entries.next().is_some() {
        return Err(exactly_one());
    }

    let key = match_key
        .as_str()
        .ok_or_else(|| Error::MatchKeyMustBeString {
            found: crate::ordered::plain(match_key),
        })?;
    let value = match_value
        .as_str()
        .ok_or_else(|| Error::MatchValueMustBeString {
            key: key.to_string(),
            found: crate::ordered::plain(match_value),
        })?;
    if value.is_empty() {
        return Err(Error::MatchValueEmpty {
            key: key.to_string(),
        });
    }
    let match_type = match key {
        "model" => ModelPolicyMatchType::Model,
        "alias" => ModelPolicyMatchType::Alias,
        "model-glob" => ModelPolicyMatchType::ModelGlob,
        _ => {
            return Err(Error::UnknownMatchKey {
                key: key.to_string(),
            });
        }
    };
    Ok((match_type, value.to_string()))
}

/// The override keys Mars reads; each rejected one adds its own error.
fn read_overrides(
    mapping: &serde_yaml::Mapping,
    errors: &mut Vec<ModelPolicyRuleParseError>,
) -> ModelPolicyOverride {
    let mut overrides = ModelPolicyOverride::default();
    for (key, value) in mapping {
        let Some(key) = key.as_str().filter(|key| OVERRIDE_KEYS.contains(key)) else {
            continue;
        };
        if matches!(key, "network-allow" | "network-deny") {
            errors.push(ModelPolicyRuleParseError::NetworkHostsNotAllowed {
                key: key.to_string(),
            });
            continue;
        }
        let invalid = |found: String| ModelPolicyRuleParseError::OverrideValueInvalid {
            key: key.to_string(),
            found,
            allowed: allowed_override_values(key),
        };
        let Some(raw) = value.as_str() else {
            errors.push(invalid(crate::ordered::plain(value)));
            continue;
        };
        let parsed = match key {
            "effort" => EffortLevel::parse(raw).map(|value| value.as_str().to_string()),
            "sandbox" => SandboxMode::parse(raw).map(|value| value.as_str().to_string()),
            "approval" => ApprovalMode::parse(raw).map(|value| value.as_str().to_string()),
            _ => NetworkPolicy::parse(raw).map(|value| value.as_str().to_string()),
        };
        let Some(parsed) = parsed else {
            errors.push(invalid(raw.to_string()));
            continue;
        };
        match key {
            "effort" => overrides.effort = Some(parsed),
            "sandbox" => overrides.sandbox = Some(parsed),
            "approval" => overrides.approval = Some(parsed),
            _ => overrides.network = Some(parsed),
        }
    }
    overrides
}

fn allowed_override_values(key: &str) -> &'static str {
    match key {
        "effort" => EffortLevel::ALLOWED,
        "sandbox" => SandboxMode::ALLOWED,
        "approval" => ApprovalMode::ALLOWED,
        "network" => NetworkPolicy::ALLOWED,
        _ => "effort, sandbox, approval, network",
    }
}

impl<'de> Deserialize<'de> for ModelPolicyRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// Takes the keys a rule reads and hands every other value to serde as ignored, so a
        /// reader that reports ignored keys sees an unknown field of a rule or of its `override`.
        struct RuleVisitor;

        impl<'de> serde::de::Visitor<'de> for RuleVisitor {
            type Value = ModelPolicyRule;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a model policy rule mapping")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut rule = serde_yaml::Mapping::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key == "override" {
                        rule.insert(key.into(), map.next_value_seed(OverrideSeed)?);
                    } else if RULE_KEYS.contains(&key.as_str()) {
                        rule.insert(key.into(), map.next_value::<serde_yaml::Value>()?);
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                read_model_policy_rule(&serde_yaml::Value::Mapping(rule))
                    .rule
                    .map_err(|errors| {
                        serde::de::Error::custom(
                            errors
                                .iter()
                                .map(ModelPolicyRuleParseError::deserialize_message)
                                .collect::<Vec<_>>()
                                .join("; "),
                        )
                    })
            }
        }

        /// An `override` table with only the keys Mars reads; the others are ignored values.
        struct OverrideSeed;

        impl<'de> serde::de::DeserializeSeed<'de> for OverrideSeed {
            type Value = serde_yaml::Value;

            fn deserialize<D: serde::Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<Self::Value, D::Error> {
                deserializer.deserialize_any(self)
            }
        }

        impl<'de> serde::de::Visitor<'de> for OverrideSeed {
            type Value = serde_yaml::Value;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a model policy `override` mapping")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(serde_yaml::Value::Null)
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut overrides = serde_yaml::Mapping::new();
                while let Some(key) = map.next_key::<String>()? {
                    if OVERRIDE_KEYS.contains(&key.as_str()) {
                        overrides.insert(key.into(), map.next_value::<serde_yaml::Value>()?);
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                Ok(serde_yaml::Value::Mapping(overrides))
            }
        }

        deserializer.deserialize_map(RuleVisitor)
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
        if let Some(when) = &self.when {
            map.serialize_entry("when", when)?;
        }
        map.end()
    }
}
