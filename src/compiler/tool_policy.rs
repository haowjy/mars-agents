//! Shared tool-policy parsing for agent and skill frontmatter.
//!
//! Canonical Mars profiles express tool gating with `tools:` (list or allow/deny map),
//! `disallowed-tools:`, and legacy `mcp-tools:` (or `mcp_tools`). Allowed MCP refs live
//! in `tools:` as `mcp(...)` entries; legacy `mcp-tools:` values normalize to the same
//! representation at policy-merge time. [`EffectiveToolPolicy`] merges those fields into
//! the portable allow/deny/mcp view both compilers use when lowering to harnesses.
//!
//! [`NON_CANONICAL_TOOL_FIELD_ALIASES`] is the single source for foreign spellings that
//! must not appear in canonical/MarsNative profiles. Staging strips alias keys; the skill
//! parser warns with the canonical replacement label.

use serde_yaml::Value;

use crate::compiler::mcp_ref::{McpRef, mcp_ref_from_legacy_server_name, try_parse_mcp_tool_name};
use crate::compiler::tool_names::{ParsedToolName, parse_mars_tool_name};
use crate::frontmatter::Frontmatter;

/// Portable tool policy from top-level Mars fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveToolPolicy {
    pub allowed: Vec<String>,
    pub disallowed: Vec<String>,
    pub(crate) mcp_allowed: Vec<McpRef>,
    pub(crate) mcp_disallowed: Vec<McpRef>,
}

/// Parsed `tools:` field — allowlist entries and map-form denials before merge.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedToolsField {
    pub allowed: Vec<String>,
    pub denied: Vec<String>,
}

/// Canonical frontmatter key for legacy whole-server MCP allowlists.
pub const MCP_TOOLS_FIELD: &str = "mcp-tools";

/// Snake-case alias accepted on both agents and skills (canonical key is kebab-case).
pub const MCP_TOOLS_FIELD_ALIASES: &[&str] = &["mcp_tools"];

/// Read legacy `mcp-tools:` / `mcp_tools` from frontmatter (shared agent + skill path).
pub(crate) fn legacy_mcp_tools_from_frontmatter(fm: &Frontmatter) -> Vec<String> {
    fm.get(MCP_TOOLS_FIELD)
        .or_else(|| fm.get(MCP_TOOLS_FIELD_ALIASES[0]))
        .map(yaml_str_list)
        .unwrap_or_default()
}

