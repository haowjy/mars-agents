//! An agent's effective tool rules: its profile's rules with the `mars.toml` overlay applied,
//! joined with the project-wide `[settings.tools]` denies.
//!
//! Every path that needs an agent's rules (the launch bundle, native agent files and the
//! plugin build) goes through [`effective_tools`], so they cannot disagree.

use crate::config::AgentOverlayTools;
use mars_schema::permission_set::{PermissionSet, merge_permission_set};
use mars_schema::tool_cover::{Relation, ToolKey, relation};
use mars_schema::tool_names::{OverlayToolName, ParsedToolName};
use mars_schema::tool_policy::{EffectiveToolPolicy, RuleOrigin, ToolAction, ToolRule};
use mars_schema::tool_rule_map::ToolRuleMap;

pub(crate) struct EffectiveTools {
    pub policy: EffectiveToolPolicy,
    pub settlement: Settlement,
}

/// What joining the project-wide denies did to an agent's allows and asks.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Settlement {
    /// Agent allows and asks that a project-wide deny covers, left out of the policy.
    pub set_aside: Vec<SetAside>,
    /// Pairs whose keys may overlap in a way Mars cannot prove either way; both stay in the
    /// policy and the enforcers decide.
    pub unsettled: Vec<Unsettled>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SetAside {
    pub rule: ToolRule,
    pub by: ToolRule,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Unsettled {
    pub project: ToolRule,
    pub agent: ToolRule,
}

/// The rules for one agent:
///
/// 1. The agent's side: `profile` without each key the overlay's rule entries name, with the
///    `allowed` / `disallowed` lists applied, then those entries (origin `Overlay`).
/// 2. The project-wide side: `project` (every rule a deny) without the keys the overlay names,
///    origin `Project`.
/// 3. Each agent allow or ask that a project-wide deny covers is set aside. A pair
///    [`relation`] cannot decide leaves both rules in and is recorded. A project-wide deny
///    the agent already states joins it in place as one rule of origin `Project`.
///
/// The result is the agent's side followed by the project-wide rules it did not already hold.
pub(crate) fn effective_tools(
    project: &ToolRuleMap,
    profile: &EffectiveToolPolicy,
    overlay: Option<&AgentOverlayTools>,
) -> EffectiveTools {
    let named = |rule: &ToolRule| {
        overlay.is_some_and(|overlay| overlay.rules.contains_key(&parsed_key(rule)))
    };
    let mut policy = EffectiveToolPolicy {
        rules: profile
            .rules
            .iter()
            .filter(|rule| !named(rule))
            .cloned()
            .collect(),
    };
    if let Some(overlay) = overlay {
        if let Some(layer) = &overlay.allowed {
            apply_permission_set(&mut policy, layer, ToolAction::Allow);
        }
        if let Some(layer) = &overlay.disallowed {
            apply_permission_set(&mut policy, layer, ToolAction::Deny);
        }
        for (name, action) in overlay.rules.iter() {
            policy.push_unique(
                ToolRule::from_parsed(name.parsed().clone(), action)
                    .with_origin(RuleOrigin::Overlay),
            );
        }
    }

    let project_rules: Vec<ToolRule> = project
        .iter()
        .map(|(name, action)| {
            ToolRule::from_parsed(name.parsed().clone(), action).with_origin(RuleOrigin::Project)
        })
        .filter(|rule| !named(rule))
        .collect();
    let settlement = settle(&mut policy, &project_rules);
    for rule in project_rules {
        policy.push_unique(rule);
    }
    EffectiveTools { policy, settlement }
}

fn parsed_key(rule: &ToolRule) -> ParsedToolName {
    ParsedToolName {
        capability: rule.capability.clone(),
        pattern: rule.pattern.clone(),
    }
}

/// Removes from `policy` each allow or ask a project-wide deny covers. `*` rules are left
/// alone: every enforcer ranks a named key above `*`, so a deny already wins there.
fn settle(policy: &mut EffectiveToolPolicy, project: &[ToolRule]) -> Settlement {
    let mut settlement = Settlement::default();
    policy.rules.retain(|agent| {
        if agent.action == ToolAction::Deny || agent.is_wildcard() {
            return true;
        }
        let mut unsettled = Vec::new();
        for denied in project {
            match relation(ToolKey::from(denied), ToolKey::from(agent)) {
                Relation::Equal | Relation::Covers => {
                    settlement.set_aside.push(SetAside {
                        rule: agent.clone(),
                        by: denied.clone(),
                    });
                    return false;
                }
                Relation::Unknown => unsettled.push(Unsettled {
                    project: denied.clone(),
                    agent: agent.clone(),
                }),
                Relation::CoveredBy | Relation::Disjoint => {}
            }
        }
        settlement.unsettled.extend(unsettled);
        true
    });
    settlement
}

fn apply_permission_set(
    policy: &mut EffectiveToolPolicy,
    layer: &PermissionSet<OverlayToolName>,
    action: ToolAction,
) {
    if layer.is_noop() {
        return;
    }
    let mut merged = policy
        .rules
        .iter()
        .filter(|rule| rule.action == action && !rule.is_wildcard())
        .cloned()
        .collect::<Vec<_>>();
    let layer = match layer {
        PermissionSet::Add(entries) => PermissionSet::Add(overlay_rules(entries, action)),
        PermissionSet::Replace { replace } => PermissionSet::Replace {
            replace: overlay_rules(replace, action),
        },
    };
    merge_permission_set(&mut merged, &layer);
    let keep_wildcard = !layer.replaces() || action == ToolAction::Deny;
    policy
        .rules
        .retain(|rule| rule.action != action || (keep_wildcard && rule.is_wildcard()));
    for rule in merged {
        policy.push_unique(rule);
    }
}

/// Rules for the tool names in a `mars.toml` overlay (`[agents.x.tools]`, `[skills.x.tools]`).
/// The names parsed when the config loaded, so there is nothing left to reject here.
fn overlay_rules(names: &[OverlayToolName], action: ToolAction) -> Vec<ToolRule> {
    names
        .iter()
        .map(|name| {
            ToolRule::from_parsed(name.parsed().clone(), action).with_origin(RuleOrigin::Overlay)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_policy(yaml: &str) -> EffectiveToolPolicy {
        let reading = mars_schema::agent::read(yaml);
        assert!(
            reading.diagnostics.is_empty(),
            "agent diags: {:?}",
            reading.diagnostics
        );
        reading.fields.unwrap().tools
    }

    fn apply(policy: &EffectiveToolPolicy, overlay: &AgentOverlayTools) -> EffectiveToolPolicy {
        effective_tools(&ToolRuleMap::default(), policy, Some(overlay)).policy
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

    fn join(
        project: &str,
        profile: &str,
        overlay: &str,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let project: ToolRuleMap = toml::from_str(project).unwrap();
        let tools = if profile.is_empty() {
            "tools: {}\n".to_string()
        } else {
            format!("tools:\n{profile}")
        };
        let profile = agent_policy(&format!("---\nname: a\ndescription: d\n{tools}---\n"));
        let overlay: AgentOverlayTools = toml::from_str(overlay).unwrap();
        let tools = effective_tools(&project, &profile, Some(&overlay));
        let pair = |project: &ToolRule, agent: &ToolRule| {
            format!("{} / {}", project.render(), agent.render())
        };
        (
            show(&tools.policy),
            tools
                .settlement
                .set_aside
                .iter()
                .map(|entry| pair(&entry.by, &entry.rule))
                .collect(),
            tools
                .settlement
                .unsettled
                .iter()
                .map(|entry| pair(&entry.project, &entry.agent))
                .collect(),
        )
    }

    #[test]
    fn project_denies_cover_agent_allows_and_asks() {
        // (project, profile yaml, overlay, rules, set aside "by / rule", unsettled)
        type Row = (
            &'static str,
            &'static str,
            &'static str,
            &'static [&'static str],
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
                &[],
            ),
            // The first table of the design: bash, edit, bash(git *).
            (
                "bash = 'deny'",
                "  'bash(rg *)': allow\n",
                "",
                &["deny:bash@project"],
                &["bash / bash(rg *)"],
                &[],
            ),
            (
                "edit = 'deny'",
                "  write: allow\n  read: allow\n",
                "",
                &["allow:read", "deny:edit@project"],
                &["edit / write"],
                &[],
            ),
            (
                "'bash(git *)' = 'deny'",
                "  'bash(git status *)': allow\n",
                "",
                &["deny:bash(git *)@project"],
                &["bash(git *) / bash(git status *)"],
                &[],
            ),
            // An ask is covered like an allow.
            (
                "bash = 'deny'",
                "  'bash(git commit *)': ask\n",
                "",
                &["deny:bash@project"],
                &["bash / bash(git commit *)"],
                &[],
            ),
            // An agent deny of the same key is the project rule; the other denies stay.
            (
                "'bash(pkill -f *)' = 'deny'",
                "  'bash(pkill -f:*)': deny\n  'bash(ls *)': deny\n",
                "",
                &["deny:bash(pkill -f *)@project", "deny:bash(ls *)"],
                &[],
                &[],
            ),
            // An unscoped agent allow does not cover a scoped project deny; both stay.
            (
                "'bash(pkill -f *)' = 'deny'",
                "  bash: allow\n",
                "",
                &["allow:bash", "deny:bash(pkill -f *)@project"],
                &[],
                &[],
            ),
            // Different capabilities do not touch.
            (
                "web = 'deny'",
                "  bash: allow\n  web_fetch: allow\n",
                "",
                &["allow:bash", "deny:web@project"],
                &["web / web_fetch"],
                &[],
            ),
            // An agent with bash: deny and a scoped allow, under bash(git *).
            (
                "'bash(git *)' = 'deny'",
                "  bash: deny\n  'bash(git status *)': allow\n",
                "",
                &["deny:bash", "deny:bash(git *)@project"],
                &["bash(git *) / bash(git status *)"],
                &[],
            ),
            // Keys that may overlap stay and are recorded.
            (
                "'bash(git *)' = 'deny'",
                "  'bash(* status)': allow\n",
                "",
                &["allow:bash(* status)", "deny:bash(git *)@project"],
                &[],
                &["bash(git *) / bash(* status)"],
            ),
            (
                "'read(secret/**)' = 'deny'",
                "  'read(src/**)': allow\n",
                "",
                &["allow:read(src/**)", "deny:read(secret/**)@project"],
                &[],
                &["read(secret/**) / read(src/**)"],
            ),
            // `*` is not measured against a key.
            (
                "bash = 'deny'",
                "  '*': allow\n",
                "",
                &["allow:*", "deny:bash@project"],
                &[],
                &[],
            ),
            // The exemption: the same key in the overlay replaces the project rule and the profile's.
            (
                "'bash(pkill -f *)' = 'deny'",
                "  'bash(pkill -f:*)': deny\n",
                "'bash(pkill -f *)' = 'allow'",
                &["allow:bash(pkill -f *)@overlay"],
                &[],
                &[],
            ),
            // Only the exact key: bash = allow leaves the scoped project deny, and its own allow stays.
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
                &[],
            ),
            // An exemption of an unscoped deny: the scoped project denies still cover scoped overlay allows.
            (
                "bash = 'deny'\n'bash(git *)' = 'deny'",
                "",
                "bash = 'allow'",
                &["allow:bash@overlay", "deny:bash(git *)@project"],
                &[],
                &[],
            ),
            // The exempting allow is itself set aside when another project deny covers it.
            (
                "'bash(git *)' = 'deny'",
                "",
                "'bash(git status *)' = 'allow'",
                &["deny:bash(git *)@project"],
                &["bash(git *) / bash(git status *)"],
                &[],
            ),
            // An overlay ask replaces the profile's rule of that key.
            (
                "",
                "  'bash(git push *)': deny\n",
                "'bash(git push *)' = 'ask'",
                &["ask:bash(git push *)@overlay"],
                &[],
                &[],
            ),
            // Spellings of one key are one key.
            (
                "shell = 'deny'",
                "  Bash: allow\n",
                "",
                &["deny:bash@project"],
                &["bash / bash"],
                &[],
            ),
            // replace clears the profile's allows, not the overlay's entries.
            (
                "",
                "  bash: allow\n",
                "allowed = { replace = ['read'] }\nedit = 'allow'",
                &["allow:read@overlay", "allow:edit@overlay"],
                &[],
                &[],
            ),
        ];
        for (project, profile, overlay, rules, set_aside, unsettled) in rows {
            let got = join(project, profile, overlay);
            let want = (
                rules.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                set_aside.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                unsettled.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            );
            assert_eq!(
                got, want,
                "project `{project}`, profile `{profile}`, overlay `{overlay}`"
            );
        }
    }

    #[test]
    fn a_launch_with_no_agent_gets_the_project_rules_alone() {
        let project: ToolRuleMap = toml::from_str("bash = 'deny'").unwrap();
        let tools = effective_tools(&project, &EffectiveToolPolicy::default(), None);
        assert_eq!(show(&tools.policy), ["deny:bash@project"]);
    }

    #[test]
    fn the_overlay_adds_or_replaces_agent_and_skill_permission_sets_alike() {
        let overlay = AgentOverlayTools {
            allowed: Some(vec!["bash".to_string().try_into().unwrap()].into()),
            disallowed: Some(vec!["agent".to_string().try_into().unwrap()].into()),
            ..Default::default()
        };
        let yaml = "tools:
  'bash(git push *)': deny
  'bash(git commit *)': ask
  read: allow
";

        // Plain lists add grants and denies while keeping asks.
        let mut agent = agent_policy(&format!("---\nname: a\ndescription: d\n{yaml}---\n"));
        agent = apply(&agent, &overlay);
        assert_eq!(
            agent.describe(),
            [
                "ask:bash(git commit *)",
                "allow:read",
                "allow:bash",
                "deny:bash(git push *)",
                "deny:agent"
            ]
        );

        // Explicit replacement gives the previous overlay behavior.
        let replacing = AgentOverlayTools {
            allowed: Some(PermissionSet::Replace {
                replace: vec!["bash".to_string().try_into().unwrap()],
            }),
            disallowed: Some(PermissionSet::Replace {
                replace: vec!["agent".to_string().try_into().unwrap()],
            }),
            ..Default::default()
        };
        let mut replaced = agent_policy(&format!("---\nname: a\ndescription: d\n{yaml}---\n"));
        replaced = apply(&replaced, &replacing);
        assert_eq!(
            replaced.describe(),
            ["ask:bash(git commit *)", "allow:bash", "deny:agent"]
        );

        // An authored empty list was a no-op in 0.17 and remains one now.
        let empty_lists = AgentOverlayTools {
            allowed: Some(Vec::new().into()),
            disallowed: Some(Vec::new().into()),
            ..Default::default()
        };
        let mut unchanged = agent_policy(&format!("---\nname: a\ndescription: d\n{yaml}---\n"));
        unchanged = apply(&unchanged, &empty_lists);
        assert_eq!(
            unchanged.describe(),
            [
                "deny:bash(git push *)",
                "ask:bash(git commit *)",
                "allow:read"
            ]
        );

        // A plain list adds alongside a wildcard grant. An explicit replacement drops that
        // wildcard grant, while preserving the deny rules.
        let wildcard_profile = "---\nname: a\ntools: { '*': allow, write: deny }\n---\n";
        let additive = AgentOverlayTools {
            allowed: Some(vec!["read".to_string().try_into().unwrap()].into()),
            disallowed: None,
            ..Default::default()
        };
        let mut widened = agent_policy(wildcard_profile);
        widened = apply(&widened, &additive);
        assert_eq!(widened.describe(), ["allow:*", "deny:write", "allow:read"]);

        let replacing = AgentOverlayTools {
            allowed: Some(PermissionSet::Replace {
                replace: vec!["read".to_string().try_into().unwrap()],
            }),
            ..AgentOverlayTools::default()
        };
        let mut narrowed = agent_policy(wildcard_profile);
        narrowed = apply(&narrowed, &replacing);
        assert_eq!(narrowed.describe(), ["deny:write", "allow:read"]);
    }
}
