//! The `permission` map of an OpenCode agent file.
//!
//! Mars resolves a call by the strictest matching rule (deny, then ask, then allow). OpenCode
//! resolves it by the last matching rule. The map is written in an order that makes them agree:
//!
//! 1. `*` first, when it restricts (`deny` or `ask`). A named tool's rule matches later and
//!    wins, so `*` decides only a call no named rule matches, as in Mars.
//! 2. Then one entry per OpenCode action. Rules for different actions never match the same
//!    call, so their order among themselves does not matter.
//! 3. Inside an action, rules are sorted allow, ask, deny (stable). Every rule that matches a
//!    call is in that one list, so the last match is the strictest, however narrow either
//!    pattern is. This holds for any pattern syntax, because the sort looks at actions only.
//!
//! Two rules with the same action and resource collapse to the stricter. Several Mars
//! capabilities can share an OpenCode action (`write` and `edit` are both `edit`); they are
//! grouped by the action, not the capability.
//!
//! The file keeps the v1 `permission` map and not the v2 `permissions` array: OpenCode loads it
//! (translating `bash` to `shell` and keeping key order), the plugin build reads it, and an
//! agent file with a `permissions` array would ignore `name` and the rest of the v1 fields.
//!
//! An allow is written only where it carves an exception: under a restricting `*`, or beside
//! a rule of the same action that is not an allow. Elsewhere it restates OpenCode's default and
//! would override the project's own `opencode.json` rules, so it is left out.
//!
//! OpenCode's `execute` (code mode) runs arbitrary code and stays offered when only the shell
//! is restricted, and an `ask` on it is not raised for code that calls no gated tool (checked
//! on OpenCode 2.2.5), so a shell deny or ask denies `execute`.

use crate::compiler::lossiness::{Lossiness, LossyField, Lost};
use crate::compiler::tool_policy::{PolicyProjection, ToolPolicyRuntime};
use mars_schema::HarnessId;
use mars_schema::tool_policy::{EffectiveToolPolicy, ToolAction};
use serde_yaml::{Mapping, Value};

const STAR: &str = "*";

/// One OpenCode action's rules, ordered for last-match-wins.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ActionRules {
    action: String,
    rules: Vec<(String, ToolAction)>,
}

/// The ordered `permission` entries Mars writes for `tools`, empty when it writes none.
pub(super) fn rules(tools: &EffectiveToolPolicy) -> Vec<ActionRules> {
    let projection = tools.project(HarnessId::OpenCode);
    let star = star_effect(tools);
    let mut groups = action_groups(&projection);

    let shell_restricted = groups.iter().any(|group| {
        group.action == "bash"
            && group
                .rules
                .iter()
                .any(|(_, effect)| *effect != ToolAction::Allow)
    });
    if shell_restricted {
        add_rule(&mut groups, "execute", STAR, ToolAction::Deny);
    }

    let restricting = star.is_some();
    groups.retain(|group| {
        restricting
            || group
                .rules
                .iter()
                .any(|(_, effect)| *effect != ToolAction::Allow)
    });
    for group in &mut groups {
        group.rules.sort_by_key(|(_, effect)| effect.strictness());
    }
    if let Some(effect) = star {
        groups.insert(
            0,
            ActionRules {
                action: STAR.into(),
                rules: vec![(STAR.into(), effect)],
            },
        );
    }
    groups
}

/// The `*` rule's effect when it restricts: `allow` is OpenCode's default and is not written.
fn star_effect(tools: &EffectiveToolPolicy) -> Option<ToolAction> {
    tools
        .rules
        .iter()
        .filter(|rule| rule.is_wildcard() && rule.action != ToolAction::Allow)
        .map(|rule| rule.action)
        .max_by_key(|action| action.strictness())
}

fn action_groups(projection: &PolicyProjection<'_>) -> Vec<ActionRules> {
    let mut groups: Vec<ActionRules> = Vec::new();
    // Built-in tools before MCP refs, as the flat deny list always was.
    let mut rules: Vec<_> = projection
        .rules()
        .iter()
        .filter(|(rule, _)| !rule.is_wildcard())
        .collect();
    rules.sort_by_key(|(rule, _)| rule.is_mcp());
    for (rule, projected) in rules {
        for native in projected.native() {
            let (action, resource) = split_native(&native);
            add_rule(&mut groups, action, resource, rule.action);
        }
    }
    groups
}

/// `bash(git *)` is action `bash`, resource `git *`; a bare name's resource is `*`.
fn split_native(native: &str) -> (&str, &str) {
    match native.split_once('(') {
        Some((action, rest)) => (action, rest.strip_suffix(')').unwrap_or(rest)),
        None => (native, STAR),
    }
}

fn add_rule(groups: &mut Vec<ActionRules>, action: &str, resource: &str, effect: ToolAction) {
    let at = match groups.iter().position(|group| group.action == action) {
        Some(at) => at,
        None => {
            groups.push(ActionRules {
                action: action.into(),
                rules: Vec::new(),
            });
            groups.len() - 1
        }
    };
    let rules = &mut groups[at].rules;
    match rules.iter_mut().find(|(existing, _)| existing == resource) {
        Some((_, existing)) => {
            if effect.strictness() > existing.strictness() {
                *existing = effect;
            }
        }
        None => rules.push((resource.into(), effect)),
    }
}

/// `groups` over the author's `harness-overrides.opencode.permission`. Mars's entry for an
/// action replaces the author's for it; the rest of their map is kept. `*` stays first.
pub(super) fn permission_map(
    groups: &[ActionRules],
    author: Option<serde_json::Value>,
    target: &str,
    lossy: &mut Vec<LossyField>,
) -> Mapping {
    let lost = |field: String, classification| LossyField {
        field,
        target: target.into(),
        classification,
    };
    let mut author = match author.map(serde_yaml::to_value) {
        Some(Ok(Value::Mapping(map))) => map,
        Some(_) => {
            // A scalar throws away the author's whole permission policy.
            lossy.push(lost(
                "harness-overrides.opencode.permission".into(),
                Lossiness::Dropped(Lost::Policy),
            ));
            Mapping::new()
        }
        None => Mapping::new(),
    };
    let mut map = Mapping::new();
    let mars_star = groups.iter().find(|group| group.action == STAR);
    if mars_star.is_none()
        && let Some(value) = author.remove(STAR)
    {
        map.insert(Value::String(STAR.into()), value);
    }
    if let Some(group) = mars_star {
        map.insert(Value::String(STAR.into()), group.value());
    }
    for (key, value) in author {
        if key.as_str() != Some(STAR) {
            map.insert(key, value);
        }
    }
    for group in groups.iter().filter(|group| group.action != STAR) {
        if let Some(existing) = map.insert(Value::String(group.action.clone()), group.value())
            && existing != group.value()
        {
            lossy.push(lost(
                format!("harness-overrides.opencode.permission.{}", group.action),
                Lossiness::Quiet {
                    why: "Mars's rules for this tool win",
                },
            ));
        }
    }
    map
}

impl ActionRules {
    /// `deny` for a lone unscoped rule, else the `pattern: effect` map in order.
    fn value(&self) -> Value {
        match self.rules.as_slice() {
            [(resource, effect)] if resource == STAR => Value::String(effect.as_str().into()),
            rules => Value::Mapping(
                rules
                    .iter()
                    .map(|(resource, effect)| {
                        (
                            Value::String(resource.clone()),
                            Value::String(effect.as_str().into()),
                        )
                    })
                    .collect(),
            ),
        }
    }
}
