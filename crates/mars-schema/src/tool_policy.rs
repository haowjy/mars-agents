//! Canonical authored tool-policy values and frontmatter parsing.

use crate::frontmatter::Frontmatter;
use crate::tool_names::{Capability, ParsedToolName, parse_mars_tool_name, render_scoped};
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_yaml::Value;

/// What a rule does when its capability (and pattern) match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolAction {
    Allow,
    Deny,
    Ask,
}

impl ToolAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Ask => "ask",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "ask" => Some(Self::Ask),
            _ => None,
        }
    }
}

/// One tool-policy statement: `action` for `capability`, optionally narrowed by `pattern`.
///
/// Rules are kept in profile order, which carries no precedence: a broad rule against a scoped
/// one is the consuming harness's call, and [`EffectiveToolPolicy::project`] settles only
/// capabilities one harness tool cannot tell apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRule {
    pub capability: Capability,
    /// Scope inside `capability(...)`. `None` for `*` and `mcp`, whose scope is the capability.
    pub pattern: Option<String>,
    pub action: ToolAction,
}

impl ToolRule {
    pub fn from_parsed(parsed: ParsedToolName, action: ToolAction) -> Self {
        Self {
            capability: parsed.capability,
            pattern: parsed.pattern,
            action,
        }
    }

    pub fn is_wildcard(&self) -> bool {
        matches!(self.capability, Capability::Any)
    }

    pub fn is_mcp(&self) -> bool {
        matches!(self.capability, Capability::Mcp(_))
    }

    /// Canonical `capability(pattern)` spelling, for diagnostics and frontmatter.
    pub fn render(&self) -> String {
        match &self.capability {
            Capability::Mcp(mcp_ref) => mcp_ref.to_canonical(),
            capability => render_scoped(capability.name(), self.pattern.as_deref()),
        }
    }

    /// The pattern as the bundle and `show --json` print it: an `mcp` rule's `server/tool`.
    fn wire_pattern(&self) -> Option<String> {
        match &self.capability {
            Capability::Mcp(mcp_ref) => Some(mcp_ref.to_pattern()),
            _ => self.pattern.clone(),
        }
    }
}

/// The rule's wire shape, shared by the launch bundle's `tools.rules` and `agents show --json`:
/// `{capability, pattern, action}`. `capability` is `*` for "every other tool" and
/// `mcp` for an MCP ref, whose `pattern` is its `server/tool`.
impl Serialize for ToolRule {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("capability", self.capability.name())?;
        map.serialize_entry("pattern", &self.wire_pattern())?;
        map.serialize_entry("action", self.action.as_str())?;
        map.end()
    }
}

/// The tool rules of an agent or skill.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
#[serde(transparent)]
pub struct EffectiveToolPolicy {
    pub rules: Vec<ToolRule>,
}

impl EffectiveToolPolicy {
    pub fn with_action(&self, action: ToolAction) -> impl Iterator<Item = &ToolRule> {
        self.rules.iter().filter(move |rule| rule.action == action)
    }
    pub fn describe(&self) -> Vec<String> {
        self.rules
            .iter()
            .map(|rule| format!("{}:{}", rule.action.as_str(), rule.render()))
            .collect()
    }
    pub fn rendered(&self, action: ToolAction) -> Vec<String> {
        self.with_action(action)
            .filter(|rule| !rule.is_wildcard())
            .map(ToolRule::render)
            .collect()
    }
    pub fn is_allowlist(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.is_wildcard() && rule.action == ToolAction::Deny)
    }
    pub fn push_unique(&mut self, rule: ToolRule) {
        if !self.rules.iter().any(|existing| {
            existing.capability == rule.capability
                && existing.pattern == rule.pattern
                && existing.action == rule.action
        }) {
            self.rules.push(rule);
        }
    }
}

/// Append whole-server `mcp(server)` entries to the frontmatter `tools:` list or map.
pub fn append_mcp_server_entries_to_tools(fm: &mut Frontmatter, servers: &[String]) -> bool {
    let entries: Vec<String> = servers
        .iter()
        .map(|server| server.trim())
        .filter(|server| !server.is_empty())
        .map(|server| format!("mcp({server})"))
        .collect();
    if entries.is_empty() {
        return false;
    }

    let Some(existing) = fm.get("tools").cloned() else {
        // Map form: an absent `tools:` inherits every tool, and a list here would read as an
        // allowlist of only these MCP servers.
        let grants = entries
            .into_iter()
            .map(|entry| (Value::String(entry), Value::String("allow".into())))
            .collect::<serde_yaml::Mapping>();
        fm.insert("tools", Value::Mapping(grants));
        return true;
    };

    match existing {
        Value::Mapping(mut mapping) => {
            let mut changed = false;
            for entry in entries {
                let key = Value::String(entry);
                if !mapping.contains_key(&key) {
                    mapping.insert(key, Value::String("allow".into()));
                    changed = true;
                }
            }
            if changed {
                fm.insert("tools", Value::Mapping(mapping));
            }
            changed
        }
        other => {
            let mut tools = tool_entries(&other);
            let mut changed = false;
            for entry in entries {
                if !tools.iter().any(|existing| existing == &entry) {
                    tools.push(entry);
                    changed = true;
                }
            }
            if changed {
                fm.insert(
                    "tools",
                    Value::Sequence(tools.into_iter().map(Value::String).collect()),
                );
            }
            changed
        }
    }
}

