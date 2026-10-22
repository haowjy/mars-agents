//! Typed tool policy for agent and skill frontmatter.
//!
//! Canonical Mars profiles express tool gating with `tools:` (list or allow/deny/ask map),
//! `disallowed-tools:`, and whole-server or per-tool MCP grants as `mcp(...)` entries in
//! those lists. [`parse_tool_policy`] reads them once into an [`EffectiveToolPolicy`]: one list
//! of [`ToolRule`]s that the profile stores, a `mars.toml` overlay edits, and
//! [`EffectiveToolPolicy::project`] spells for a harness. Native agent lowering, skill lowering
//! and the launch bundle all read that projection; nothing else turns a rule into native names.
//!
//! [`NON_CANONICAL_TOOL_FIELD_ALIASES`] is the single source for foreign spellings that
//! must not appear in canonical/MarsNative profiles. Staging strips alias keys; the skill
//! parser warns with the canonical replacement label.

use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_yaml::Value;

use crate::compiler::agents::HarnessKind;
use crate::compiler::mcp_ref::{McpProjection, project_mcp_ref};
use crate::compiler::tool_names::{
    Capability, OverlayToolName, ParsedToolName, is_mcp_wire_token, parse_mars_tool_name,
    render_scoped,
};
use crate::compiler::tool_natives::{self, Native};
use crate::config::AgentOverlayTools;
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
/// Rules are kept in profile order, which carries no precedence: a broad rule against a scoped
/// one is the consuming harness's call, and [`EffectiveToolPolicy::project`] settles only
/// capabilities one harness tool cannot tell apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRule {
    pub(crate) capability: Capability,
    /// Scope inside `capability(...)`. `None` for `*` and `mcp`, whose scope is the capability.
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
            capability: Capability::Any,
            pattern: None,
            action: ToolAction::Deny,
            origin: Some(RuleOrigin::ListForm),
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
/// `{capability, pattern, action, origin?}`. `capability` is `*` for "every other tool" and
/// `mcp` for an MCP ref, whose `pattern` is its `server/tool`.
impl Serialize for ToolRule {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("capability", self.capability.name())?;
        map.serialize_entry("pattern", &self.wire_pattern())?;
        map.serialize_entry("action", self.action.as_str())?;
        if let Some(origin) = self.origin {
            map.serialize_entry("origin", origin.as_str())?;
        }
        map.end()
    }
}

/// Whether Mars can name a rule's tools on a harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeStatus {
    /// Mars knows the native name(s).
    Known,
    /// The harness has no such tool, so there is nothing to allow or deny.
    Absent,
    /// Mars cannot name it on this harness, for the reason given: the harness has no switch
    /// for it, Mars has no row for it, or the name is not one Mars recognizes (a typo). The
    /// launch bundle reports the reason in `note` and nowhere else.
    Unknown(&'static str),
}

impl NativeStatus {
    /// The bundle's `native_status`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Known => "known",
            Self::Absent => "absent",
            Self::Unknown(_) => "unknown",
        }
    }

    /// Why Mars could not name the rule, for `unknown`.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::Unknown(note) => Some(note),
            Self::Known | Self::Absent => None,
        }
    }
}

const NO_NATIVE_NAME: &str = "no native name known for this tool on this harness; left out";

/// A rule as one harness spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectedRule {
    /// Native names, one per native tool the capability covers (`edit` → `Edit`, `Write`, …),
    /// with the pattern appended when the rule is scoped.
    pub native: Vec<String>,
    /// `None` for `*`, which names no tool.
    pub status: Option<NativeStatus>,
}

impl ProjectedRule {
    fn known(native: Vec<String>) -> Self {
        Self {
            native,
            status: Some(NativeStatus::Known),
        }
    }

    fn with_status(status: NativeStatus) -> Self {
        Self {
            native: Vec::new(),
            status: Some(status),
        }
    }

    /// Why Mars could not name this rule on the harness, when it could not.
    pub fn gap(&self) -> Option<&'static str> {
        self.status.and_then(NativeStatus::note)
    }
}

