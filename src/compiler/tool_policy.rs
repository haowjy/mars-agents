//! Shared tool-policy parsing for agent and skill frontmatter.
//!
//! Canonical Mars profiles express tool gating with `tools:` (list or allow/deny map),
//! `disallowed-tools:`, and `mcp-tools:`. [`EffectiveToolPolicy`] merges those fields
//! into the portable allow/deny/mcp view both compilers use when lowering to harnesses.

use serde_yaml::Value;

use crate::compiler::tool_names::{ParsedToolName, parse_mars_tool_name};

/// Portable tool policy from top-level Mars fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveToolPolicy {
    pub allowed: Vec<String>,
    pub disallowed: Vec<String>,
    pub mcp: Vec<String>,
}

/// Parsed `tools:` field — allowlist entries and map-form denials before merge.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedToolsField {
    pub allowed: Vec<String>,
    pub denied: Vec<String>,
}

/// Merge top-level tool fields into one effective policy.
pub fn effective_tool_policy(
    allowed: &[String],
    denied: &[String],
    disallowed: &[String],
    mcp: &[String],
) -> EffectiveToolPolicy {
    EffectiveToolPolicy {
        allowed: dedupe_ordered(allowed.to_vec()),
        disallowed: dedupe_ordered(
            denied
                .iter()
                .chain(disallowed.iter())
                .cloned()
                .collect(),
        ),
        mcp: dedupe_ordered(mcp.to_vec()),
    }
}

pub(crate) fn dedupe_ordered(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_string();
        if seen.insert(key.clone()) {
            out.push(key);
        }
    }
    out
}

pub(crate) fn yaml_str_list(val: &Value) -> Vec<String> {
    match val {
        Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_owned)
            .collect(),
        Value::String(s) => vec![s.clone()],
        _ => vec![],
    }
}

fn parse_tool_name_field(
    field: &str,
    raw: &str,
    on_invalid: &mut dyn FnMut(&str, &str, &'static str),
) -> Option<String> {
    match parse_mars_tool_name(raw) {
        Ok(ParsedToolName { name, .. }) => Some(name),
        Err(err) => {
            on_invalid(field, raw, err.allowed());
            None
        }
    }
}

pub(crate) fn yaml_tool_list(
    field: &str,
    val: &Value,
    on_invalid: &mut dyn FnMut(&str, &str, &'static str),
) -> Vec<String> {
    dedupe_ordered(
        yaml_str_list(val)
            .into_iter()
            .enumerate()
            .filter_map(|(idx, tool)| {
                parse_tool_name_field(&format!("{field}[{idx}]"), &tool, on_invalid)
            })
            .collect(),
    )
}

pub(crate) fn parse_tools_field(
    field: &str,
    val: &Value,
    on_invalid: &mut dyn FnMut(&str, &str, &'static str),
) -> ParsedToolsField {
    match val {
        Value::Mapping(mapping) => {
            let mut allowed = Vec::new();
            let mut denied = Vec::new();
            for (key, value) in mapping {
                let Some(tool_name) = key.as_str() else {
                    on_invalid(field, &format!("{key:?}"), "string tool keys");
                    continue;
                };

                let Some(policy) = value.as_str() else {
                    on_invalid(
                        &format!("{field}.{tool_name}"),
                        &format!("{value:?}"),
                        "allow or deny",
                    );
                    continue;
                };

                let normalized_tool =
                    parse_tool_name_field(&format!("{field}.{tool_name}"), tool_name, on_invalid);
                if policy.eq_ignore_ascii_case("allow") {
                    if let Some(normalized_tool) = normalized_tool {
                        allowed.push(normalized_tool);
                    }
                } else if policy.eq_ignore_ascii_case("deny") {
                    if let Some(normalized_tool) = normalized_tool {
                        denied.push(normalized_tool);
                    }
                } else {
                    on_invalid(
                        &format!("{field}.{tool_name}"),
                        policy,
                        "allow or deny",
                    );
                }
            }
            ParsedToolsField {
                allowed: dedupe_ordered(allowed),
                denied: dedupe_ordered(denied),
            }
        }
        _ => ParsedToolsField {
            allowed: yaml_tool_list(field, val, on_invalid),
            denied: vec![],
        },
    }
}
