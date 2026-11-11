//! Typed tool policy for agent and skill frontmatter.
//!
//! Canonical Mars profiles express tool gating with a `tools:` action map (`rule: allow`,
//! `deny` or `ask`), whole-server or per-tool MCP grants being `mcp(...)` keys in it.
//! `mars_schema::agent::read` reads them once into an [`EffectiveToolPolicy`]: one list
//! of [`ToolRule`]s that the profile stores, a `mars.toml` overlay edits, and
//! [`EffectiveToolPolicy::project`] spells for a harness. Native agent lowering, skill lowering
//! and the launch bundle all read that projection; nothing else turns a rule into native
//! permission names or unscoped harness tool names.

use crate::compiler::effective_tools::covered;
use crate::compiler::mcp_ref::{McpProjection, project_mcp_ref};
use mars_schema::HarnessId;
use mars_schema::tool_names::{Capability, is_mcp_wire_token, render_scoped};
use mars_schema::tool_natives::{self, Native};
use mars_schema::tool_policy::{EffectiveToolPolicy, ToolAction, ToolRule};

/// Whether Mars can name a rule's tools on a harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeStatus {
    /// Mars knows the native name(s). A stricter rule on the same native name can leave this
    /// rule no surviving name.
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

/// The stricter rule that wins a covered rule's calls, as its author wrote it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CoveredBy {
    pub action: ToolAction,
    pub key: String,
}

impl CoveredBy {
    fn new(rule: &ToolRule) -> Self {
        Self {
            action: rule.action,
            key: rule.render(),
        }
    }
}

impl std::fmt::Display for CoveredBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.action.as_str(), self.key)
    }
}

/// One permission spelling and the unscoped harness tool names the canonical rule concerns.
///
/// Keeping the relationship intact lets conflict resolution remove availability tools with
/// the permission they belong to. Claude scoped rules are the non-trivial case: one
/// `Edit(path)` permission can concern `Edit`, `Write` and `NotebookEdit`.
///
/// A binding is never removed. `beaten_by` marks one a stricter rule wins, so it is still
/// something the rule makes available but no longer something it tells the permission engine.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NativeBinding {
    native: String,
    native_tools: Vec<String>,
    /// The stricter rule (`deny bash`) that wins this name.
    beaten_by: Option<CoveredBy>,
}

impl NativeBinding {
    fn direct(name: &str, pattern: Option<&str>) -> Self {
        Self {
            native: render_scoped(name, pattern),
            native_tools: vec![name.to_string()],
            beaten_by: None,
        }
    }

    fn scoped(name: &str, pattern: &str, native_tools: &[&str]) -> Self {
        Self {
            native: render_scoped(name, Some(pattern)),
            native_tools: native_tools
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            beaten_by: None,
        }
    }
}

/// A rule as one harness spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectedRule {
    /// Every name the rule makes available. The ones not `beaten_by` are what it tells the
    /// permission engine ([`Self::native`]); a covered rule tells it nothing, so a reader that
    /// ignores `covered_by` cannot let through a call the covering rule refuses.
    bindings: Vec<NativeBinding>,
    /// `None` for `*`, which names no tool.
    pub status: Option<NativeStatus>,
    /// A harness-specific consequence of this projection that callers must surface even when
    /// the native name is known.
    note: Option<&'static str>,
    /// The stricter rule (`deny bash`) that covers this one, so that it never applies: another
    /// rule wins every call it would match, or every name it has on this harness.
    pub covered_by: Option<CoveredBy>,
}

impl ProjectedRule {
    fn known(bindings: Vec<NativeBinding>, note: Option<&'static str>) -> Self {
        Self {
            bindings,
            status: Some(NativeStatus::Known),
            note,
            covered_by: None,
        }
    }

    fn with_status(status: NativeStatus) -> Self {
        Self {
            bindings: Vec::new(),
            status: Some(status),
            note: None,
            covered_by: None,
        }
    }

    fn winning(&self) -> impl Iterator<Item = &NativeBinding> {
        self.bindings
            .iter()
            .filter(|binding| binding.beaten_by.is_none())
    }

    /// Permission-rule spellings after conflict resolution.
    pub fn native(&self) -> Vec<String> {
        self.native_names().cloned().collect()
    }

    /// Unscoped harness tool names concerned by the surviving permission rules, each once.
    pub fn native_tools(&self) -> Vec<String> {
        let mut tools = Vec::new();
        for binding in self.winning() {
            for tool in &binding.native_tools {
                if !tools.contains(tool) {
                    tools.push(tool.clone());
                }
            }
        }
        tools
    }

    fn native_names(&self) -> impl Iterator<Item = &String> {
        self.winning().map(|binding| &binding.native)
    }