/// Project `rule` onto `harness`. The one place a capability becomes native names.
fn project_rule(rule: &ToolRule, harness: HarnessKind) -> ProjectedRule {
    let scoped = |name: &str| render_scoped(name, rule.pattern.as_deref());
    match &rule.capability {
        Capability::Any => ProjectedRule {
            native: Vec::new(),
            status: None,
        },
        Capability::Mcp(mcp_ref) => match project_mcp_ref(mcp_ref, harness) {
            McpProjection::Token(token) => ProjectedRule::known(vec![token]),
            McpProjection::Unsupported(reason) => {
                ProjectedRule::with_status(NativeStatus::Unknown(reason.message()))
            }
        },
        Capability::Unknown(name) if is_mcp_wire_token(name) => {
            ProjectedRule::known(vec![scoped(name)])
        }
        Capability::Unknown(_) => ProjectedRule::with_status(NativeStatus::Unknown(NO_NATIVE_NAME)),
        Capability::Builtin(name) => match tool_natives::lookup(name, harness) {
            Some(Native::Tools(names)) => {
                ProjectedRule::known(names.iter().map(|name| scoped(name)).collect())
            }
            Some(Native::Absent) => ProjectedRule::with_status(NativeStatus::Absent),
            Some(Native::NoSwitch(note)) => ProjectedRule::with_status(NativeStatus::Unknown(note)),
            None => ProjectedRule::with_status(NativeStatus::Unknown(NO_NATIVE_NAME)),
        },
    }
}

/// The tool rules of an agent or skill.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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

    /// Compact `action:capability(pattern)` spellings, one per rule.
    pub fn describe(&self) -> Vec<String> {
        self.rules
            .iter()
            .map(|rule| format!("{}:{}", rule.action.as_str(), rule.render()))
            .collect()
    }

    /// The `action` rules as `capability(pattern)` spellings, without the `*` allowlist marker.
    /// Feeds the deprecated `tools` and `disallowed-tools` keys of `show --json`.
    pub fn rendered(&self, action: ToolAction) -> Vec<String> {
        self.with_action(action)
            .filter(|rule| !rule.is_wildcard())
            .map(ToolRule::render)
            .collect()
    }

    /// Whether there is a `kind` rule with `action`.
    pub(crate) fn has_rule(&self, action: ToolAction, kind: RuleKind) -> bool {
        self.with_action(action).any(|rule| kind.covers(rule))
    }

    /// Whether a `*: deny` limits the agent to the tools its allow rules name.
    pub fn is_allowlist(&self) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.is_wildcard() && rule.action == ToolAction::Deny)
    }

    /// Add `rule` unless the policy already has one for the same capability, pattern and action.
    fn push_unique(&mut self, rule: ToolRule) {
        let seen = self.rules.iter().any(|existing| {
            existing.capability == rule.capability
                && existing.pattern == rule.pattern
                && existing.action == rule.action
        });
        if !seen {
            self.rules.push(rule);
        }
    }

    /// Apply a `mars.toml` `[agents.<name>.tools]` or `[skills.<name>.tools]` overlay. The one
    /// place overlay tool lists meet a policy, for agents and skills alike.
    ///
    /// `allowed` replaces what the policy grants; the existing deny and ask rules stay, because
    /// an overlay that widens the grants must not silently lift a `git push` deny. When
    /// `allowed_is_allowlist` (an agent: a list means "exactly these tools") the policy becomes
    /// an allowlist. `disallowed` replaces the deny rules, except a `*: deny` that makes the
    /// policy an allowlist, which no tool list stands in for.
    pub(crate) fn apply_overlay(
        &mut self,
        overlay: &AgentOverlayTools,
        allowed_is_allowlist: bool,
    ) {
        if !overlay.allowed.is_empty() {
            self.rules.retain(|rule| rule.action != ToolAction::Allow);
            for rule in overlay_rules(&overlay.allowed, ToolAction::Allow) {
                self.push_unique(rule);
            }
            if allowed_is_allowlist && !self.is_allowlist() {
                self.rules.insert(0, ToolRule::list_form_allowlist());
            }
        }
        if !overlay.disallowed.is_empty() {
            self.rules
                .retain(|rule| rule.action != ToolAction::Deny || rule.is_wildcard());
            for rule in overlay_rules(&overlay.disallowed, ToolAction::Deny) {
                self.push_unique(rule);
            }
        }
    }

    /// Write the policy back as a skill's `tools:` / `disallowed-tools:`, replacing what is
    /// there. Grants are a list; a map only where an `ask` needs one. Denies go to
    /// `disallowed-tools:`, where a skill reads them whichever form `tools:` had.
    pub(crate) fn write_skill_frontmatter(&self, fm: &mut Frontmatter) {
        let spell = |rule: &ToolRule| Value::String(rule.render());
        let granted: Vec<&ToolRule> = self
            .rules
            .iter()
            .filter(|rule| rule.action != ToolAction::Deny)
            .collect();
        let tools = if self.with_action(ToolAction::Ask).next().is_some() {
            Value::Mapping(
                granted
                    .iter()
                    .map(|rule| (spell(rule), Value::String(rule.action.as_str().into())))
                    .collect(),
            )
        } else {
            Value::Sequence(granted.iter().map(|rule| spell(rule)).collect())
        };
        let denied: Vec<Value> = self.with_action(ToolAction::Deny).map(spell).collect();

        fm.remove("disallowed_tools");
        let (no_grants, no_denies) = (granted.is_empty(), denied.is_empty());
        for (key, value, empty) in [
            ("tools", tools, no_grants),
            ("disallowed-tools", Value::Sequence(denied), no_denies),
        ] {
            if empty {
                fm.remove(key);
            } else {
                fm.insert(key, value);
            }
        }
    }
}

