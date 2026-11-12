//! The `permission` map of an OpenCode agent file.
//!
//! Three names:
//!
//! - **policy**: the agent's tool rules as `tools.project(OpenCode)` spells them, with the
//!   `execute: deny` rule added when the shell is restricted. Mars semantics: the strictest named
//!   rule that matches decides, and `*` (`s`) decides a call no named rule matches.
//! - **override**: the author's `harness-overrides.opencode.permission`. It is written in
//!   OpenCode's language, so its meaning is OpenCode's: the last matching entry, in the author's
//!   order, which is why it is read from the order-keeping value.
//! - **outside rules**: every rule OpenCode composes that Mars does not write: its built-ins
//!   (`* allow`, `read *.env: ask`), `opencode.json` (top-level and `agent.<name>`), plugins and
//!   a launcher's config.
//!
//! # The guarantee
//!
//! Read on its own by OpenCode's last-match rule, the map gives every call an action at least as
//! strict as the policy gives it and at least as strict as the override gives it, exactly the
//! policy's action when there is no override, and no rule at all for a call both leave at the
//! default. It stops at the map: OpenCode appends an agent file's rules after the built-ins and
//! `opencode.json`, so for a call the map has a rule for, the outside rules before it do not
//! apply. That is OpenCode's agent precedence, the same for a hand-written agent file. A
//! restriction every agent must keep belongs in `[settings.tools]`, which is part of the policy.
//! The test module checks the guarantee with a last-match evaluator.
//!
//! # How the map is built
//!
//! 1. Both sources become rules `(key, resource, effect)`. Keys are compared by the name OpenCode
//!    matches (`opencode_v2_action`: `bash` is `shell`, `write` is `edit`), because OpenCode
//!    renames v1 keys on load and glob-matches action keys against the new name. An override
//!    entry Mars cannot read, or that sits before the author's `*` (which OpenCode resolves
//!    after it), is not written, so the map holds only rules the guarantee can reason about.
//! 2. Rules from one source must not answer weaker than the other source does for the calls they
//!    match. The other source's answer is its resource-`*` rule in the same key when it has one
//!    (that rule is written and sets the floor there), otherwise its `*`. A weaker `ask` becomes
//!    `deny`. A weaker `allow` becomes the bound, except in a sorted key when the map's `*`
//!    already equals the bound: it is left out and the call falls to `*`. The map's `*` is the
//!    stricter of the two.
//! 3. Order: `*` first. Then keys in three bands: keys whose rules are all `allow`, every other
//!    key, keys whose rules are all `deny`; inside a band the policy's keys in policy order, then
//!    keys only the override names in the author's order. A key the policy names is sorted allow,
//!    ask, deny; a key only the override names keeps the author's order, so
//!    `bash: {"*": ask, "git *": allow}` keeps meaning what it says. The last match is then the
//!    strictest across keys as well as inside one.
//! 4. Action keys are globs, so two keys in the middle band can match the same call
//!    (`srv_*` and `srv_tool`; an author's `sh*` and `bash`). Unless every rule in both has one
//!    effect, each glob key of the pair is written as `deny`, which moves it to the last band.
//!
//! The map can be stricter than both sources in three places: a key both sources name (the sort
//! overrides the author's order), a raise in step 2, and a denied glob key. Each is reported.
//!
//! The file keeps the v1 `permission` map and not the v2 `permissions` array: OpenCode loads it
//! (translating `bash` to `shell` and keeping key order), the plugin build reads it, and an
//! agent file with a `permissions` array would ignore `name` and the rest of the v1 fields.
//!
//! An allow is written only under a restricting `*`, where it carves an exception to it;
//! elsewhere it restates OpenCode's default and could only override outside rules.
//!
//! OpenCode's `execute` (code mode) runs arbitrary code and stays offered when only the shell
//! is restricted, and an `ask` on it is not raised for code that calls no gated tool (checked
//! on OpenCode 2.2.5), so a shell deny or ask denies `execute`.

use crate::compiler::lossiness::{Lossiness, LossyField, Lost};
use crate::compiler::tool_policy::{PolicyProjection, ToolPolicyRuntime};
use mars_schema::HarnessId;
use mars_schema::agent::AgentProfile;
use mars_schema::ordered::OrderedValue;
use mars_schema::tool_natives::opencode_v2_action;
use mars_schema::tool_policy::{EffectiveToolPolicy, ToolAction};
use serde_yaml::{Mapping, Value};

