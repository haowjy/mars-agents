//! An agent's effective tool rules: its profile's rules with the `mars.toml` overlay applied,
//! joined with the project-wide `[settings.tools]` rules.
//!
//! Every path that needs an agent's rules (the launch bundle, native agent files and the
//! plugin build) goes through [`effective_tools`], so they cannot disagree.

use mars_schema::tool_cover::{Relation, ToolKey, relation};
use mars_schema::tool_names::ParsedToolName;
use mars_schema::tool_policy::{EffectiveToolPolicy, RuleOrigin, ToolAction, ToolRule};
use mars_schema::tool_rule_map::ToolRuleMap;

/// The rules for one agent:
///
/// 1. The agent's side: `profile` without each key the overlay's rule entries name, with the
///    then those entries (origin `Overlay`).
/// 2. The project-wide side: `project` without the keys the overlay names, origin `Project`,
///    shaped by the agent's `'*'` rule so that the one flat list gives, under strictest-wins,
///    what two answers (the agent's, the project's; the stricter decides) would. A project-wide
///    rule restricts every agent and widens none:
///
///    | project rule | no `'*'` | `'*': allow` or `ask` | `'*': deny` |
///    |---|---|---|---|
///    | `deny` | joins | joins | joins |
///    | `ask` | joins | joins | cut down by [`cut_down_ask`] |
///    | `allow` | joins | dropped | dropped |
///
///    A project-wide rule the agent already states adds nothing: the agent's rule stays, with its
///    own origin. A cut-down `ask` is a new rule of origin `Project`.
///
/// The result is the agent's side followed by the project-wide rules it did not already hold.
/// Nothing the agent wrote is dropped: a rule a stricter one covers stays, and [`covered`]
/// names the pairs.
pub(crate) fn effective_tools(
    project: &ToolRuleMap,
    profile: &EffectiveToolPolicy,
    overlay: Option<&ToolRuleMap>,
) -> EffectiveToolPolicy {
    let named = |rule: &ToolRule| overlay_names(overlay, rule);
    let mut policy = EffectiveToolPolicy {
        rules: profile
            .rules
            .iter()
            .filter(|rule| !named(rule))
            .cloned()
            .collect(),
    };
    if let Some(overlay) = overlay {
        for (name, action) in overlay.iter() {
            policy.push_unique(
                ToolRule::from_parsed(name.parsed().clone(), action)
                    .with_origin(RuleOrigin::Overlay),
            );
        }
    }

    join_project(project, policy, named)
}

/// [`effective_tools`] for an agent side the caller already holds: `agent_rules` stand in for
/// the profile's and the overlay's rules, with their origins kept. The current `overlay` lifts the
/// project-wide rule for a key only when `agent_rules` hold an `Overlay` rule with the same key
/// and action as the overlay's entry. An exemption added or changed since the caller stored its
/// rules would otherwise drop the project rule without the rule that replaces it; the project
/// rule then joins and the strictest decides.
pub(crate) fn effective_tools_for_rules(
    project: &ToolRuleMap,
    agent_rules: Vec<ToolRule>,
    overlay: Option<&ToolRuleMap>,
) -> EffectiveToolPolicy {
    let exempt: Vec<ParsedToolName> = overlay
        .into_iter()
        .flat_map(|overlay| overlay.iter())
        .filter(|(name, action)| {
            agent_rules.iter().any(|held| {
                held.origin == RuleOrigin::Overlay
                    && held.action == *action
                    && parsed_key(held) == *name.parsed()
            })
        })
        .map(|(name, _)| name.parsed().clone())
        .collect();
    join_project(
        project,
        EffectiveToolPolicy { rules: agent_rules },
        |rule| exempt.contains(&parsed_key(rule)),
    )
}

/// Whether the overlay's rule entries name `rule`'s key.
fn overlay_names(overlay: Option<&ToolRuleMap>, rule: &ToolRule) -> bool {
    overlay.is_some_and(|overlay| overlay.contains_key(&parsed_key(rule)))
}