    /// Every name the rule makes available, whether or not a stricter rule wins it.
    fn available_names(&self) -> impl Iterator<Item = &String> {
        self.bindings.iter().map(|binding| &binding.native)
    }

    fn beat(&mut self, by: CoveredBy) {
        for binding in &mut self.bindings {
            binding.beaten_by.get_or_insert_with(|| by.clone());
        }
        self.covered_by = Some(by);
    }

    /// Why Mars could not name this rule on the harness, when it could not.
    pub fn gap(&self) -> Option<&'static str> {
        self.status.and_then(NativeStatus::note)
    }

    /// A known projection's semantic note, or an unknown projection's gap reason.
    pub fn note(&self) -> Option<&'static str> {
        self.note.or_else(|| self.gap())
    }
}

/// Project `rule` onto `harness`. The one place a capability becomes native permission names
/// and unscoped harness tool names.
fn project_rule(rule: &ToolRule, harness: HarnessId) -> ProjectedRule {
    match &rule.capability {
        Capability::Any => ProjectedRule {
            bindings: Vec::new(),
            status: None,
            note: None,
            covered_by: None,
        },
        Capability::Mcp(mcp_ref) => match project_mcp_ref(mcp_ref, harness) {
            McpProjection::Token(token) => {
                ProjectedRule::known(vec![NativeBinding::direct(&token, None)], None)
            }
            McpProjection::Unsupported(reason) => {
                ProjectedRule::with_status(NativeStatus::Unknown(reason.message()))
            }
        },
        Capability::Unknown(name) if is_mcp_wire_token(name) => ProjectedRule::known(
            vec![NativeBinding::direct(name, rule.pattern.as_deref())],
            None,
        ),
        Capability::Unknown(_) => ProjectedRule::with_status(NativeStatus::Unknown(NO_NATIVE_NAME)),
        Capability::Builtin(name) => {
            project_builtin_capability(name, rule.pattern.as_deref(), harness)
        }
    }
}

/// Project one canonical builtin capability through the same table and status semantics as a
/// profile rule. Launch-bundle platform rules use this without inventing a second native-name
/// projection path.
pub(crate) fn project_platform_capability(
    capability: &'static str,
    harness: HarnessId,
) -> ProjectedRule {
    project_builtin_capability(capability, None, harness)
}

fn project_builtin_capability(
    capability: &str,
    pattern: Option<&str>,
    harness: HarnessId,
) -> ProjectedRule {
    match tool_natives::lookup(capability, harness) {
        Some(Native::Tools(names)) => ProjectedRule::known(
            names
                .iter()
                .map(|name| NativeBinding::direct(name, pattern))
                .collect(),
            None,
        ),
        Some(
            native @ Native::ScopedTools {
                unscoped,
                scoped: scoped_names,
                note,
            },
        ) => {
            let (bindings, note) = match pattern {
                Some(pattern) => (
                    scoped_names
                        .iter()
                        .map(|name| NativeBinding::scoped(name, pattern, native.native_tools()))
                        .collect(),
                    Some(note),
                ),
                None => (
                    unscoped
                        .iter()
                        .map(|name| NativeBinding::direct(name, None))
                        .collect(),
                    None,
                ),
            };
            ProjectedRule::known(bindings, note)
        }
        Some(Native::Absent) => ProjectedRule::with_status(NativeStatus::Absent),
        Some(Native::NoSwitch(note)) => ProjectedRule::with_status(NativeStatus::Unknown(note)),
        None => ProjectedRule::with_status(NativeStatus::Unknown(NO_NATIVE_NAME)),
    }
}

pub(crate) trait ToolPolicyRuntime {
    fn project(&self, harness: HarnessId) -> PolicyProjection<'_>;
}