const STAR: &str = "*";

/// Project key of the once-per-sync line about rules that are not denies.
pub(crate) const OUTSIDE_RULES_KEY: &str = "opencode.permission";

/// The line, for a project whose `opencode.json` sets `permission`.
pub(super) const OUTSIDE_RULES_NOTE: &str = "OpenCode applies an agent file's rules after opencode.json and its built-in rules (`read *.env: ask`), so an agent that carries allow or ask rules decides the tools it names itself, and every tool when it has a `*` rule; the project's `permission` does not apply to them. Put a restriction every agent must keep in [settings.tools]";

/// Whether the project's `opencode.json` (or `.jsonc`, here or under `.opencode/`) has a
/// `permission` key. A text search, not a parse: JSONC has comments, and the answer only
/// decides whether the note above is worth a line.
pub(crate) fn project_sets_permission(project_root: &std::path::Path) -> bool {
    [
        "opencode.json",
        "opencode.jsonc",
        ".opencode/opencode.json",
        ".opencode/opencode.jsonc",
    ]
    .iter()
    .filter_map(|name| std::fs::read_to_string(project_root.join(name)).ok())
    .any(|text| text.contains("\"permission\""))
}

/// The policy: projected rules in profile order, `*` apart. Allows are kept; whether a map
/// writes them depends on `star`.
pub(super) struct PolicyRules {
    pub star: Option<ToolAction>,
    pub keys: Vec<(String, Vec<(String, ToolAction)>)>,
}

pub(super) fn policy_rules(tools: &EffectiveToolPolicy) -> PolicyRules {
    let projection = tools.project(HarnessId::OpenCode);
    let star = tools
        .rules
        .iter()
        .filter(|rule| rule.is_wildcard() && rule.action != ToolAction::Allow)
        .map(|rule| rule.action)
        .max_by_key(|action| action.strictness());
    let mut keys = native_keys(&projection);
    let shell_restricted = keys.iter().any(|(name, rules)| {
        name == "bash" && rules.iter().any(|(_, effect)| *effect != ToolAction::Allow)
    });
    if shell_restricted {
        add_rule(&mut keys, "execute", STAR, ToolAction::Deny);
    }
    PolicyRules { star, keys }
}

fn native_keys(projection: &PolicyProjection<'_>) -> Vec<(String, Vec<(String, ToolAction)>)> {
    let mut keys = Vec::new();
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
            add_rule(&mut keys, action, resource, rule.action);
        }
    }
    keys
}

/// `bash(git *)` is action `bash`, resource `git *`; a bare name's resource is `*`.
fn split_native(native: &str) -> (&str, &str) {
    match native.split_once('(') {
        Some((action, rest)) => (action, rest.strip_suffix(')').unwrap_or(rest)),
        None => (native, STAR),
    }
}

fn add_rule(
    keys: &mut Vec<(String, Vec<(String, ToolAction)>)>,
    action: &str,
    resource: &str,
    effect: ToolAction,
) {
    let at = match keys.iter().position(|(name, _)| name == action) {
        Some(at) => at,
        None => {
            keys.push((action.into(), Vec::new()));
            keys.len() - 1
        }
    };
    let rules = &mut keys[at].1;
    match rules.iter_mut().find(|(existing, _)| existing == resource) {
        Some((_, existing)) => {
            if effect.strictness() > existing.strictness() {
                *existing = effect;
            }
        }
        None => rules.push((resource.into(), effect)),
    }
}

/// One rule, with what each source wrote for it.
#[derive(Debug, Clone)]
struct Rule {
    resource: String,
    policy: Option<ToolAction>,
    author: Option<ToolAction>,
    /// Position among the author's rules in this key.
    author_at: usize,
    effect: ToolAction,
}

/// The rules of one OpenCode action, matched by `v2` (the name OpenCode loads the key as).
#[derive(Debug)]
struct Key {
    name: String,
    v2: String,
    rules: Vec<Rule>,
}