/// Which rules that name tools a query covers. `*` names none, so no kind covers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuleKind {
    /// Built-in tools: everything but MCP refs.
    Builtin,
    Mcp,
    All,
}

impl RuleKind {
    fn covers(self, rule: &ToolRule) -> bool {
        match self {
            Self::Builtin => !rule.is_mcp() && !rule.is_wildcard(),
            Self::Mcp => rule.is_mcp(),
            Self::All => !rule.is_wildcard(),
        }
    }
}

/// The bundle `note` on a rule whose native tool another rule, of the opposite action, also
/// names (see [`PolicyProjection::overlaps`]).
pub(crate) const OVERLAP_NOTE: &str =
    "another rule allows or denies the same native tool; the harness decides which wins";

/// A policy's rules spelled for one harness, in profile order.
pub(crate) struct PolicyProjection<'a> {
    rules: Vec<(&'a ToolRule, ProjectedRule)>,
}

impl<'a> PolicyProjection<'a> {
    pub fn rules(&self) -> &[(&'a ToolRule, ProjectedRule)] {
        &self.rules
    }

    /// The projected `kind` rules with `action`: built-in tools first, then MCP refs.
    fn select(&self, action: ToolAction, kind: RuleKind) -> Vec<&ProjectedRule> {
        let mut picked: Vec<_> = self
            .rules
            .iter()
            .filter(|(rule, _)| rule.action == action && kind.covers(rule))
            .collect();
        picked.sort_by_key(|(rule, _)| rule.is_mcp());
        picked.into_iter().map(|(_, projected)| projected).collect()
    }

    /// Native names of the `kind` rules with `action`, built-in tools first, each name once.
    pub fn natives(&self, action: ToolAction, kind: RuleKind) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for projected in self.select(action, kind) {
            for native in &projected.native {
                if !names.contains(native) {
                    names.push(native.clone());
                }
            }
        }
        names
    }

