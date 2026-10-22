//! Shared tool-policy parsing for agent and skill frontmatter.
//!
//! Canonical Mars profiles express tool gating with `tools:` (list or allow/deny map),
//! `disallowed-tools:`, and whole-server or per-tool MCP grants as `mcp(...)` entries in
//! those lists. [`EffectiveToolPolicy`] merges those fields into the portable allow/deny/mcp
//! view both compilers use when lowering to harnesses.
//!
//! [`NON_CANONICAL_TOOL_FIELD_ALIASES`] is the single source for foreign spellings that
//! must not appear in canonical/MarsNative profiles. Staging strips alias keys; the skill
//! parser warns with the canonical replacement label.

use serde_yaml::Value;

use crate::compiler::agents::HarnessKind;
use crate::compiler::mcp_ref::{
    McpProjection, McpUnsupportedReason, parse_mcp_ref, project_mcp_ref,
};
use crate::compiler::tool_names::{
    ANY_CAPABILITY, MCP_CAPABILITY, NativeStatus, ParsedToolName, native_names,
    parse_mars_tool_name, render_scoped,
};
use crate::frontmatter::Frontmatter;

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
        match raw.trim().to_ascii_lowercase().as_str() {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "ask" => Some(Self::Ask),
            _ => None,
        }
    }
}

/// Why a rule exists when the author did not write it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleOrigin {
    /// The `*: deny` Mars adds for a list-form `tools: [read, grep]`, which means
    /// "exactly these tools".
    ListForm,
}

impl RuleOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ListForm => "list-form",
        }
    }
}

/// One tool-policy statement: `action` for `capability`, optionally narrowed by `pattern`.
///
/// Rules carry no order: precedence between a broad and a scoped rule is the consuming
/// harness's, not Mars's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRule {
    /// Canonical capability, `*` (everything else), `mcp`, or the author's spelling when
    /// Mars does not know the name.
    pub capability: String,
    /// Scope inside `capability(...)`. For `mcp` the canonical `server/tool` reference.
    pub pattern: Option<String>,
    pub action: ToolAction,
    pub origin: Option<RuleOrigin>,
}

impl ToolRule {
    fn from_parsed(parsed: ParsedToolName, action: ToolAction) -> Self {
        Self {
            capability: parsed.capability,
            pattern: parsed.pattern,
            action,
            origin: None,
        }
    }

    /// The `*: deny` that turns a list into an allowlist.
    fn list_form_allowlist() -> Self {
        Self {
            capability: ANY_CAPABILITY.to_string(),
            pattern: None,
            action: ToolAction::Deny,
            origin: Some(RuleOrigin::ListForm),
        }
    }

    pub fn is_wildcard(&self) -> bool {
        self.capability == ANY_CAPABILITY
    }

    pub fn is_mcp(&self) -> bool {
        self.capability == MCP_CAPABILITY
    }

    /// Canonical `capability(pattern)` spelling, for diagnostics.
    pub fn render(&self) -> String {
        render_scoped(&self.capability, self.pattern.as_deref())
    }

    fn mcp_ref(&self) -> Option<crate::compiler::mcp_ref::McpRef> {
        if !self.is_mcp() {
            return None;
        }
        parse_mcp_ref(self.pattern.as_deref()?).ok()
    }
}

/// A rule as one harness spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectedRule {
    /// Native names, one per native tool the capability covers (`edit` → `Edit`, `Write`, …),
    /// with the pattern appended when the rule is scoped.
    pub native: Vec<String>,
    pub status: NativeStatus,
    /// Why Mars cannot express an MCP rule on this harness.
    pub note: Option<&'static str>,
}