impl Key {
    fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            v2: opencode_v2_action(name).into(),
            rules: Vec::new(),
        }
    }

    /// A key whose rules a policy rule is in is sorted; one only the override names keeps the
    /// author's order.
    fn sorted(&self) -> bool {
        self.rules.iter().any(|rule| rule.policy.is_some())
    }

    fn glob(&self) -> bool {
        is_glob(&self.name)
    }

    /// 0: only allows, 2: only denies, 1: the rest.
    fn band(&self) -> u8 {
        if self.rules.iter().all(|r| r.effect == ToolAction::Allow) {
            0
        } else if self.rules.iter().all(|r| r.effect == ToolAction::Deny) {
            2
        } else {
            1
        }
    }

    fn has_wide(&self, own: impl Fn(&Rule) -> bool) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.resource == STAR && own(rule))
    }

    fn value(&self) -> Value {
        match self.rules.as_slice() {
            [rule] if rule.resource == STAR => Value::String(rule.effect.as_str().into()),
            rules => Value::Mapping(
                rules
                    .iter()
                    .map(|rule| {
                        (
                            Value::String(rule.resource.clone()),
                            Value::String(rule.effect.as_str().into()),
                        )
                    })
                    .collect(),
            ),
        }
    }
}

fn is_glob(key: &str) -> bool {
    key.contains(['*', '?'])
}

/// OpenCode's wildcard match: `*` is any run, `?` one character, and a trailing ` *` also
/// matches the bare command.
pub(super) fn glob_match(pattern: &str, text: &str) -> bool {
    fn go(pattern: &[char], text: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some(('*', rest)) => (0..=text.len()).any(|at| go(rest, &text[at..])),
            Some(('?', rest)) => !text.is_empty() && go(rest, &text[1..]),
            Some((c, rest)) => text.first() == Some(c) && go(rest, &text[1..]),
        }
    }
    let text: Vec<char> = text.chars().collect();
    let whole: Vec<char> = pattern.chars().collect();
    go(&whole, &text)
        || pattern.strip_suffix(" *").is_some_and(|bare| {
            let bare: Vec<char> = bare.chars().collect();
            go(&bare, &text)
        })
}

/// One entry of the map in OpenCode's rule form. The launch bundle carries the list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PermissionRule {
    pub permission: String,
    pub pattern: String,
    pub action: ToolAction,
}

/// The map Mars writes, the same rules as a list, and whether it holds a rule that is not a
/// deny. `rules` is `map` read the way OpenCode reads it (a string value is pattern `*`), in
/// the order its last-match rule needs.
pub(crate) struct Built {
    pub map: Mapping,
    pub rules: Vec<PermissionRule>,
    pub non_deny: bool,
}

/// The author's `harness-overrides.opencode.permission`, in written order.
pub(crate) fn author_override(profile: &AgentProfile) -> Option<&OrderedValue> {
    profile
        .harness_overrides
        .get(&HarnessId::OpenCode)?
        .get("permission")
}

/// The `permission` map for `tools` and the author's `permission` override, empty when Mars
/// writes none. What differs from what a source wrote is added to `lossy`.
pub(crate) fn build(
    tools: &EffectiveToolPolicy,
    author: Option<&OrderedValue>,
    target: &str,
    lossy: &mut Vec<LossyField>,
) -> Built {
    let mut notes = Notes {
        target,
        lossy: Vec::new(),
    };
    let policy = policy_rules(tools);
    let s = policy.star;
    let mut keys = policy_keys(policy);
    let (a, author_keys) = match author {
        Some(value) => read_author(value, &mut notes),
        None => (None, Vec::new()),
    };
    let map_star = match (s, a) {
        (Some(x), Some(y)) => Some(if y.strictness() > x.strictness() {
            y
        } else {
            x
        }),
        (x, y) => x.or(y),
    };
    join(&mut keys, author_keys, &mut notes);
    for key in &mut keys {
        clamp(key, s, a, map_star, &mut notes);
    }
    keys.retain(|key| !key.rules.is_empty());
    deny_overlapping_globs(&mut keys, &mut notes);
    for key in &mut keys {
        if key.sorted() {
            key.rules.sort_by_key(|rule| rule.effect.strictness());
            notes.reordered(key);
        }
    }
    // A stable sort of an unordered set into bands: policy keys first, then the author's.
    keys.sort_by_key(Key::band);

    let mut map = Mapping::new();
    let mut rules = Vec::new();
    if let Some(effect) = map_star {
        map.insert(
            Value::String(STAR.into()),
            Value::String(effect.as_str().into()),
        );
        rules.push(PermissionRule {
            permission: STAR.into(),
            pattern: STAR.into(),
            action: effect,
        });
    }
    let mut non_deny = map_star.is_some_and(|effect| effect != ToolAction::Deny);
    for key in &keys {
        non_deny |= key.rules.iter().any(|rule| rule.effect != ToolAction::Deny);
        map.insert(Value::String(key.name.clone()), key.value());
        rules.extend(key.rules.iter().map(|rule| PermissionRule {
            permission: key.name.clone(),
            pattern: rule.resource.clone(),
            action: rule.effect,
        }));
    }
    lossy.extend(notes.lossy);
    Built {
        map,
        rules,
        non_deny,
    }
}

