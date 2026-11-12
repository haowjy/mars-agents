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
//! An allow is written only under a restricting `*`, where it carves an exception to it.
//! Elsewhere it restates OpenCode's default, and because the last match wins it would also
//! land after the project's `opencode.json` rules and OpenCode's built-in ones and override
//! them. Under a restricting `*` that cannot be avoided, and it is a loss: an allowlist
//! agent's allows beat the project's deny for the same tool, and `read: allow` beats OpenCode's
//! built-in `read *.env: ask`.
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

    if star.is_none() {
        for group in &mut groups {
            group
                .rules
                .retain(|(_, effect)| *effect != ToolAction::Allow);
        }
        groups.retain(|group| !group.rules.is_empty());
    }
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

/// Whether the rules allow reading every path under a restricting `*`: such an allow lands
/// after OpenCode's built-in `read *.env: ask`, so `.env` files are read without asking.
pub(super) fn overrides_env_ask(groups: &[ActionRules]) -> bool {
    groups.iter().any(|group| group.action == STAR)
        && groups.iter().any(|group| {
            group.action == "read"
                && group.rules.iter().any(|(resource, effect)| {
                    *effect == ToolAction::Allow && matches!(resource.as_str(), "*" | "**")
                })
        })
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
    merge_rule(&mut groups[at].rules, resource, effect);
}

/// Add a rule; a resource already there keeps the stricter effect.
fn merge_rule(rules: &mut Vec<(String, ToolAction)>, resource: &str, effect: ToolAction) {
    match rules.iter_mut().find(|(existing, _)| existing == resource) {
        Some((_, existing)) => {
            if effect.strictness() > existing.strictness() {
                *existing = effect;
            }
        }
        None => rules.push((resource.into(), effect)),
    }
}

/// `groups` over the author's `harness-overrides.opencode.permission`. Where both set an
/// action, the two lists merge and the stricter rule wins for a resource they share, and the
/// action's list is sorted again; the same holds for `*`. The rest of the author's map is kept.
/// `*` stays first. An author rule the merge changes or reorders is reported by default: it no
/// longer does what it said.
pub(super) fn permission_map(
    groups: &[ActionRules],
    author: Option<serde_json::Value>,
    target: &str,
    lossy: &mut Vec<LossyField>,
) -> Mapping {
    let lost = |field: String| LossyField {
        field: format!("harness-overrides.opencode.permission{field}"),
        target: target.into(),
        classification: Lossiness::Dropped(Lost::Policy),
    };
    let mut author = match author.map(serde_yaml::to_value) {
        Some(Ok(Value::Mapping(map))) => map,
        Some(_) => {
            // A scalar throws away the author's whole permission policy.
            lossy.push(lost(String::new()));
            Mapping::new()
        }
        None => Mapping::new(),
    };
    let mut map = Mapping::new();
    let author_star = author.remove(STAR);
    match (
        groups.iter().find(|group| group.action == STAR),
        author_star,
    ) {
        (Some(mars), Some(written)) => {
            let merged = merge(mars, &written);
            if merged.rules != read_rules(&written).unwrap_or_default() {
                lossy.push(lost(format!(".{STAR}")));
            }
            map.insert(Value::String(STAR.into()), merged.value());
        }
        (Some(mars), None) => {
            map.insert(Value::String(STAR.into()), mars.value());
        }
        (None, Some(written)) => {
            map.insert(Value::String(STAR.into()), written);
        }
        (None, None) => {}
    }
    for (key, written) in author {
        let Some(mars) = key
            .as_str()
            .and_then(|key| groups.iter().find(|group| group.action == key))
        else {
            map.insert(key, written);
            continue;
        };
        let merged = merge(mars, &written);
        if !keeps_author(&merged.rules, read_rules(&written).as_deref()) {
            lossy.push(lost(format!(".{}", mars.action)));
        }
        map.insert(key, merged.value());
    }
    for group in groups.iter().filter(|group| group.action != STAR) {
        map.entry(Value::String(group.action.clone()))
            .or_insert_with(|| group.value());
    }
    map
}

/// An author's action value as rules: `deny` is `*: deny`, a map is `pattern: action`.
fn read_rules(written: &Value) -> Option<Vec<(String, ToolAction)>> {
    match written {
        Value::String(action) => Some(vec![(STAR.into(), ToolAction::parse(action)?)]),
        Value::Mapping(map) => map
            .iter()
            .map(|(resource, action)| {
                Some((
                    resource.as_str()?.to_owned(),
                    ToolAction::parse(action.as_str()?)?,
                ))
            })
            .collect(),
        _ => None,
    }
}

/// Mars's rules with the author's, the stricter for a resource both name, sorted for
/// last-match-wins. An author value Mars cannot read leaves Mars's alone.
fn merge(mars: &ActionRules, written: &Value) -> ActionRules {
    let mut rules = mars.rules.clone();
    for (resource, effect) in read_rules(written).unwrap_or_default() {
        merge_rule(&mut rules, &resource, effect);
    }
    rules.sort_by_key(|(_, effect)| effect.strictness());
    ActionRules {
        action: mars.action.clone(),
        rules,
    }
}

/// Whether the author's rules all stand in `merged`, in their own order.
fn keeps_author(merged: &[(String, ToolAction)], author: Option<&[(String, ToolAction)]>) -> bool {
    let Some(author) = author else { return false };
    let standing: Vec<_> = merged
        .iter()
        .filter(|(resource, _)| author.iter().any(|(named, _)| named == resource))
        .collect();
    standing.len() == author.len()
        && standing
            .iter()
            .zip(author)
            .all(|(held, kept)| *held == kept)
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