fn join_project(
    project: &ToolRuleMap,
    mut policy: EffectiveToolPolicy,
    named: impl Fn(&ToolRule) -> bool,
) -> EffectiveToolPolicy {
    let project_rules: Vec<ToolRule> = project
        .iter()
        .map(|(name, action)| {
            ToolRule::from_parsed(name.parsed().clone(), action).with_origin(RuleOrigin::Project)
        })
        .filter(|rule| !named(rule))
        .collect();
    let agent_star = policy
        .rules
        .iter()
        .filter(|rule| rule.is_wildcard())
        .map(|rule| rule.action)
        .max_by_key(|action| action.strictness());
    let agent_allows: Vec<ToolRule> = policy
        .with_action(ToolAction::Allow)
        .filter(|rule| !rule.is_wildcard())
        .cloned()
        .collect();
    for rule in project_rules {
        match (rule.action, agent_star) {
            (ToolAction::Deny, _)
            | (ToolAction::Ask, None | Some(ToolAction::Allow | ToolAction::Ask))
            | (ToolAction::Allow, None) => {
                policy.push_unique(rule);
            }
            (ToolAction::Ask, Some(ToolAction::Deny)) => {
                for ask in cut_down_ask(&rule, &agent_allows) {
                    policy.push_unique(ask);
                }
            }
            (ToolAction::Allow, Some(_)) => {}
        }
    }
    policy
}

/// A project-wide `ask` on an agent whose `'*': deny` closes every tool it does not allow: it may
/// only ask where the agent allows, never open a call the agent's answer refuses. For each
/// agent allow `A`:
/// - `A` covers or equals the ask: the ask joins as written.
/// - the ask covers `A`, or [`relation`] cannot tell: an ask with `A`'s key joins, which only
///   asks more than the pair needs.
/// - the two are disjoint: nothing.
fn cut_down_ask(ask: &ToolRule, agent_allows: &[ToolRule]) -> Vec<ToolRule> {
    let mut rules = Vec::new();
    for allow in agent_allows {
        match relation(ToolKey::from(allow), ToolKey::from(ask)) {
            Relation::Equal | Relation::Covers => rules.push(ask.clone()),
            Relation::CoveredBy | Relation::Unknown => {
                let mut cut = allow.clone();
                cut.action = ToolAction::Ask;
                rules.push(cut.with_origin(RuleOrigin::Project));
            }
            Relation::Disjoint => {}
        }
    }
    rules
}

fn parsed_key(rule: &ToolRule) -> ParsedToolName {
    ParsedToolName {
        capability: rule.capability.clone(),
        pattern: rule.pattern.clone(),
    }
}