/// The policy as keys. Without a restricting `*` an allow restates OpenCode's default, and
/// written last it could only override outside rules, so it is left out.
fn policy_keys(policy: PolicyRules) -> Vec<Key> {
    let restricts = policy.star.is_some();
    policy
        .keys
        .into_iter()
        .filter_map(|(name, rules)| {
            let mut key = Key::new(&name);
            key.rules = rules
                .into_iter()
                .filter(|(_, effect)| restricts || *effect != ToolAction::Allow)
                .map(|(resource, effect)| Rule {
                    resource,
                    policy: Some(effect),
                    author: None,
                    author_at: 0,
                    effect,
                })
                .collect();
            (!key.rules.is_empty()).then_some(key)
        })
        .collect()
}

struct Notes<'a> {
    target: &'a str,
    lossy: Vec<LossyField>,
}

impl Notes<'_> {
    fn add(&mut self, field: String) {
        self.lossy.push(LossyField {
            field,
            target: self.target.into(),
            classification: Lossiness::Dropped(Lost::Policy),
        });
    }

    fn author(&mut self, at: &str, what: &str) {
        self.add(format!("harness-overrides.opencode.permission{at}: {what}"));
    }

    /// Report a key both sources name whose sort changed what an override rule decides: its
    /// place among the author's rules, or a stricter `*` rule that now follows it.
    fn reordered(&mut self, key: &Key) {
        let author_rules: Vec<_> = key.rules.iter().filter(|r| r.author.is_some()).collect();
        let moved = author_rules
            .windows(2)
            .any(|w| w[0].author_at > w[1].author_at)
            || author_rules.iter().any(|rule| {
                key.rules
                    .iter()
                    .skip_while(|other| other.resource != rule.resource)
                    .skip(1)
                    .any(|later| {
                        later.resource == STAR
                            && later.effect.strictness() > rule.effect.strictness()
                    })
            });
        if moved {
            self.author(
                &format!(".{}", key.name),
                "the profile's tool rules also apply to this tool, so Mars sorts the rules allow, ask, deny and an earlier rule no longer wins over a stricter later one",
            );
        }
    }

    /// A rule written at another effect than the source wrote.
    fn stricter(&mut self, key: &str, rule: &Rule, because: &str) {
        let (source, wrote) = match (rule.policy, rule.author) {
            (Some(wrote), None) => ("profile", wrote),
            (None, Some(wrote)) => ("override", wrote),
            _ => return,
        };
        let what = format!(
            "{} `{}` is written as {}, because {because}",
            wrote.as_str(),
            rule.resource,
            rule.effect.as_str()
        );
        if source == "profile" {
            self.add(format!("permission.{key}: {what}"));
        } else {
            self.author(&format!(".{key}"), &what);
        }
    }
}