/// The tool names in a `tools:` / `disallowed-tools:` value. A scalar is Claude's own
/// agent-file spelling, `tools: Read, Grep`: a comma-separated list. Commas inside a scope
/// (`bash(rg a,b)`) belong to the entry.
pub fn tool_entries(val: &Value) -> Vec<String> {
    let Value::String(raw) = val else {
        return yaml_str_list(val);
    };
    let mut entries = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, ch) in raw.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            // Claude writes tool lists comma- or space-separated; parentheses stay whole.
            ',' | ' ' | '\t' | '\n' if depth == 0 => {
                entries.push(&raw[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    entries.push(&raw[start..]);
    entries
        .into_iter()
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn yaml_str_list(val: &Value) -> Vec<String> {
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

type OnInvalid<'a> = &'a mut dyn FnMut(Vec<crate::diagnostic::PathSegment>, &str, &'static str);

/// Read a v1 profile's `tools:` action map into one policy.
///
/// `on_invalid` gets `(field, value, allowed)` for each entry Mars rejects.
pub fn parse_tool_policy(tools: Option<&Value>, on_invalid: OnInvalid<'_>) -> EffectiveToolPolicy {
    let mut policy = EffectiveToolPolicy::default();
    if let Some(value) = tools {
        for rule in parse_tools_field(value, on_invalid) {
            policy.push_unique(rule);
        }
    }
    policy
}

fn parse_tool_rule(
    path: Vec<crate::diagnostic::PathSegment>,
    raw: &str,
    action: ToolAction,
    on_invalid: OnInvalid<'_>,
) -> Option<ToolRule> {
    match parse_mars_tool_name(raw) {
        Ok(parsed) => Some(ToolRule::from_parsed(parsed, action)),
        Err(err) => {
            on_invalid(path, raw, err.allowed());
            None
        }
    }
}

/// Parse the v1 `tools:` action map.
fn parse_tools_field(val: &Value, on_invalid: OnInvalid<'_>) -> Vec<ToolRule> {
    use crate::diagnostic::{PathSegment, key_path};
    let field = key_path("tools");
    if permission_replace_entries(val).is_some() {
        on_invalid(
            field.clone(),
            &crate::ordered::plain(val),
            "plain list or mapping of tool to allow|deny|ask; package profiles cannot replace a permission set",
        );
        return Vec::new();
    }
    match val {
        Value::Mapping(mapping) => {
            let mut rules = Vec::new();
            for (key, value) in mapping {
                let Some(tool_name) = key.as_str() else {
                    on_invalid(
                        field.clone(),
                        &crate::ordered::plain(key),
                        "string tool keys",
                    );
                    continue;
                };

                let mut entry_path = field.clone();
                entry_path.push(PathSegment::Key(tool_name.to_owned()));
                let Some(policy) = value.as_str() else {
                    on_invalid(entry_path, &crate::ordered::plain(value), ACTIONS_ALLOWED);
                    continue;
                };
                let Some(action) = ToolAction::parse(policy) else {
                    on_invalid(entry_path, policy, ACTIONS_ALLOWED);
                    continue;
                };
                if let Some(rule) = parse_tool_rule(entry_path, tool_name, action, on_invalid) {
                    rules.push(rule);
                }
            }
            rules
        }
        // `tools: allow` / `tools: deny` was a Meridian-side shorthand that never worked
        // here: it read as a tool named `allow`. Say so instead of guessing.
        Value::String(raw) if raw == "allow" || raw == "deny" => {
            on_invalid(
                field,
                raw,
                "list of tool names or mapping of tool to allow|deny|ask",
            );
            Vec::new()
        }
        _ => {
            on_invalid(
                field,
                &crate::ordered::plain(val),
                "mapping of tool to allow|deny|ask",
            );
            Vec::new()
        }
    }
}

fn permission_replace_entries(value: &Value) -> Option<&Value> {
    let mapping = value.as_mapping()?;
    (mapping.len() == 1)
        .then(|| mapping.get(Value::String("replace".to_string())))
        .flatten()
}

const ACTIONS_ALLOWED: &str = "allow, deny, or ask";

/// Retired top-level MCP grant field spellings (use inline `mcp(...)` in `tools:` instead).
const REMOVED_MCP_TOOLS_FIELDS: &[&str] = &["mcp-tools", "mcp_tools"];

/// Strip retired `mcp-tools` keys from canonical frontmatter during staging lift.
pub fn strip_removed_mcp_tools_fields(fm: &mut Frontmatter) -> bool {
    let mut changed = false;
    for key in REMOVED_MCP_TOOLS_FIELDS {
        if fm.remove(key).is_some() {
            changed = true;
        }
    }
    changed
}

/// Foreign tool-field keys in canonical/MarsNative profiles → canonical replacement label.
pub const NON_CANONICAL_TOOL_FIELD_ALIASES: &[(&str, &str)] = &[
    ("allowed-tools", "tools:"),
    ("allowed_tools", "tools:"),
    ("disallowed_tools", "disallowed-tools:"),
];