/// Each allow or ask in `policy`, by index, with the stricter rule that covers it or has its
/// key: a call it matches also matches that rule, so under strictest-wins it never applies.
/// `*` takes no part, and a pair [`relation`] cannot decide is left out: the enforcer matches
/// the real call.
pub(crate) fn covered(policy: &EffectiveToolPolicy) -> Vec<(usize, &ToolRule)> {
    policy
        .rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| rule.action != ToolAction::Deny && !rule.is_wildcard())
        .filter_map(|(index, rule)| {
            policy
                .rules
                .iter()
                .filter(|other| other.action.strictness() > rule.action.strictness())
                .filter(|other| !other.is_wildcard())
                .filter(|other| {
                    matches!(
                        relation(ToolKey::from(*other), ToolKey::from(rule)),
                        Relation::Equal | Relation::Covers
                    )
                })
                .min_by_key(|other| std::cmp::Reverse(other.action.strictness()))
                .map(|other| (index, other))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mars_schema::tool_names::parse_mars_tool_name;

    fn agent_policy(yaml: &str) -> EffectiveToolPolicy {
        let reading = mars_schema::agent::read(yaml);
        assert!(
            reading.diagnostics.is_empty(),
            "agent diags: {:?}",
            reading.diagnostics
        );
        reading.fields.unwrap().tools
    }

    fn apply(policy: &EffectiveToolPolicy, overlay: &ToolRuleMap) -> EffectiveToolPolicy {
        effective_tools(&ToolRuleMap::default(), policy, Some(overlay))
    }

    /// `action:key` for each rule, with `@project` or `@overlay` after one of that origin.
    fn show(policy: &EffectiveToolPolicy) -> Vec<String> {
        policy
            .rules
            .iter()
            .map(|rule| match rule.origin {
                RuleOrigin::Profile => format!("{}:{}", rule.action.as_str(), rule.render()),
                origin => format!(
                    "{}:{}@{}",
                    rule.action.as_str(),
                    rule.render(),
                    origin.as_str()
                ),
            })
            .collect()
    }

    fn join(project: &str, profile: &str, overlay: &str) -> (Vec<String>, Vec<String>) {
        let project: ToolRuleMap = toml::from_str(project).unwrap();
        let tools = if profile.is_empty() {
            "tools: {}\n".to_string()
        } else {
            format!("tools:\n{profile}")
        };
        let profile = agent_policy(&format!("---\nname: a\ndescription: d\n{tools}---\n"));
        let overlay: ToolRuleMap = toml::from_str(overlay).unwrap();
        let policy = effective_tools(&project, &profile, Some(&overlay));
        let pairs = covered(&policy)
            .into_iter()
            .map(|(index, by)| format!("{} / {}", by.render(), policy.rules[index].render()))
            .collect();
        (show(&policy), pairs)
    }

    #[test]
    fn supplied_rules_lift_a_project_rule_only_with_the_matching_overlay_rule() {
        let project: ToolRuleMap = toml::from_str("'bash(rm *)' = 'ask'").unwrap();
        let overlay: ToolRuleMap = toml::from_str("'bash(rm *)' = 'deny'").unwrap();
        let rule = |action, origin| {
            let parsed = parse_mars_tool_name("bash(rm *)").unwrap();
            ToolRule::from_parsed(parsed, action).with_origin(origin)
        };
        let run = |rules: Vec<ToolRule>| {
            show(&effective_tools_for_rules(&project, rules, Some(&overlay)))
        };
        // Stored before the overlay entry existed: the project rule still applies.
        assert_eq!(run(vec![]), ["ask:bash(rm *)@project"]);
        // Stored with another action for the key: the project rule still applies.
        assert_eq!(
            run(vec![rule(ToolAction::Allow, RuleOrigin::Overlay)]),
            ["allow:bash(rm *)@overlay", "ask:bash(rm *)@project"]
        );
        // Stored with the same overlay rule: the project rule is lifted.
        assert_eq!(
            run(vec![rule(ToolAction::Deny, RuleOrigin::Overlay)]),
            ["deny:bash(rm *)@overlay"]
        );
    }

    #[test]
    fn project_denies_join_every_rule_and_covered_pairs_are_named() {
        // (project, profile yaml, overlay, rules, covered "by / rule")
        type Row = (
            &'static str,
            &'static str,
            &'static str,
            &'static [&'static str],
            &'static [&'static str],
        );
        let rows: &[Row] = &[
            // Nothing project-wide: the profile as it is.
            (
                "",
                "  bash: allow\n  read: deny\n",
                "",
                &["allow:bash", "deny:read"],
                &[],
            ),
            // A covered allow or ask stays in the policy; the pair is named.
            (
                "bash = 'deny'",
                "  'bash(rg *)': allow\n",
                "",
                &["allow:bash(rg *)", "deny:bash@project"],
                &["bash / bash(rg *)"],
            ),
            (
                "edit = 'deny'",
                "  write: allow\n  read: allow\n",
                "",
                &["allow:write", "allow:read", "deny:edit@project"],
                &["edit / write"],
            ),
            (
                "'bash(git *)' = 'deny'",
                "  'bash(git status *)': allow\n",
                "",
                &["allow:bash(git status *)", "deny:bash(git *)@project"],
                &["bash(git *) / bash(git status *)"],
            ),
            (
                "bash = 'deny'",
                "  'bash(git commit *)': ask\n",
                "",
                &["ask:bash(git commit *)", "deny:bash@project"],
                &["bash / bash(git commit *)"],
            ),
            // An agent deny of the same key is the project rule; the other denies stay.
            (
                "'bash(pkill -f *)' = 'deny'",
                "  'bash(pkill -f:*)': deny\n  'bash(ls *)': deny\n",
                "",
                &["deny:bash(pkill -f *)", "deny:bash(ls *)"],
                &[],
            ),
            // An unscoped agent allow is not covered by a scoped project deny.
            (
                "'bash(pkill -f *)' = 'deny'",
                "  bash: allow\n",
                "",
                &["allow:bash", "deny:bash(pkill -f *)@project"],
                &[],
            ),
            // Different capabilities do not touch; `web` covers `web_fetch`.
            (
                "web = 'deny'",
                "  bash: allow\n  web_fetch: allow\n",
                "",
                &["allow:bash", "allow:web_fetch", "deny:web@project"],
                &["web / web_fetch"],
            ),
            // The first covering deny is named.
            (
                "'bash(git *)' = 'deny'",
                "  bash: deny\n  'bash(git status *)': allow\n",
                "",
                &[
                    "deny:bash",
                    "allow:bash(git status *)",
                    "deny:bash(git *)@project",
                ],
                &["bash / bash(git status *)"],
            ),
            // Keys that may overlap are not recorded: the enforcer matches the real call.
            (
                "'bash(git *)' = 'deny'",
                "  'bash(* status)': allow\n",
                "",
                &["allow:bash(* status)", "deny:bash(git *)@project"],
                &[],
            ),
            (
                "'read(secret/**)' = 'deny'",
                "  'read(src/**)': allow\n",
                "",
                &["allow:read(src/**)", "deny:read(secret/**)@project"],
                &[],
            ),
            // `*` is not measured against a key.
            (
                "bash = 'deny'",
                "  '*': allow\n",
                "",
                &["allow:*", "deny:bash@project"],
                &[],
            ),
            // The exemption: the same key in the overlay replaces the project rule and the profile's.
            (
                "'bash(pkill -f *)' = 'deny'",
                "  'bash(pkill -f:*)': deny\n",
                "'bash(pkill -f *)' = 'allow'",
                &["allow:bash(pkill -f *)@overlay"],
                &[],
            ),
            // Only the exact key: bash = allow leaves the scoped project deny.
            (
                "'bash(pkill -f *)' = 'deny'",
                "  read: allow\n",
                "bash = 'allow'",
                &[
                    "allow:read",
                    "allow:bash@overlay",
                    "deny:bash(pkill -f *)@project",
                ],
                &[],
            ),
            (
                "bash = 'deny'\n'bash(git *)' = 'deny'",
                "",
                "bash = 'allow'",
                &["allow:bash@overlay", "deny:bash(git *)@project"],
                &[],
            ),
            // The exempting allow is itself covered by another project deny.
            (
                "'bash(git *)' = 'deny'",
                "",
                "'bash(git status *)' = 'allow'",
                &[
                    "allow:bash(git status *)@overlay",
                    "deny:bash(git *)@project",
                ],
                &["bash(git *) / bash(git status *)"],
            ),
            // An overlay ask replaces the profile's rule of that key.
            (
                "",
                "  'bash(git push *)': deny\n",
                "'bash(git push *)' = 'ask'",
                &["ask:bash(git push *)@overlay"],
                &[],
            ),
            // Spellings of one key are one key.
            (
                "shell = 'deny'",
                "  Bash: allow\n",
                "",
                &["allow:bash", "deny:bash@project"],
                &["bash / bash"],
            ),
        ];
        for (project, profile, overlay, rules, covered) in rows {
            let got = join(project, profile, overlay);
            let want = (
                rules.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                covered.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            );
            assert_eq!(
                got, want,
                "project `{project}`, profile `{profile}`, overlay `{overlay}`"
            );
        }
    }

    /// A call: the tool and its argument (a command, or a path).
    type Call = (&'static str, &'static str);

    fn glob(pattern: &str, text: &str) -> bool {
        match pattern.split_once('*') {
            None => pattern == text,
            Some((head, rest)) => text.strip_prefix(head).is_some_and(|tail| {
                (0..=tail.len())
                    .filter(|at| tail.is_char_boundary(*at))
                    .any(|at| glob(rest, &tail[at..]))
            }),
        }
    }

    /// Whether a rule key matches a call, written out here and not through `relation`.
    fn matches(rule: &ToolRule, (tool, arg): Call) -> bool {
        let capability = rule.capability.name();
        let tool_matches = capability == tool
            || (capability == "edit" && tool == "write")
            || (capability == "web" && tool == "web_fetch");
        tool_matches
            && rule.pattern.as_deref().is_none_or(|pattern| {
                glob(pattern, arg) || pattern.strip_suffix(" *").is_some_and(|bare| bare == arg)
            })
    }

    /// Deny, then ask, then allow among the rules that match; `fallback` when none does.
    fn answer(rules: &[ToolRule], call: Call, fallback: Option<ToolAction>) -> Option<ToolAction> {
        rules
            .iter()
            .filter(|rule| !rule.is_wildcard() && matches(rule, call))
            .map(|rule| rule.action)
            .max_by_key(|action| action.strictness())
            .or(fallback)
    }

    fn star(rules: &[ToolRule]) -> Option<ToolAction> {
        rules
            .iter()
            .find(|rule| rule.is_wildcard())
            .map(|rule| rule.action)
    }

    fn strictness(action: Option<ToolAction>) -> i8 {
        action.map_or(-1, |action| action.strictness() as i8)
    }

    #[test]
    fn the_flat_policy_gives_what_two_answers_with_the_stricter_deciding_would() {
        let agents = [
            "tools: {}\n",
            "tools:\n  bash: allow\n  read: allow\n",
            "tools:\n  'bash(git status *)': allow\n  'bash(git log *)': ask\n",
            "tools:\n  '*': allow\n  'bash(rg *)': deny\n",
            "tools:\n  '*': ask\n  read: allow\n",
            "tools:\n  '*': deny\n  'bash(git status *)': allow\n  read: allow\n",
            "tools:\n  '*': deny\n  bash: allow\n",
            "tools:\n  '*': deny\n  'bash(*status)': allow\n",
            "tools:\n  '*': deny\n  'bash(git *)': allow\n  edit: allow\n",
            "tools:\n  '*': deny\n  bash: ask\n  web: allow\n",
        ];
        // `relation` cannot compare a pattern that is not a literal and one trailing `*` (the last
        // two projects and the agent with `bash(*status)`): the cut-down asks more there, never less.
        let projects = [
            "bash = 'allow'",
            "bash = 'ask'",
            "bash = 'deny'",
            "'bash(git *)' = 'ask'",
            "'bash(git push *)' = 'ask'",
            "'bash(git *)' = 'allow'",
            "'bash(git status *)' = 'ask'",
            "edit = 'ask'",
            "write = 'allow'",
            "web_fetch = 'ask'",
            "bash = 'allow'\nread = 'ask'",
            "'bash(*--force*)' = 'ask'",
            "'bash(*status)' = 'ask'",
        ];
        let calls: [Call; 13] = [
            ("bash", "git"),
            ("bash", "git status"),
            ("bash", "git status -s"),
            ("bash", "git log -1"),
            ("bash", "git push origin"),
            ("bash", "git push --force"),
            ("bash", "rg x"),
            ("bash", "ls"),
            ("read", "src/a"),
            ("write", "src/a"),
            ("edit", "src/a"),
            ("web_fetch", "example.com"),
            ("task", ""),
        ];
        for agent in agents {
            let profile = agent_policy(&format!("---\nname: a\ndescription: d\n{agent}---\n"));
            for (index, project) in projects.iter().enumerate() {
                let map: ToolRuleMap = toml::from_str(project).unwrap();
                let flat = effective_tools(&map, &profile, None);
                let project_rules = effective_tools(&map, &EffectiveToolPolicy::default(), None);
                let inexact = index >= projects.len() - 2 || agent.contains("(*status)");
                for call in calls {
                    let agent_answer = answer(&profile.rules, call, star(&profile.rules));
                    let project_answer = answer(&project_rules.rules, call, None);
                    // A project-wide allow counts only when the agent has no answer.
                    let two_sided = match (agent_answer, project_answer) {
                        (None, project) => project,
                        (agent, Some(ToolAction::Allow) | None) => agent,
                        (agent, project) => {
                            if strictness(project) > strictness(agent) {
                                project
                            } else {
                                agent
                            }
                        }
                    };
                    let got = answer(&flat.rules, call, star(&flat.rules));
                    let context = format!("agent `{agent}` project `{project}` call {call:?}");
                    assert!(
                        strictness(got) >= strictness(two_sided),
                        "weaker: {context}"
                    );
                    if !inexact {
                        assert_eq!(got, two_sided, "{context}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_launch_with_no_agent_gets_the_project_rules_alone() {
        let project: ToolRuleMap = toml::from_str("bash = 'deny'").unwrap();
        let policy = effective_tools(&project, &EffectiveToolPolicy::default(), None);
        assert_eq!(show(&policy), ["deny:bash@project"]);
    }

    #[test]
    fn an_overlay_entry_replaces_the_same_key_and_adds_the_rest() {
        let yaml = "tools:
  'bash(git push *)': deny
  'bash(git commit *)': ask
  read: allow
";
        let profile = agent_policy(&format!("---\nname: a\ndescription: d\n{yaml}---\n"));
        let overlay: ToolRuleMap =
            toml::from_str("'bash(git push *)' = 'ask'\nbash = 'allow'\nagent = 'deny'").unwrap();
        assert_eq!(
            apply(&profile, &overlay).describe(),
            [
                "ask:bash(git commit *)",
                "allow:read",
                "ask:bash(git push *)",
                "allow:bash",
                "deny:agent"
            ]
        );

        // `'*'` in an overlay is the agent's own default, so it replaces the profile's.
        let allowlist = agent_policy("---\nname: a\ntools: { '*': deny, read: allow }\n---\n");
        let overlay: ToolRuleMap = toml::from_str("'*' = 'allow'").unwrap();
        assert_eq!(
            apply(&allowlist, &overlay).describe(),
            ["allow:read", "allow:*"]
        );
    }
}