/// The author's map as keys in written order, and the author's `*`. What cannot be read, or
/// OpenCode never applied, is reported and not written.
fn read_author(value: &OrderedValue, notes: &mut Notes<'_>) -> (Option<ToolAction>, Vec<Key>) {
    let OrderedValue::Object(entries) = value else {
        // A scalar throws away the author's whole permission policy.
        notes.author("", "not a map of rules, so Mars writes none of it");
        return (None, Vec::new());
    };
    let star = entries.get(STAR).and_then(read_effect);
    let star_at = star.and(entries.get_index_of(STAR));
    let mut keys: Vec<Key> = Vec::new();
    for (at, (name, value)) in entries.iter().enumerate() {
        if name == STAR {
            if star.is_none() {
                notes.author(
                    ".*",
                    "not an effect (allow, ask or deny), so Mars does not write it",
                );
            }
            continue;
        }
        if star_at.is_some_and(|star_at| at < star_at) {
            notes.author(
                &format!(".{name}"),
                "comes before the `*` entry, which OpenCode resolves after it, so it never applied and is not written",
            );
            continue;
        }
        let Some(rules) = read_rules(value) else {
            notes.author(
                &format!(".{name}"),
                "not an effect or a map of pattern to effect, so Mars does not write it",
            );
            continue;
        };
        let v2 = opencode_v2_action(name);
        let index = keys.iter().position(|key| key.v2 == v2).unwrap_or_else(|| {
            keys.push(Key::new(name));
            keys.len() - 1
        });
        let key = &mut keys[index];
        for (resource, effect) in rules {
            // The later entry decides for a call both match; an earlier rule it covers is dead.
            let before = key.rules.len();
            key.rules
                .retain(|rule| resource != STAR && rule.resource != resource);
            if key.rules.len() != before {
                notes.author(
                    &format!(".{name}"),
                    "a later rule covers an earlier one, which never applied and is not written",
                );
            }
            let author_at = key.rules.iter().map(|r| r.author_at + 1).max().unwrap_or(0);
            key.rules.push(Rule {
                resource,
                policy: None,
                author: Some(effect),
                author_at,
                effect,
            });
        }
    }
    (star, keys)
}

fn read_effect(value: &OrderedValue) -> Option<ToolAction> {
    match value {
        OrderedValue::String(effect) => ToolAction::parse(effect),
        _ => None,
    }
}

/// An author's action value as rules: `deny` is `*: deny`, a map is `pattern: effect`.
fn read_rules(value: &OrderedValue) -> Option<Vec<(String, ToolAction)>> {
    match value {
        OrderedValue::Object(map) => map
            .iter()
            .map(|(resource, effect)| Some((resource.clone(), read_effect(effect)?)))
            .collect(),
        other => Some(vec![(STAR.into(), read_effect(other)?)]),
    }
}

/// Merge the author's keys into the policy's: one key per v2 name, the stricter effect for a
/// resource both name. Keys only the author names follow, in the author's order.
fn join(keys: &mut Vec<Key>, author: Vec<Key>, notes: &mut Notes<'_>) {
    for theirs in author {
        let Some(key) = keys.iter_mut().find(|key| key.v2 == theirs.v2) else {
            keys.push(theirs);
            continue;
        };
        for rule in theirs.rules {
            match key
                .rules
                .iter_mut()
                .find(|own| own.resource == rule.resource)
            {
                Some(own) => {
                    own.author = rule.author;
                    own.author_at = rule.author_at;
                    if rule.effect.strictness() > own.effect.strictness() {
                        own.effect = rule.effect;
                        notes.add(format!(
                            "permission.{}: the override sets `{}` stricter ({}) than the profile does",
                            key.name,
                            own.resource,
                            own.effect.as_str()
                        ));
                    } else if own
                        .policy
                        .is_some_and(|p| p.strictness() > rule.effect.strictness())
                    {
                        notes.author(
                            &format!(".{}", key.name),
                            &format!(
                                "`{}` is written as {}, because the profile sets it stricter",
                                own.resource,
                                own.effect.as_str()
                            ),
                        );
                    }
                }
                None => key.rules.push(rule),
            }
        }
    }
}

/// Step 2: a rule only one source wrote must not answer weaker than the other source does.
fn clamp(
    key: &mut Key,
    s: Option<ToolAction>,
    a: Option<ToolAction>,
    map_star: Option<ToolAction>,
    notes: &mut Notes<'_>,
) {
    let sorted = key.sorted();
    let policy_wide = key.has_wide(|rule| rule.policy.is_some());
    let author_wide = key.has_wide(|rule| rule.author.is_some());
    let name = key.name.clone();
    key.rules.retain_mut(|rule| {
        let (other_wide, bound) = match (rule.policy.is_some(), rule.author.is_some()) {
            (true, false) => (author_wide, a),
            (false, true) => (policy_wide, s),
            _ => return true,
        };
        let Some(bound) = bound.filter(|bound| *bound != ToolAction::Allow) else {
            return true;
        };
        if other_wide || rule.effect.strictness() >= bound.strictness() {
            return true;
        }
        let because = if rule.policy.is_some() {
            format!("the override's `*` is {}", bound.as_str())
        } else {
            format!("the profile's `*` is {}", bound.as_str())
        };
        if rule.effect == ToolAction::Allow && sorted && map_star == Some(bound) {
            let wrote = match rule.policy {
                Some(_) => format!("permission.{name}"),
                None => format!("harness-overrides.opencode.permission.{name}"),
            };
            notes.add(format!(
                "{wrote}: allow `{}` is not written, because {because}",
                rule.resource
            ));
            return false;
        }
        rule.effect = if rule.effect == ToolAction::Ask {
            ToolAction::Deny
        } else {
            bound
        };
        notes.stricter(&name, rule, &because);
        true
    });
}