/// Merge top-level tool fields into one effective policy.
///
/// Allowed MCP refs may appear in `allowed` as `mcp(...)` tool-list entries and/or via
/// legacy `legacy_mcp` whole-server names. Disallowed MCP refs appear in `denied` /
/// `disallowed` as canonical `mcp(...)` strings. Harness emission projects structured
/// [`McpRef`] values per target via [`crate::compiler::mcp_ref::project_mcp_ref`].
pub fn effective_tool_policy(
    allowed: &[String],
    denied: &[String],
    disallowed: &[String],
    legacy_mcp: &[String],
) -> EffectiveToolPolicy {
    let mut allowed_tools = Vec::new();
    let mut mcp_allowed = Vec::new();

    for tool in allowed {
        if let Some(mcp_ref) = try_parse_mcp_tool_name(tool) {
            mcp_allowed.push(mcp_ref);
        } else {
            allowed_tools.push(tool.clone());
        }
    }

    for server in legacy_mcp {
        let trimmed = server.trim();
        if !trimmed.is_empty() {
            mcp_allowed.push(mcp_ref_from_legacy_server_name(trimmed));
        }
    }

    let mut disallowed_tools = Vec::new();
    let mut mcp_disallowed = Vec::new();
    for tool in denied.iter().chain(disallowed.iter()) {
        if let Some(mcp_ref) = try_parse_mcp_tool_name(tool) {
            mcp_disallowed.push(mcp_ref);
        } else {
            disallowed_tools.push(tool.clone());
        }
    }

    EffectiveToolPolicy {
        allowed: dedupe_ordered(allowed_tools),
        disallowed: dedupe_ordered(disallowed_tools),
        mcp_allowed: dedupe_mcp_refs(mcp_allowed),
        mcp_disallowed: dedupe_mcp_refs(mcp_disallowed),
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

fn dedupe_mcp_refs(refs: Vec<McpRef>) -> Vec<McpRef> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for mcp_ref in refs {
        let key = mcp_ref.to_canonical();
        if seen.insert(key) {
            out.push(mcp_ref);
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
                    on_invalid(&format!("{field}.{tool_name}"), policy, "allow or deny");
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

/// Foreign tool-field keys in canonical/MarsNative profiles → canonical replacement label.
pub const NON_CANONICAL_TOOL_FIELD_ALIASES: &[(&str, &str)] = &[
    ("allowed-tools", "tools:"),
    ("allowed_tools", "tools:"),
    ("disallowed_tools", "disallowed-tools:"),
];

fn canonical_key_from_label(label: &str) -> &str {
    label.strip_suffix(':').unwrap_or(label)
}

/// Alias keys that map to a canonical tool field (without trailing colon).
pub(crate) fn non_canonical_aliases_for(canonical_key: &str) -> Vec<&'static str> {
    NON_CANONICAL_TOOL_FIELD_ALIASES
        .iter()
        .filter(|&(_, label)| canonical_key_from_label(label) == canonical_key)
        .map(|&(alias, _)| alias)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::agents::{AgentDiagnostic, parse_agent_content};
    use crate::compiler::mcp_ref::{McpSegment, parse_mcp_ref};
    use crate::compiler::skills::{SkillDiagnostic, parse_skill_content};
    use crate::frontmatter::Frontmatter;

    mod non_canonical_alias_tests {
        use super::*;

        #[test]
        fn aliases_grouped_by_canonical_field() {
            assert_eq!(
                non_canonical_aliases_for("tools"),
                vec!["allowed-tools", "allowed_tools"]
            );
            assert_eq!(
                non_canonical_aliases_for("disallowed-tools"),
                vec!["disallowed_tools"]
            );
            assert!(non_canonical_aliases_for("mcp-tools").is_empty());
        }
    }

    fn agent_policy(yaml: &str) -> EffectiveToolPolicy {
        let mut diags = Vec::new();
        let (profile, _) = parse_agent_content(yaml, &mut diags).unwrap();
        assert!(diags.is_empty(), "agent diags: {diags:?}");
        profile.effective_tool_policy(&crate::compiler::agents::HarnessKind::Claude)
    }

    fn skill_policy(yaml: &str) -> EffectiveToolPolicy {
        let mut diags = Vec::new();
        let (profile, _) = parse_skill_content(yaml, &mut diags).unwrap();
        assert!(diags.is_empty(), "skill diags: {diags:?}");
        profile.effective_tool_policy()
    }

    #[test]
    fn tools_mcp_entry_matches_legacy_mcp_tools_field() {
        let legacy = agent_policy("---\nname: a\ndescription: d\nmcp-tools: [context7]\n---\n");
        let inline = agent_policy("---\nname: a\ndescription: d\ntools: [mcp(context7)]\n---\n");
        assert_eq!(legacy.mcp_allowed, inline.mcp_allowed);
        assert!(legacy.allowed.is_empty());
        assert_eq!(
            legacy.mcp_allowed,
            vec![McpRef {
                server: McpSegment::Named("context7".into()),
                tool: McpSegment::Any,
            }]
        );
    }

    #[test]
    fn skill_tools_mcp_entry_matches_legacy_mcp_tools_field() {
        let legacy = skill_policy("---\nname: a\ndescription: d\nmcp-tools: [context7]\n---\nbody");
        let inline =
            skill_policy("---\nname: a\ndescription: d\ntools: [mcp(context7)]\n---\nbody");
        assert_eq!(legacy.mcp_allowed, inline.mcp_allowed);
        assert_eq!(legacy.mcp_allowed.len(), 1);
    }

    #[test]
    fn agent_and_skill_accept_mcp_tools_snake_alias() {
        let agent_yaml = "---\nname: a\ndescription: d\nmcp_tools: [plugin:x]\n---\n";
        let skill_yaml = "---\nname: a\ndescription: d\nmcp_tools: [plugin:x]\n---\nbody";

        let mut agent_diags = Vec::<AgentDiagnostic>::new();
        let (agent, _) = parse_agent_content(agent_yaml, &mut agent_diags).unwrap();
        assert!(agent_diags.is_empty());
        assert_eq!(agent.mcp_tools, vec!["plugin:x"]);

        let mut skill_diags = Vec::<SkillDiagnostic>::new();
        let (skill, _) = parse_skill_content(skill_yaml, &mut skill_diags).unwrap();
        assert!(skill_diags.is_empty());
        assert_eq!(skill.mcp_tools, vec!["plugin:x"]);
    }

    #[test]
    fn disallowed_mcp_ref_round_trips_through_policy() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ndisallowed-tools: [mcp(github/delete_repo)]\n---\n",
        );
        assert!(policy.allowed.is_empty());
        assert!(policy.mcp_allowed.is_empty());
        assert!(policy.disallowed.is_empty());
        assert_eq!(policy.mcp_disallowed.len(), 1);
        assert_eq!(
            policy.mcp_disallowed[0].to_canonical(),
            "mcp(github/delete_repo)"
        );
    }

    #[test]
    fn legacy_mcp_tools_from_frontmatter_unifies_kebab_and_snake() {
        let kebab = Frontmatter::parse("---\nmcp-tools: [a]\n---\n").unwrap();
        let snake = Frontmatter::parse("---\nmcp_tools: [b]\n---\n").unwrap();
        assert_eq!(legacy_mcp_tools_from_frontmatter(&kebab), vec!["a"]);
        assert_eq!(legacy_mcp_tools_from_frontmatter(&snake), vec!["b"]);
    }

    #[test]
    fn plugin_colon_server_names_preserve_verbatim_in_mcp_refs() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools: [mcp(plugin:context7:context7)]\n---\n",
        );
        assert_eq!(policy.mcp_allowed.len(), 1);
        assert_eq!(
            policy.mcp_allowed[0],
            parse_mcp_ref("plugin:context7:context7").unwrap()
        );
    }
}