    /// Why Mars could not name each `kind` rule with `action`.
    pub fn gaps(&self, action: ToolAction, kind: RuleKind) -> Vec<&'static str> {
        self.select(action, kind)
            .into_iter()
            .filter_map(ProjectedRule::gap)
            .collect()
    }

    /// Native names that an allow rule and a deny rule both still name after
    /// [`resolve_by_specificity`]: two capabilities one harness tool cannot tell apart
    /// (`write` and `edit` on Codex). The harness decides which wins; a deny usually does.
    /// [`OVERLAP_NOTE`] is the bundle's word for it.
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
    let takes_part = |rule: &ToolRule| !rule.is_wildcard() && !rule.is_mcp();
    let narrower: Vec<Option<(ToolAction, Vec<String>)>> = rules
        .iter()
        .map(|(rule, projected)| {
            (takes_part(rule) && !projected.native.is_empty())
                .then(|| (rule.action, projected.native.clone()))
        })
        .collect();

    for (rule, projected) in rules.iter_mut() {
        if !takes_part(rule) {
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

/// Rules for the tool names in a `mars.toml` overlay (`[agents.x.tools]`, `[skills.x.tools]`).
/// The names parsed when the config loaded, so there is nothing left to reject here.
fn overlay_rules(names: &[OverlayToolName], action: ToolAction) -> Vec<ToolRule> {
    names
        .iter()
        .map(|name| ToolRule::from_parsed(name.parsed().clone(), action))
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

type OnInvalid<'a> = &'a mut dyn FnMut(&str, &str, &'static str);

/// Read a profile's `tools:` and `disallowed-tools:` into one policy.
///
/// A list-form `tools:` is an allowlist when `list_is_allowlist` (an agent): Mars adds the
/// `*: deny` the author implied. A skill's list only grants. An empty list names no tools, so it
/// stays "no policy" rather than "deny everything". `on_invalid` gets `(field, value, allowed)`
/// for each entry Mars rejects.
pub(crate) fn parse_tool_policy(
    tools: Option<&Value>,
    disallowed: Option<&Value>,
    list_is_allowlist: bool,
    on_invalid: OnInvalid<'_>,
) -> EffectiveToolPolicy {
    let mut policy = EffectiveToolPolicy::default();
    if let Some(value) = tools {
        for rule in parse_tools_field("tools", value, on_invalid) {
            policy.push_unique(rule);
        }
        let is_list = !matches!(value, Value::Mapping(_));
        if list_is_allowlist && is_list && !policy.rules.is_empty() {
            policy.rules.insert(0, ToolRule::list_form_allowlist());
        }
    }
    if let Some(value) = disallowed {
        for rule in parse_entries("disallowed-tools", value, ToolAction::Deny, on_invalid) {
            policy.push_unique(rule);
        }
    }
    policy
}

fn parse_tool_rule(
    field: &str,
    raw: &str,
    action: ToolAction,
    on_invalid: OnInvalid<'_>,
) -> Option<ToolRule> {
    match parse_mars_tool_name(raw) {
        Ok(parsed) => Some(ToolRule::from_parsed(parsed, action)),
        Err(err) => {
            on_invalid(field, raw, err.allowed());
            None
        }
    }
}

/// Parse a list of tool names into `action` rules.
fn parse_entries(
    field: &str,
    val: &Value,
    action: ToolAction,
    on_invalid: OnInvalid<'_>,
) -> Vec<ToolRule> {
    tool_entries(val)
        .iter()
        .enumerate()
        .filter_map(|(idx, tool)| {
            parse_tool_rule(&format!("{field}[{idx}]"), tool, action, on_invalid)
        })
        .collect()
}

/// Parse `tools:`: a list of tools to allow, or a map of tool to `allow` / `deny` / `ask`.
fn parse_tools_field(field: &str, val: &Value, on_invalid: OnInvalid<'_>) -> Vec<ToolRule> {
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
            rules
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
            Vec::new()
        }
        _ => parse_entries(field, val, ToolAction::Allow, on_invalid),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::agents::{HarnessKind, parse_agent_content};
    use crate::compiler::skills::parse_skill_content;
    use crate::frontmatter::Frontmatter;

    fn agent_policy(yaml: &str) -> EffectiveToolPolicy {
        let mut diags = Vec::new();
        let (profile, _) = parse_agent_content(yaml, &mut diags).unwrap();
        assert!(diags.is_empty(), "agent diags: {diags:?}");
        profile.tools
    }

    fn skill_policy(yaml: &str) -> EffectiveToolPolicy {
        let mut diags = Vec::new();
        let (profile, _) = parse_skill_content(yaml, &mut diags).unwrap();
        assert!(diags.is_empty(), "skill diags: {diags:?}");
        profile.tools
    }

    /// Native names and status of a raw tool entry on a harness, via parse then table.
    fn natives(raw: &str, harness: &str) -> (Vec<String>, &'static str) {
        let kind = crate::compiler::harness_descriptor::descriptor_for_canonical_id(harness)
            .expect("test harness exists")
            .kind;
        let parsed = parse_mars_tool_name(raw).unwrap();
        let projected = project_rule(&ToolRule::from_parsed(parsed, ToolAction::Deny), kind);
        (
            projected.native,
            projected.status.map_or("none", NativeStatus::as_str),
        )
    }

    fn known(names: &[&str]) -> (Vec<String>, &'static str) {
        (names.iter().map(|n| n.to_string()).collect(), "known")
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
        let Capability::Mcp(mcp_ref) = &policy.rules[0].capability else {
            panic!("expected an MCP rule");
        };
        assert_eq!(mcp_ref.to_canonical(), "mcp(github/delete_repo)");
    }

    #[test]
    fn plugin_colon_server_names_preserve_verbatim_in_mcp_refs() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools: [mcp(plugin:context7:context7)]\n---\n",
        );
        let mcp = policy.with_action(ToolAction::Allow).next().unwrap();
        assert_eq!(
            mcp.capability,
            Capability::Mcp(
                crate::compiler::mcp_ref::parse_mcp_ref("plugin:context7:context7").unwrap()
            )
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
        let policy = crate::compiler::agents::parse_agent_profile(&fm, &mut diags).tools;
        assert_eq!(policy.describe(), vec!["allow:mcp(context7/*)"]);
        assert!(!policy.is_allowlist());
    }

    #[test]
    fn aliases_and_native_spellings_reach_the_same_native_tools() {
        assert_eq!(natives("Bash", "codex"), known(&["exec_command"]));
        assert_eq!(natives("shell", "claude"), known(&["Bash"]));
        assert_eq!(natives("exec_command", "claude"), known(&["Bash"]));
        assert_eq!(natives("WebSearch", "opencode"), known(&["websearch"]));
        assert_eq!(
            natives("web", "opencode"),
            known(&["websearch", "webfetch"])
        );
        assert_eq!(
            natives("AskUserQuestion", "claude"),
            known(&["AskUserQuestion"])
        );
        assert_eq!(natives("askuser", "opencode"), known(&["question"]));
        assert_eq!(
            natives("edit", "claude"),
            known(&["Edit", "Write", "NotebookEdit"])
        );
        assert_eq!(natives("todowrite", "claude").0.len(), 7);
    }

    #[test]
    fn a_pattern_scopes_every_native_tool_of_the_capability() {
        assert_eq!(
            natives("bash(git push *)", "codex"),
            known(&["exec_command(git push *)"])
        );
        assert_eq!(
            natives("edit(src/**)", "claude"),
            known(&["Edit(src/**)", "Write(src/**)", "NotebookEdit(src/**)"])
        );
    }

    #[test]
    fn a_tool_the_harness_lacks_is_absent_not_guessed() {
        assert_eq!(natives("notebook", "codex"), (vec![], "absent"));
        assert_eq!(natives("ask_user", "pi"), (vec![], "absent"));
    }

    #[test]
    fn a_thing_the_harness_can_do_without_a_tool_to_switch_is_unknown_not_absent() {
        // Codex fetches pages through `web_search` and reads, globs and greps through its shell;
        // Codex and Pi load skills by reading files.
        for (capability, harness) in [
            ("web_fetch", "codex"),
            ("skill", "codex"),
            ("skill", "pi"),
            ("read", "codex"),
            ("glob", "codex"),
            ("grep", "codex"),
        ] {
            assert_eq!(
                natives(capability, harness),
                (vec![], "unknown"),
                "{harness} {capability}"
            );
        }
    }

    #[test]
    fn a_name_without_a_table_row_is_unknown_with_no_native_name() {
        // Known capability, no Cursor row: no convention fallback fills it in.
        assert_eq!(natives("lsp", "cursor"), (vec![], "unknown"));
        // Not a capability at all, including one that merely looks like the `mcp` capability.
        for harness in ["claude", "codex", "opencode", "pi", "cursor"] {
            assert_eq!(
                natives("my_custom_tool", harness),
                (vec![], "unknown"),
                "{harness}"
            );
        }
        for raw in ["café_tool", "🚀tool"] {
            assert_eq!(natives(raw, "claude"), (vec![], "unknown"));
        }
    }

    #[test]
    fn mcp_wire_tokens_are_verbatim_on_every_harness() {
        for raw in [
            "mcp__github__CreateIssue",
            "MCP__Server__Tool",
            "mcp__github__create_issue",
        ] {
            for harness in ["claude", "codex", "opencode", "pi", "cursor"] {
                assert_eq!(natives(raw, harness), known(&[raw]), "{raw} on {harness}");
            }
        }
    }

    #[test]
    fn the_any_capability_names_no_tool_and_has_no_status() {
        assert_eq!(natives("*", "claude"), (vec![], "none"));
    }

    #[test]
    fn natives_put_builtin_tools_before_mcp_refs_and_name_each_once() {
        let policy = agent_policy(
            "---
name: a
description: d
disallowed-tools: [mcp(github/delete_repo), bash, shell, read]
---
",
        );
        let claude = policy.project(HarnessKind::Claude);
        assert_eq!(
            claude.natives(ToolAction::Deny, RuleKind::All),
            ["Bash", "Read", "mcp__github__delete_repo"]
        );
        assert_eq!(
            claude.natives(ToolAction::Deny, RuleKind::Mcp),
            ["mcp__github__delete_repo"]
        );
        // Codex cannot scope one MCP tool: the rule is a gap with the reason, not a name.
        let codex = policy.project(HarnessKind::Codex);
        assert_eq!(
            codex.natives(ToolAction::Deny, RuleKind::Mcp),
            Vec::<String>::new()
        );
        assert_eq!(
            codex.gaps(ToolAction::Deny, RuleKind::Mcp),
            ["Codex per-tool MCP gating lives in server config, not the tool list"]
        );
    }

    #[test]
    fn malformed_mcp_in_disallowed_tools_is_validation_error() {
        let mut diags = Vec::new();
        let value = Value::Sequence(vec![Value::String("mcp()".into())]);
        let mut push = |field: &str, value: &str, _: &'static str| {
            diags.push((field.to_string(), value.to_string()));
        };
        let policy = parse_tool_policy(None, Some(&value), true, &mut push);
        assert!(policy.rules.is_empty());
        assert_eq!(
            diags,
            [("disallowed-tools[0]".to_string(), "mcp()".to_string())]
        );
    }

    #[test]
    fn a_bare_mcp_tool_is_invalid_not_an_unknown_tool() {
        for raw in ["mcp", "MCP"] {
            let mut diags = Vec::new();
            let value = Value::Sequence(vec![Value::String(raw.into())]);
            let mut push = |field: &str, value: &str, _: &'static str| {
                diags.push((field.to_string(), value.to_string()));
            };
            let policy = parse_tool_policy(Some(&value), None, true, &mut push);
            assert!(policy.rules.is_empty(), "{raw}");
            assert_eq!(diags.len(), 1, "{raw}");
        }
    }

    #[test]
    fn the_overlay_edits_agent_and_skill_policies_alike() {
        let overlay = AgentOverlayTools {
            allowed: vec!["bash".to_string().try_into().unwrap()],
            disallowed: vec!["agent".to_string().try_into().unwrap()],
        };
        let yaml = "tools:
  'bash(git push *)': deny
  'bash(git commit *)': ask
  read: allow
";

        // An agent: `allowed` replaces the grants, keeps deny and ask, and becomes an allowlist.
        let mut agent = agent_policy(&format!("---\nname: a\ndescription: d\n{yaml}---\n"));
        agent.apply_overlay(&overlay, true);
        assert_eq!(
            agent.describe(),
            [
                "deny:*",
                "ask:bash(git commit *)",
                "allow:bash",
                "deny:agent"
            ]
        );

        // A skill: the same edit, but a skill list only grants, so there is no `*: deny`.
        let mut skill = skill_policy(&format!("---\nname: a\ndescription: d\n{yaml}---\nbody"));
        skill.apply_overlay(&overlay, false);
        assert_eq!(
            skill.describe(),
            ["ask:bash(git commit *)", "allow:bash", "deny:agent"]
        );
    }
}