/// Project `rule` onto `harness`. The one place a capability becomes native names.
pub(crate) fn project_rule(rule: &ToolRule, harness: HarnessKind) -> ProjectedRule {
    if rule.is_wildcard() {
        return ProjectedRule {
            native: Vec::new(),
            status: NativeStatus::Known,
            note: None,
        };
    }

    if rule.is_mcp() {
        let projection = rule
            .mcp_ref()
            .map(|mcp_ref| project_mcp_ref(&mcp_ref, harness));
        return match projection {
            Some(McpProjection::Token(token)) => ProjectedRule {
                native: vec![token],
                status: NativeStatus::Known,
                note: None,
            },
            Some(McpProjection::Unsupported(reason)) => ProjectedRule {
                native: Vec::new(),
                status: NativeStatus::Unknown,
                note: Some(McpUnsupportedReason::message(reason)),
            },
            None => ProjectedRule {
                native: Vec::new(),
                status: NativeStatus::Unknown,
                note: Some("MCP reference could not be read"),
            },
        };
    }

    let natives = native_names(&rule.capability, harness, rule.pattern.is_some());
    ProjectedRule {
        native: natives
            .names
            .iter()
            .map(|name| render_scoped(name, rule.pattern.as_deref()))
            .collect(),
        status: natives.status,
        note: None,
    }
}

/// The merged tool rules both compilers lower to harnesses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveToolPolicy {
    pub rules: Vec<ToolRule>,
}

impl EffectiveToolPolicy {
    pub fn with_action(&self, action: ToolAction) -> impl Iterator<Item = &ToolRule> {
        self.rules.iter().filter(move |rule| rule.action == action)
    }

    /// Every rule spelled for `harness`. The one projection both lowerers and the launch
    /// bundle use.
    pub(crate) fn project(&self, harness: HarnessKind) -> PolicyProjection<'_> {
        let mut rules: Vec<(&ToolRule, ProjectedRule)> = self
            .rules
            .iter()
            .map(|rule| (rule, project_rule(rule, harness)))
            .collect();
        resolve_by_specificity(&mut rules);
        PolicyProjection { rules }
    }

    /// Compact `action:capability(pattern)` spellings, for assertions.
    #[cfg(test)]
    pub(crate) fn describe(&self) -> Vec<String> {
        self.rules
            .iter()
            .map(|rule| format!("{}:{}", rule.action.as_str(), rule.render()))
            .collect()
    }

    /// Whether a `*: deny` limits the agent to the tools its allow rules name.
    pub fn is_allowlist(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.is_wildcard() && rule.action == ToolAction::Deny)
    }
}

/// A policy's rules spelled for one harness, in profile order.
pub(crate) struct PolicyProjection<'a> {
    rules: Vec<(&'a ToolRule, ProjectedRule)>,
}

impl<'a> PolicyProjection<'a> {
    pub fn rules(&self) -> &[(&'a ToolRule, ProjectedRule)] {
        &self.rules
    }

    /// Rules with `action`: built-in tools first, then MCP refs.
    pub fn with_action(&self, action: ToolAction) -> Vec<(&'a ToolRule, &ProjectedRule)> {
        let (mcp, builtin): (Vec<_>, Vec<_>) = self
            .rules
            .iter()
            .filter(|(rule, _)| rule.action == action)
            .map(|(rule, projected)| (*rule, projected))
            .partition(|(rule, _)| rule.is_mcp());
        builtin.into_iter().chain(mcp).collect()
    }

    /// Native names that an allow rule and a deny rule both still name after
    /// [`resolve_by_specificity`]: two capabilities one harness tool cannot tell apart
    /// (`write` and `edit` on Codex). The harness decides which wins; a deny usually does.
    pub fn overlaps(&self) -> Vec<String> {
        let named = |action| -> Vec<&String> {
            self.rules
                .iter()
                .filter(|(rule, _)| rule.action == action)
                .flat_map(|(_, projected)| &projected.native)
                .collect()
        };
        let allowed = named(ToolAction::Allow);
        let mut overlaps: Vec<String> = Vec::new();
        for native in named(ToolAction::Deny) {
            if allowed.contains(&native) && !overlaps.contains(native) {
                overlaps.push(native.clone());
            }
        }
        overlaps
    }
}

/// Let a rule on a narrower capability win over a broader one for the natives they share.
///
/// `edit: deny` with `write: allow` means "no edits, but writing is fine". On Claude `edit`
/// covers `Edit`, `Write` and `NotebookEdit`, so projecting both rules as written would put
/// `Write` in the allow and the deny list, where the deny wins. A capability is narrower when
/// the natives it names are a strict subset of the other's, and the rules disagree on the
/// action. Capabilities a harness cannot tell apart (equal sets) stay in conflict: see
/// [`PolicyProjection::overlaps`]. Scoped rules compare by their rendered `name(pattern)`,
/// so a scope never narrows an unscoped rule and the harness's own pattern precedence applies.
fn resolve_by_specificity(rules: &mut [(&ToolRule, ProjectedRule)]) {
    let narrower: Vec<Option<(ToolAction, Vec<String>)>> = rules
        .iter()
        .map(|(rule, projected)| {
            let plain = !rule.is_wildcard() && !rule.is_mcp() && !projected.native.is_empty();
            plain.then(|| (rule.action, projected.native.clone()))
        })
        .collect();

    for (rule, projected) in rules.iter_mut() {
        if rule.is_wildcard() || rule.is_mcp() {
            continue;
        }
        let broad = projected.native.clone();
        projected.native.retain(|native| {
            !narrower.iter().flatten().any(|(action, natives)| {
                *action != rule.action
                    && natives.len() < broad.len()
                    && natives.iter().all(|name| broad.contains(name))
                    && natives.contains(native)
            })
        });
    }
}

/// Parsed `tools:` field: rules plus the shape the author wrote them in.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedToolsField {
    pub rules: Vec<ToolRule>,
    /// `tools: [read, grep]`. A map (`tools: {read: allow}`) is not an allowlist.
    pub list_form: bool,
}