/// Step 4: two middle-band keys that can match one call are safe only when every rule in both has
/// one effect; otherwise each glob key of the pair is written as `deny`.
fn deny_overlapping_globs(keys: &mut [Key], notes: &mut Notes<'_>) {
    let middle: Vec<usize> = (0..keys.len()).filter(|&i| keys[i].band() == 1).collect();
    let mut denied = Vec::new();
    for (n, &i) in middle.iter().enumerate() {
        for &j in &middle[n + 1..] {
            let (a, b) = (&keys[i], &keys[j]);
            let overlap = (a.glob() && b.glob())
                || (a.glob() && glob_match(&a.v2, &b.v2))
                || (b.glob() && glob_match(&b.v2, &a.v2));
            let mut effects = a.rules.iter().chain(&b.rules).map(|rule| rule.effect);
            let uniform = effects
                .next()
                .is_none_or(|first| effects.all(|effect| effect == first));
            if overlap && !uniform {
                denied.extend([i, j].into_iter().filter(|&k| keys[k].glob()));
            }
        }
    }
    denied.sort_unstable();
    denied.dedup();
    for i in denied {
        notes.add(format!(
            "permission.{}: written as deny, because it can match the same tool as another key whose rules differ",
            keys[i].name
        ));
        for rule in &mut keys[i].rules {
            rule.effect = ToolAction::Deny;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::lower::lower_to_opencode;
    use super::*;
    use crate::compiler::agents::lower::NativeModel;
    use mars_schema::frontmatter::Frontmatter;

    /// A call: the OpenCode action as loaded, and its argument.
    type Call = (&'static str, &'static str);

    const CALLS: &[Call] = &[
        ("shell", "git status"),
        ("shell", "git push"),
        ("shell", "rm -rf x"),
        ("shell", "cat f"),
        ("shell", "ls"),
        ("read", "a.rs"),
        ("read", "a.env"),
        ("edit", "a.rs"),
        ("webfetch", "x"),
        ("subagent", "x"),
        ("execute", "x"),
        ("srv_x", "x"),
        ("srv_tool", "x"),
        ("srv_other", "x"),
        ("other_tool", "x"),
        ("task_x", "x"),
        ("plan_enter", "x"),
    ];

    /// OpenCode's last match over rules in written order; keys are renamed to v2 on load and
    /// glob-matched against the call's action. `None` when no rule matches.
    fn last_match(
        rules: &[(String, String, ToolAction)],
        (action, arg): Call,
    ) -> Option<ToolAction> {
        rules
            .iter()
            .rev()
            .find(|(key, resource, _)| {
                glob_match(opencode_v2_action(key), action) && glob_match(resource, arg)
            })
            .map(|(_, _, effect)| *effect)
    }

    fn flatten(map: &Mapping) -> Vec<(String, String, ToolAction)> {
        let mut out = Vec::new();
        for (key, value) in map {
            let key = key.as_str().unwrap().to_string();
            match value {
                Value::String(effect) => {
                    out.push((key, STAR.into(), ToolAction::parse(effect).unwrap()))
                }
                Value::Mapping(rules) => {
                    for (resource, effect) in rules {
                        out.push((
                            key.clone(),
                            resource.as_str().unwrap().into(),
                            ToolAction::parse(effect.as_str().unwrap()).unwrap(),
                        ));
                    }
                }
                other => panic!("{other:?}"),
            }
        }
        out
    }

    /// Mars semantics: the strictest named rule that matches, else `*`.
    fn policy_answer(policy: &PolicyRules, call: Call) -> (Option<ToolAction>, bool) {
        let mut named = policy
            .keys
            .iter()
            .flat_map(|(key, rules)| rules.iter().map(move |(r, e)| (key, r, *e)))
            .filter(|(key, resource, _)| {
                glob_match(opencode_v2_action(key), call.0) && glob_match(resource, call.1)
            })
            .map(|(_, _, effect)| effect)
            .collect::<Vec<_>>();
        named.sort_by_key(|effect| effect.strictness());
        match named.pop() {
            Some(effect) => (Some(effect), true),
            None => (policy.star, false),
        }
    }

    fn strictness(effect: Option<ToolAction>) -> u8 {
        effect.map_or(0, ToolAction::strictness)
    }

    /// Lower an agent and return the file's `permission` map and the lossy fields.
    fn lower(tools: &str, permission: &str) -> (mars_schema::agent::AgentProfile, Mapping, usize) {
        let mut content = String::from("---\nname: r\n");
        if !tools.is_empty() {
            content += &format!("tools:\n{tools}\n");
        }
        if !permission.is_empty() {
            content += &format!("harness-overrides:\n  opencode:\n    permission:\n{permission}\n");
        }
        content += "---\n# body";
        let reading = mars_schema::agent::read(&content);
        let profile = reading.fields.unwrap();
        let fm = Frontmatter::parse(&content).unwrap();
        let out = lower_to_opencode(&profile, fm.body(), &NativeModel::Inherit);
        let text = String::from_utf8(out.bytes).unwrap();
        let doc: Value = serde_yaml::from_str(text.split("---").nth(1).unwrap()).unwrap();
        let map = match doc.get("permission") {
            Some(Value::Mapping(map)) => map.clone(),
            _ => Mapping::new(),
        };
        let reported = out.lossy_fields.len();
        (profile, map, reported)
    }

    /// The override as the evaluator reads it, from the YAML text and not through Mars.
    fn author_rules(permission: &str) -> Vec<(String, String, ToolAction)> {
        if permission.is_empty() {
            return Vec::new();
        }
        let doc: Value = serde_yaml::from_str(permission).unwrap();
        flatten(doc.as_mapping().unwrap())
    }

    fn check(name: &str, tools: &str, permission: &str) {
        let (profile, map, _) = lower(tools, permission);
        let policy = policy_rules(&profile.tools);
        let written = flatten(&map);
        let bundled = crate::build::resolve_bundle_tools(&profile, "opencode")
            .unwrap()
            .opencode_permission
            .unwrap()
            .into_iter()
            .map(|r| (r.permission, r.pattern, r.action))
            .collect::<Vec<_>>();
        assert_eq!(
            bundled, written,
            "{name}: the bundle's list is not the file's map"
        );
        let author = author_rules(permission);
        for call in CALLS {
            let f = last_match(&written, *call);
            let (p, p_named) = policy_answer(&policy, *call);
            let a = last_match(&author, *call);
            assert!(
                strictness(f) >= strictness(p),
                "{name}: {call:?} is {f:?}, the policy says {p:?}; map {written:?}"
            );
            assert!(
                strictness(f) >= strictness(a),
                "{name}: {call:?} is {f:?}, the override says {a:?}; map {written:?}"
            );
            if permission.is_empty() {
                assert_eq!(
                    strictness(f),
                    strictness(p),
                    "{name}: {call:?} is {f:?}, the policy says {p:?}; map {written:?}"
                );
            }
            if !p_named && policy.star.is_none() && a.is_none() {
                assert!(
                    f.is_none(),
                    "{name}: {call:?} got a rule {f:?}; map {written:?}"
                );
            }
        }
    }

    #[test]
    fn the_map_is_never_looser_than_the_policy_or_the_override() {
        const CASES: &[(&str, &str, &str)] = &[
            // (name, tools, permission override)
            (
                "glob action key order",
                "  '*': deny\n  mcp(srv/x): deny\n  mcp(srv): allow",
                "",
            ),
            (
                "allowlist",
                "  '*': deny\n  read: allow\n  bash(git *): allow",
                "",
            ),
            (
                "ask beside a scoped deny",
                "  bash: ask\n  bash(rm *): deny",
                "",
            ),
            ("shell deny", "  bash: deny", ""),
            (
                "scoped allow under ask",
                "  '*': ask\n  bash(git *): allow",
                "",
            ),
            ("edit and write", "  write: deny\n  edit: allow", ""),
            (
                "no tool rules, author star last",
                "",
                "      read: allow\n      '*': deny",
            ),
            (
                "author star and nested star",
                "  webfetch: deny",
                "      read: allow\n      '*': deny\n      bash:\n        git *: allow\n        '*': deny",
            ),
            (
                "glob key against a scoped key",
                "  bash: ask\n  bash(rm *): deny",
                "      'sh*':\n        '*': ask\n        cat *: deny",
            ),
            (
                "allow under profile star",
                "  '*': deny\n  bash(git *): allow",
                "      bash: allow",
            ),
            (
                "profile allow under override star",
                "  '*': deny\n  read: allow",
                "      '*': ask",
            ),
            (
                "author key order kept",
                "  '*': ask",
                "      bash:\n        '*': deny\n        git *: allow",
            ),
            (
                "keys only the author names",
                "  webfetch: deny",
                "      bash:\n        '*': ask\n        git *: allow",
            ),
            (
                "both key-wide rules",
                "  '*': deny\n  bash(git *): allow",
                "      '*': deny\n      bash: allow",
            ),
            (
                "ask against author allowlist",
                "  bash: ask",
                "      '*': deny\n      bash:\n        git *: allow",
            ),
            (
                "write then edit",
                "  '*': deny",
                "      write: deny\n      edit: allow",
            ),
            ("shell spelling", "  bash: deny", "      shell: allow"),
            ("v2 glob", "  bash: deny", "      'sh*': allow"),
            (
                "v1 glob does not match",
                "  bash: deny",
                "      'ba*': allow",
            ),
            (
                "two globs",
                "  mcp(srv): ask\n  '*': deny",
                "      'srv*':\n        a: allow\n        '*': ask",
            ),
            (
                "author deny glob over allow",
                "  '*': deny\n  read: allow",
                "      'rea*': deny",
            ),
            (
                "author allow glob under deny",
                "  read: deny",
                "      'rea*': allow",
            ),
            (
                "author star ask",
                "  '*': deny\n  read: allow\n  bash: allow",
                "      '*': ask",
            ),
            (
                "author star allow",
                "  '*': ask\n  read: allow",
                "      '*': allow",
            ),
            ("mcp star", "  mcp(*): deny\n  read: allow", ""),
        ];
        for (name, tools, permission) in CASES {
            check(name, tools, permission);
        }
    }

    /// Every policy beside every override from two small pools, so a pair nobody wrote down is
    /// a failing property too.
    #[test]
    fn the_map_holds_for_every_pair_of_policy_and_override() {
        const POLICIES: &[&str] = &[
            "",
            "  '*': deny",
            "  '*': ask",
            "  '*': deny\n  read: allow",
            "  '*': deny\n  bash(git *): allow",
            "  '*': deny\n  bash(git *): allow\n  mcp(srv/x): deny\n  mcp(srv): allow",
            "  '*': ask\n  bash(git *): allow\n  read: allow",
            "  bash: ask",
            "  bash: ask\n  bash(rm *): deny",
            "  bash: deny\n  write: allow",
            "  webfetch: deny",
            "  mcp(srv): ask\n  mcp(srv/tool): deny",
            "  '*': deny\n  edit: allow\n  write: deny",
        ];
        const OVERRIDES: &[&str] = &[
            "      '*': deny",
            "      '*': ask",
            "      '*': allow",
            "      read: allow\n      '*': deny",
            "      bash: allow",
            "      bash: ask",
            "      bash: {'*': ask, 'git *': allow}",
            "      bash: {'*': deny, 'git *': allow}",
            "      shell: {'git *': allow, 'rm *': ask}",
            "      'sh*': {'*': ask, 'cat *': deny}",
            "      'ba*': allow",
            "      'rea*': allow",
            "      read: deny\n      'rea*': allow",
            "      write: deny\n      edit: allow",
            "      edit: allow\n      write: deny",
            "      srv_tool: allow\n      'srv_*': ask",
            "      '*': ask\n      bash: {'*': deny, 'git *': allow}\n      read: allow",
            "      webfetch: allow\n      subagent: ask\n      task: deny",
            "      'srv*': {'a': allow, '*': ask}\n      srv_x: deny",
        ];
        for tools in POLICIES {
            check(tools, tools, "");
            for permission in OVERRIDES {
                check(&format!("{tools} / {permission}"), tools, permission);
            }
        }
    }
}