impl ToolPolicyRuntime for EffectiveToolPolicy {
    fn project(&self, harness: HarnessId) -> PolicyProjection<'_> {
        let mut rules: Vec<(&ToolRule, ProjectedRule)> = self
            .rules
            .iter()
            .map(|rule| (rule, project_rule(rule, harness)))
            .collect();
        keep_strictest_per_native(&mut rules);
        for (index, by) in covered(self) {
            rules[index].1.beat(CoveredBy::new(by));
        }
        PolicyProjection { rules }
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

/// The `note` on a rule that lost a native name to a stricter rule on the same name.
const STRICTER_RULE_NOTE: &str = "a stricter rule controls the same native tool and wins (deny, then ask, then allow); this rule is left out";

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
            for native in projected.native_names() {
                if !names.contains(native) {
                    names.push(native.clone());
                }
            }
        }
        names
    }

    /// Names of every allow and ask rule, taken before coverage and conflict resolution cleared
    /// any: the tools an allowlist must keep available. A deny that beats one of them is
    /// written beside the list, never by emptying it.
    pub fn available(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut rules: Vec<_> = self
            .rules
            .iter()
            .filter(|(rule, _)| rule.action != ToolAction::Deny && !rule.is_wildcard())
            .collect();
        rules.sort_by_key(|(rule, _)| rule.is_mcp());
        for (_, projected) in rules {
            for name in projected.available_names() {
                if !names.contains(name) {
                    names.push(name.clone());
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
}

/// Of the rules that name one native permission, the strictest action keeps it (deny, then ask,
/// then allow), as Claude's own lists do. `edit: deny` beside `write: allow` leaves the allow
/// no `Write` on Claude and no `write` on Pi; capabilities a harness cannot tell apart
/// (`write` and `edit` on Codex) resolve the same way. A rule that loses a name gets a note.
fn keep_strictest_per_native(rules: &mut [(&ToolRule, ProjectedRule)]) {
    let named: Vec<(&ToolRule, Vec<String>)> = rules
        .iter()
        .map(|(rule, projected)| (*rule, projected.native()))
        .collect();
    for (rule, projected) in rules.iter_mut() {
        let mut beaten = 0;
        for binding in &mut projected.bindings {
            let by = named.iter().find(|(stricter, names)| {
                stricter.action.strictness() > rule.action.strictness()
                    && names.contains(&binding.native)
            });
            if let Some((stricter, _)) = by {
                binding.beaten_by = Some(CoveredBy::new(stricter));
                beaten += 1;
            }
        }
        if beaten > 0 {
            projected.note = Some(STRICTER_RULE_NOTE);
        }
        if beaten > 0 && beaten == projected.bindings.len() {
            projected.covered_by = projected.bindings[0].beaten_by.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mars_schema::HarnessId;
    use mars_schema::frontmatter::Frontmatter;
    use mars_schema::tool_names::parse_mars_tool_name;
    use mars_schema::tool_policy::append_mcp_server_entries_to_tools;
    use serde_yaml::Value;

    fn agent_policy(yaml: &str) -> EffectiveToolPolicy {
        let reading = mars_schema::agent::read(yaml);
        assert!(
            reading.diagnostics.is_empty(),
            "agent diags: {:?}",
            reading.diagnostics
        );
        reading.fields.unwrap().tools
    }

    /// Native names and status of a raw tool entry on a harness, via parse then table.
    fn natives(raw: &str, harness: &str) -> (Vec<String>, &'static str) {
        let kind = crate::compiler::harness_descriptor::descriptor_for_canonical_id(harness)
            .expect("test harness exists")
            .kind;
        let parsed = parse_mars_tool_name(raw).unwrap();
        let projected = project_rule(&ToolRule::from_parsed(parsed, ToolAction::Deny), kind);
        (
            projected.native(),
            projected.status.map_or("none", NativeStatus::as_str),
        )
    }

    fn known(names: &[&str]) -> (Vec<String>, &'static str) {
        (names.iter().map(|n| n.to_string()).collect(), "known")
    }

    #[test]
    fn covered_allows_stay_available() {
        // `deny` covers the allow, so the allow names no permission; the tool it opens stays
        // on Claude's `tools:` list, and the deny is what takes it back.
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools:\n  '*': deny\n  write: allow\n  edit: deny\n  'bash(git status *)': allow\n  bash: ask\n---\n",
        );
        let projection = policy.project(HarnessId::Claude);
        assert_eq!(
            projection.natives(ToolAction::Allow, RuleKind::All),
            Vec::<String>::new()
        );
        assert_eq!(
            projection.available(),
            ["Write", "Bash(git status *)", "Bash"]
        );
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
    }

    #[test]
    fn the_strictest_action_keeps_each_native_name_on_every_harness() {
        // (rules, harness, the allow rule's surviving names, the deny rule's)
        type Row = (
            &'static str,
            &'static str,
            HarnessId,
            &'static [&'static str],
            &'static [&'static str],
        );
        let rows: &[Row] = &[
            (
                "edit",
                "write",
                HarnessId::Claude,
                &[],
                &["Edit", "Write", "NotebookEdit"],
            ),
            (
                "edit",
                "notebook",
                HarnessId::Claude,
                &[],
                &["Edit", "Write", "NotebookEdit"],
            ),
            ("edit", "write", HarnessId::Pi, &[], &["edit", "write"]),
            ("edit", "write", HarnessId::Codex, &[], &["apply_patch"]),
            ("edit", "write", HarnessId::OpenCode, &[], &["edit"]),
            (
                "web",
                "web_fetch",
                HarnessId::Claude,
                &[],
                &["WebSearch", "WebFetch"],
            ),
            (
                "web",
                "web_search",
                HarnessId::Claude,
                &[],
                &["WebSearch", "WebFetch"],
            ),
        ];
        for (deny, allow, harness, allowed, denied) in rows {
            let policy = agent_policy(&format!(
                "---\nname: a\ndescription: d\ntools:\n  {deny}: deny\n  {allow}: allow\n---\n"
            ));
            let projection = policy.project(*harness);
            assert_eq!(
                projection.rules()[1].1.native(),
                *allowed,
                "{deny}/{allow} {harness:?}"
            );
            assert!(projection.rules()[1].1.note().is_some());
            assert_eq!(
                projection.rules()[0].1.native(),
                *denied,
                "{deny}/{allow} {harness:?}"
            );
        }
    }

    #[test]
    fn a_scoped_rule_does_not_narrow_an_unscoped_one() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools:\n  bash: allow\n  'bash(git push *)': deny\n---\n",
        );
        let projection = policy.project(HarnessId::Claude);
        assert_eq!(projection.rules()[0].1.native(), ["Bash"]);
        assert_eq!(projection.rules()[1].1.native(), ["Bash(git push *)"]);
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
        let policy = mars_schema::agent::read(&fm.render()).fields.unwrap().tools;
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
    fn claude_scoped_file_rules_separate_permissions_from_callable_tools() {
        assert_eq!(
            natives("bash(git push *)", "codex"),
            known(&["exec_command(git push *)"])
        );
        for (rule, native, native_tools) in [
            ("read(src/**)", "Read(src/**)", &["Read"][..]),
            ("write(src/**)", "Edit(src/**)", &["Write"][..]),
            (
                "edit(src/**)",
                "Edit(src/**)",
                &["Edit", "Write", "NotebookEdit"][..],
            ),
            ("notebook(src/**)", "Edit(src/**)", &["NotebookEdit"][..]),
            ("glob(src/**)", "Read(src/**)", &["Glob"][..]),
            ("grep(src/**)", "Read(src/**)", &["Grep"][..]),
            (
                "web_fetch(domain:example.com)",
                "WebFetch(domain:example.com)",
                &["WebFetch"][..],
            ),
        ] {
            assert_eq!(natives(rule, "claude"), known(&[native]), "{rule}");
            let parsed = parse_mars_tool_name(rule).unwrap();
            let projected = project_rule(
                &ToolRule::from_parsed(parsed, ToolAction::Deny),
                HarnessId::Claude,
            );
            assert_eq!(projected.native_tools(), native_tools, "{rule}");
        }

        let write = project_rule(
            &ToolRule::from_parsed(
                parse_mars_tool_name("write(src/**)").unwrap(),
                ToolAction::Allow,
            ),
            HarnessId::Claude,
        );
        assert!(
            write
                .note()
                .is_some_and(|note| note.contains("deny also refuses other built-in file edits")),
            "{:?}",
            write.note()
        );
    }

    #[test]
    fn claude_strictest_action_wins_when_scoped_capabilities_share_a_permission() {
        let policy = agent_policy(
            "---\nname: a\ndescription: d\ntools:\n  'write(src/**)': allow\n  'edit(src/**)': ask\n  'notebook(src/**)': deny\n---\n",
        );
        let projection = policy.project(HarnessId::Claude);
        assert!(projection.rules()[0].1.native().is_empty(), "allow loses");
        assert!(
            projection.rules()[0].1.native_tools().is_empty(),
            "shadowed rules name no availability tools"
        );
        assert!(projection.rules()[1].1.native().is_empty(), "ask loses");
        assert!(projection.rules()[1].1.native_tools().is_empty());
        assert_eq!(projection.rules()[2].1.native(), ["Edit(src/**)"]);
        assert_eq!(projection.rules()[2].1.native_tools(), ["NotebookEdit"]);
        assert_eq!(
            projection.natives(ToolAction::Deny, RuleKind::Builtin),
            ["Edit(src/**)"]
        );
        assert!(
            projection.rules()[0]
                .1
                .note()
                .is_some_and(|note| note.contains("deny, then ask, then allow"))
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
                let kind =
                    crate::compiler::harness_descriptor::descriptor_for_canonical_id(harness)
                        .expect("test harness exists")
                        .kind;
                let projected = project_rule(
                    &ToolRule::from_parsed(parse_mars_tool_name(raw).unwrap(), ToolAction::Deny),
                    kind,
                );
                assert_eq!(projected.native_tools(), [raw], "{raw} on {harness}");
            }
        }
    }

    #[test]
    fn the_any_capability_names_no_tool_and_has_no_status() {
        assert_eq!(natives("*", "claude"), (vec![], "none"));
    }
}