impl ParsedToolsField {
    /// Canonical spellings of this field's rules with `action`, for inspection output.
    pub fn rendered(&self, action: ToolAction) -> Vec<String> {
        render_rules(&self.rules, action)
    }
}

/// Canonical spellings of the rules in `rules` with `action`.
pub fn render_rules(rules: &[ToolRule], action: ToolAction) -> Vec<String> {
    rules
        .iter()
        .filter(|rule| rule.action == action)
        .map(ToolRule::render)
        .collect()
}

/// Rules for the tool names in a `mars.toml` agent overlay (`[agents.x.tools]`).
///
/// A name Mars cannot parse is kept as an unknown capability rather than dropped: losing a
/// `disallowed` entry would widen the agent.
pub(crate) fn overlay_rules(names: &[String], action: ToolAction) -> Vec<ToolRule> {
    names
        .iter()
        .map(|name| match parse_mars_tool_name(name) {
            Ok(parsed) => ToolRule::from_parsed(parsed, action),
            Err(_) => ToolRule {
                capability: name.trim().to_string(),
                pattern: None,
                action,
                origin: None,
            },
        })
        .collect()
}

/// Merge the `tools:` and `disallowed-tools:` fields of an agent into one rule set.
///
/// A list-form `tools:` is an allowlist: Mars adds the `*: deny` the author implied.
/// An empty list names no tools, so it stays "no policy" rather than "deny everything".
pub fn agent_tool_policy(tools: &ParsedToolsField, disallowed: &[ToolRule]) -> EffectiveToolPolicy {
    merge_rules(tools, disallowed, tools.list_form)
}

/// Same merge for a skill, where `tools:` pre-approves tools while the skill runs and
/// never restricts the rest.
pub fn skill_tool_policy(tools: &ParsedToolsField, disallowed: &[ToolRule]) -> EffectiveToolPolicy {
    merge_rules(tools, disallowed, false)
}

fn merge_rules(
    tools: &ParsedToolsField,
    disallowed: &[ToolRule],
    allowlist: bool,
) -> EffectiveToolPolicy {
    let mut rules = Vec::new();
    if allowlist && !tools.rules.is_empty() {
        rules.push(ToolRule::list_form_allowlist());
    }
    rules.extend(tools.rules.iter().cloned());
    rules.extend(disallowed.iter().cloned());
    EffectiveToolPolicy {
        rules: dedupe_rules(rules),
    }
}

fn dedupe_rules(rules: Vec<ToolRule>) -> Vec<ToolRule> {
    let mut seen = std::collections::HashSet::new();
    rules
        .into_iter()
        .filter(|rule| seen.insert((rule.capability.clone(), rule.pattern.clone(), rule.action)))
        .collect()
}

