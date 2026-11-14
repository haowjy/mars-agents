//! Canonical authored tool-policy values and frontmatter parsing.

use crate::frontmatter::Frontmatter;
use crate::tool_names::{Capability, ParsedToolName, parse_mars_tool_name, render_scoped};
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_yaml::Value;

/// What a rule does when its capability (and pattern) match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
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

    /// How restrictive the action is; the stricter action of two rules that match a call wins.
    pub fn strictness(self) -> u8 {
        match self {
            Self::Allow => 0,
            Self::Ask => 1,
            Self::Deny => 2,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "ask" => Some(Self::Ask),
            _ => None,
        }
    }
}

/// Where a rule came from. It is not part of a rule's identity: two rules with the same
/// capability, pattern and action are one rule whatever their origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuleOrigin {
    /// The agent's or skill's own profile.
    #[default]
    Profile,
    /// `allowed` / `disallowed` or a rule map in an `[agents.<name>.tools]` overlay.
    Overlay,
    /// `[settings.tools]`, which holds for every agent.
    Project,
}

impl RuleOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Overlay => "overlay",
            Self::Project => "project",
        }
    }
}

/// One tool-policy statement: `action` for `capability`, optionally narrowed by `pattern`.
///
/// Rules are kept in profile order, which carries no precedence: of the rules that match a call,
/// the strictest action wins (deny, then ask, then allow), however narrow a key is.
/// [`EffectiveToolPolicy::project`] applies the same order to a native name two rules share, and
/// gives a rule that a stricter one covers no native name.
#[derive(Debug, Clone)]
pub struct ToolRule {
    pub capability: Capability,
    /// Scope inside `capability(...)`. `None` for `*` and `mcp`, whose scope is the capability.
    pub pattern: Option<String>,
    pub action: ToolAction,
    pub origin: RuleOrigin,
}

/// Equality ignores `origin`: `merge_permission_set` and `push_unique` rely on it to fold a
/// rule that two layers both state into one.
impl PartialEq for ToolRule {
    fn eq(&self, other: &Self) -> bool {
        self.capability == other.capability
            && self.pattern == other.pattern
            && self.action == other.action
    }
}

impl Eq for ToolRule {}

impl ToolRule {
    pub fn from_parsed(parsed: ParsedToolName, action: ToolAction) -> Self {
        Self {
            capability: parsed.capability,
            pattern: parsed.pattern,
            action,
            origin: RuleOrigin::Profile,
        }
    }

    /// A rule from its wire shape (`{capability, pattern, action}`, as `agent::read` reports
    /// it and `Serialize` writes it), parsed like the authored key it renders to.
    pub fn from_wire(
        capability: &str,
        pattern: Option<&str>,
        action: ToolAction,
    ) -> Result<Self, crate::tool_names::ToolNameParseError> {
        let key = match (capability, pattern) {
            ("mcp", Some(pattern)) => format!("mcp({pattern})"),
            _ => render_scoped(capability, pattern),
        };
        Ok(Self::from_parsed(parse_mars_tool_name(&key)?, action))
    }

    pub fn with_origin(mut self, origin: RuleOrigin) -> Self {
        self.origin = origin;
        self
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
/// `{capability, pattern, action, origin}`. `capability` is `*` for "every other tool" and
/// `mcp` for an MCP ref, whose `pattern` is its `server/tool`.
impl Serialize for ToolRule {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("capability", self.capability.name())?;
        map.serialize_entry("pattern", &self.wire_pattern())?;
        map.serialize_entry("action", self.action.as_str())?;
        map.serialize_entry("origin", self.origin.as_str())?;
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
    /// Add `rule` unless an equal one is there. The first rule's origin stays: consumers act on
    /// origin (a continued session replaces `Project` rules with the current ones, and the
    /// platform `task` opt-out reads only profile and overlay rules), and the rule already
    /// held says what the copy would.
    pub fn push_unique(&mut self, rule: ToolRule) {
        if !self.rules.contains(&rule) {
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

/// The spellings of an Agent's deny list that Claude and pre-v1 Mars wrote; every translation
/// reads them as one set, and each name in it is a `deny` rule.
pub(crate) const DISALLOWED_SPELLINGS: [&str; 3] =
    ["disallowedTools", "disallowed-tools", "disallowed_tools"];

/// The tool names in a `tools:` / `disallowed-tools:` value. A scalar is Claude's own
/// agent-file spelling, `tools: Read, Grep`: a comma-separated list. Commas inside a scope
/// (`bash(rg a,b)`) belong to the entry.
pub(crate) fn tool_entries(val: &Value) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(origin: RuleOrigin) -> ToolRule {
        ToolRule {
            capability: Capability::Builtin("bash"),
            pattern: Some("git push *".into()),
            action: ToolAction::Deny,
            origin,
        }
    }

    #[test]
    fn origin_is_not_part_of_identity() {
        assert_eq!(rule(RuleOrigin::Profile), rule(RuleOrigin::Project));

        let mut policy = EffectiveToolPolicy::default();
        policy.push_unique(rule(RuleOrigin::Profile));
        policy.push_unique(rule(RuleOrigin::Overlay));
        assert_eq!(policy.rules.len(), 1);
        assert_eq!(policy.rules[0].origin, RuleOrigin::Profile);
        policy.push_unique(rule(RuleOrigin::Project));
        assert_eq!(policy.rules.len(), 1);
        assert_eq!(policy.rules[0].origin, RuleOrigin::Profile);
    }
}