/// Append whole-server `mcp(server)` entries to the frontmatter `tools:` list or map.
pub(crate) fn append_mcp_server_entries_to_tools(fm: &mut Frontmatter, servers: &[String]) -> bool {
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
            ',' if depth == 0 => {
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

fn parse_tool_rule(
    field: &str,
    raw: &str,
    action: ToolAction,
    on_invalid: &mut dyn FnMut(&str, &str, &'static str),
) -> Option<ToolRule> {
    match parse_mars_tool_name(raw) {
        Ok(parsed) => Some(ToolRule::from_parsed(parsed, action)),
        Err(err) => {
            on_invalid(field, raw, err.allowed());
            None
        }
    }
}

/// Parse a `disallowed-tools:` list into deny rules.
pub(crate) fn yaml_tool_list(
    field: &str,
    val: &Value,
    on_invalid: &mut dyn FnMut(&str, &str, &'static str),
) -> Vec<ToolRule> {
    tool_entries(val)
        .iter()
        .enumerate()
        .filter_map(|(idx, tool)| {
            parse_tool_rule(
                &format!("{field}[{idx}]"),
                tool,
                ToolAction::Deny,
                on_invalid,
            )
        })
        .collect()
}

/// Parse `tools:`: a list of tools to allow, or a map of tool to `allow` / `deny` / `ask`.
pub(crate) fn parse_tools_field(
    field: &str,
    val: &Value,
    on_invalid: &mut dyn FnMut(&str, &str, &'static str),
) -> ParsedToolsField {
    match val {
        Value::Mapping(mapping) => {
            let mut rules = Vec::new();
            for (key, value) in mapping {
                let Some(tool_name) = key.as_str() else {
                    on_invalid(field, &format!("{key:?}"), "string tool keys");
                    continue;
                };

                let entry_field = format!("{field}.{tool_name}");
                let Some(policy) = value.as_str() else {
                    on_invalid(&entry_field, &format!("{value:?}"), ACTIONS_ALLOWED);
                    continue;
                };
                let Some(action) = ToolAction::parse(policy) else {
                    on_invalid(&entry_field, policy, ACTIONS_ALLOWED);
                    continue;
                };
                if let Some(rule) = parse_tool_rule(&entry_field, tool_name, action, on_invalid) {
                    rules.push(rule);
                }
            }
            ParsedToolsField {
                rules,
                list_form: false,
            }
        }
        // `tools: allow` / `tools: deny` was a Meridian-side shorthand that never worked
        // here: it read as a tool named `allow`. Say so instead of guessing.
        Value::String(raw)
            if raw.trim().eq_ignore_ascii_case("allow")
                || raw.trim().eq_ignore_ascii_case("deny") =>
        {
            on_invalid(
                field,
                raw,
                "list of tool names or mapping of tool to allow|deny|ask",
            );
            ParsedToolsField::default()
        }
        _ => ParsedToolsField {
            rules: tool_entries(val)
                .iter()
                .enumerate()
                .filter_map(|(idx, tool)| {
                    parse_tool_rule(
                        &format!("{field}[{idx}]"),
                        tool,
                        ToolAction::Allow,
                        on_invalid,
                    )
                })
                .collect(),
            list_form: true,
        },
    }
}

const ACTIONS_ALLOWED: &str = "allow, deny, or ask";

/// Retired top-level MCP grant field spellings (use inline `mcp(...)` in `tools:` instead).
pub const REMOVED_MCP_TOOLS_FIELDS: &[&str] = &["mcp-tools", "mcp_tools"];

/// Replacement guidance for [`REMOVED_MCP_TOOLS_FIELDS`] diagnostics.
pub fn removed_mcp_tools_replacement() -> &'static str {
    "use `mcp(server)` or `mcp(server/tool)` entries in `tools:` / `disallowed-tools:` instead"
}

/// Strip retired `mcp-tools` keys from canonical frontmatter during staging lift.
pub(crate) fn strip_removed_mcp_tools_fields(fm: &mut Frontmatter) -> bool {
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
    use crate::compiler::agents::{HarnessKind, parse_agent_content};
    use crate::compiler::skills::parse_skill_content;
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
    fn tools_mcp_entry_grants_whole_server() {
        let policy = agent_policy("---\nname: a\ndescription: d\ntools: [mcp(context7)]\n---\n");
        assert_eq!(
            policy.describe(),
            vec!["deny:*".to_string(), "allow:mcp(context7/*)".to_string()]
        );
    }

    #[test]
    fn skill_tools_mcp_entry_grants_whole_server() {
        let policy =
            skill_policy("---\nname: a\ndescription: d\ntools: [mcp(context7)]\n---\nbody");
        assert_eq!(policy.describe(), vec!["allow:mcp(context7/*)".to_string()]);
    }

    #[test]
    fn append_mcp_server_entries_to_tools_merges_into_existing_list() {
        let mut fm = Frontmatter::parse("---\ntools: [Bash]\n---\n").unwrap();
        assert!(append_mcp_server_entries_to_tools(
            &mut fm,
            &["context7".to_string()]
        ));
        assert_eq!(
            fm.get("tools").map(yaml_str_list),
            Some(vec!["Bash".to_string(), "mcp(context7)".to_string()])
        );
    }

    #[test]
    fn append_mcp_server_entries_to_tools_is_idempotent() {
        let mut fm = Frontmatter::parse("---\ntools: [mcp(context7)]\n---\n").unwrap();
        assert!(!append_mcp_server_entries_to_tools(
            &mut fm,
            &["context7".to_string()]
        ));
    }

    #[test]
    fn append_mcp_server_entries_to_tools_preserves_map_form() {
        let mut fm = Frontmatter::parse("---\ntools:\n  Bash: allow\n  Read: deny\n---\n").unwrap();
        assert!(append_mcp_server_entries_to_tools(
            &mut fm,
            &["context7".to_string(), "github".to_string()]
        ));
        let tools = fm.get("tools").unwrap();
        let Value::Mapping(mapping) = tools else {
            panic!("expected map-form tools, got {tools:?}");
        };
        assert_eq!(
            mapping.get(Value::String("Bash".into())),
            Some(&Value::String("allow".into()))
        );
        assert_eq!(
            mapping.get(Value::String("Read".into())),
            Some(&Value::String("deny".into()))
        );
        assert_eq!(
            mapping.get(Value::String("mcp(context7)".into())),
            Some(&Value::String("allow".into()))
        );
        assert_eq!(
            mapping.get(Value::String("mcp(github)".into())),
            Some(&Value::String("allow".into()))
        );
    }

    #[test]
    fn disallowed_mcp_ref_round_trips_through_policy() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ndisallowed-tools: [mcp(github/delete_repo)]\n---\n",
        );
        assert_eq!(
            policy.describe(),
            vec!["deny:mcp(github/delete_repo)".to_string()]
        );
        assert!(policy.rules[0].is_mcp());
        assert_eq!(
            policy.rules[0].mcp_ref().unwrap().to_canonical(),
            "mcp(github/delete_repo)"
        );
    }

    #[test]
    fn plugin_colon_server_names_preserve_verbatim_in_mcp_refs() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools: [mcp(plugin:context7:context7)]\n---\n",
        );
        let mcp = policy.with_action(ToolAction::Allow).next().unwrap();
        assert_eq!(
            mcp.mcp_ref().unwrap(),
            crate::compiler::mcp_ref::parse_mcp_ref("plugin:context7:context7").unwrap()
        );
    }

    #[test]
    fn list_form_is_an_allowlist() {
        let policy = agent_policy("---\nname: a\ndescription: d\ntools: [read, grep]\n---\n");
        assert_eq!(
            policy.describe(),
            vec!["deny:*", "allow:read", "allow:grep"]
        );
        assert!(policy.is_allowlist());
        assert_eq!(policy.rules[0].origin, Some(RuleOrigin::ListForm));
        assert_eq!(policy.rules[1].origin, None);
    }

    #[test]
    fn map_form_allows_are_grants_not_an_allowlist() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools:\n  read: allow\n  edit: deny\n---\n",
        );
        assert_eq!(policy.describe(), vec!["allow:read", "deny:edit"]);
        assert!(!policy.is_allowlist());
    }

    #[test]
    fn explicit_wildcard_deny_makes_a_map_an_allowlist() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools:\n  '*': deny\n  'bash(rg *)': allow\n---\n",
        );
        assert_eq!(policy.describe(), vec!["deny:*", "allow:bash(rg *)"]);
        assert!(policy.is_allowlist());
        assert_eq!(policy.rules[0].origin, None, "authored, not list-form");
    }

    #[test]
    fn empty_list_names_no_tools_and_is_no_policy() {
        let policy = agent_policy("---\nname: a\ndescription: d\ntools: []\n---\n");
        assert!(policy.rules.is_empty());
        assert!(!policy.is_allowlist());
    }

    #[test]
    fn ask_is_an_action() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools:\n  'bash(git push *)': ask\n  edit: Ask\n---\n",
        );
        assert_eq!(policy.describe(), vec!["ask:bash(git push *)", "ask:edit"]);
    }

    #[test]
    fn legacy_colon_star_patterns_normalize_to_space_star() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools:\n  'bash(git revert:*)': deny\n  'bash(git reset --hard:*)': deny\ndisallowed-tools: ['Bash(git clean:*)']\n---\n",
        );
        assert_eq!(
            policy.describe(),
            vec![
                "deny:bash(git revert *)",
                "deny:bash(git reset --hard *)",
                "deny:bash(git clean *)",
            ]
        );
    }

    #[test]
    fn colon_star_normalizes_only_for_bash() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ndisallowed-tools: ['read(./secrets:*)', 'web_fetch(domain:*)', 'bash(rm:*)']\n---\n",
        );
        assert_eq!(
            policy.describe(),
            vec![
                "deny:read(./secrets:*)",
                "deny:web_fetch(domain:*)",
                "deny:bash(rm *)"
            ]
        );
    }

    #[test]
    fn scalar_tools_split_on_commas_into_the_list_form() {
        let policy =
            agent_policy("---\nname: a\ndescription: d\ntools: Read, Grep, bash(rg a,b)\n---\n");
        assert_eq!(
            policy.describe(),
            vec!["deny:*", "allow:read", "allow:grep", "allow:bash(rg a,b)"]
        );
        let policy =
            agent_policy("---\nname: a\ndescription: d\ndisallowed-tools: Edit, Write\n---\n");
        assert_eq!(policy.describe(), vec!["deny:edit", "deny:write"]);
    }

    #[test]
    fn narrower_capability_wins_over_broader_for_the_natives_they_share() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools:\n  edit: deny\n  write: allow\n  notebook: deny\n---\n",
        );
        let claude = policy.project(HarnessKind::Claude);
        assert_eq!(claude.rules()[0].1.native, ["Edit", "NotebookEdit"]);
        assert_eq!(claude.rules()[1].1.native, ["Write"]);
        assert!(policy.project(HarnessKind::Claude).overlaps().is_empty());
        // Pi names `edit` and `write` as separate tools too.
        assert!(policy.project(HarnessKind::Pi).overlaps().is_empty());
    }

    #[test]
    fn capabilities_a_harness_cannot_tell_apart_stay_in_conflict() {
        // Codex edits files through `apply_patch` alone, so `write` and `edit` share it exactly.
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools:\n  edit: deny\n  write: allow\n---\n",
        );
        assert_eq!(
            policy.project(HarnessKind::Codex).overlaps(),
            ["apply_patch"]
        );
    }

    #[test]
    fn a_scoped_rule_does_not_narrow_an_unscoped_one() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools:\n  bash: allow\n  'bash(git push *)': deny\n---\n",
        );
        let projection = policy.project(HarnessKind::Claude);
        assert_eq!(projection.rules()[0].1.native, ["Bash"]);
        assert_eq!(projection.rules()[1].1.native, ["Bash(git push *)"]);
        assert!(projection.overlaps().is_empty());
    }

    #[test]
    fn skill_list_form_grants_and_never_restricts() {
        let policy = skill_policy("---\nname: a\ndescription: d\ntools: [read]\n---\nbody");
        assert_eq!(policy.describe(), vec!["allow:read"]);
        assert!(!policy.is_allowlist());
    }

    #[test]
    fn mcp_server_entries_appended_to_absent_tools_do_not_create_an_allowlist() {
        // A Claude agent with `mcpServers` and no `tools:` inherits every tool. The
        // synthesized `mcp(server)` grant must not turn that into "MCP only".
        let mut fm = Frontmatter::parse("---\nname: a\n---\n").unwrap();
        assert!(append_mcp_server_entries_to_tools(
            &mut fm,
            &["context7".to_string()]
        ));
        let mut diags = Vec::new();
        let profile = crate::compiler::agents::parse_agent_profile(&fm, &mut diags);
        let policy = profile.effective_tool_policy(&crate::compiler::agents::HarnessKind::Claude);
        assert_eq!(policy.describe(), vec!["allow:mcp(context7/*)"]);
        assert!(!policy.is_allowlist());
    }
}
