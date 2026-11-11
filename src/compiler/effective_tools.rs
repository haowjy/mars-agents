//! An agent's effective tool rules: its profile's rules with the `mars.toml` overlay applied.
//!
//! Every path that needs an agent's rules (the launch bundle, native agent files and the
//! plugin build) goes through [`effective_tools`], so they cannot disagree.

use crate::config::AgentOverlayTools;
use mars_schema::permission_set::{PermissionSet, merge_permission_set};
use mars_schema::tool_names::OverlayToolName;
use mars_schema::tool_policy::{EffectiveToolPolicy, RuleOrigin, ToolAction, ToolRule};
use mars_schema::tool_rule_map::ToolRuleMap;

pub(crate) struct EffectiveTools {
    pub policy: EffectiveToolPolicy,
}

/// `profile`'s rules with `overlay`'s `allowed` and `disallowed` lists applied. `project` is
/// the project-wide rule map and is not read yet.
pub(crate) fn effective_tools(
    _project: &ToolRuleMap,
    profile: &EffectiveToolPolicy,
    overlay: Option<&AgentOverlayTools>,
) -> EffectiveTools {
    let mut policy = profile.clone();
    if let Some(overlay) = overlay {
        if let Some(layer) = &overlay.allowed {
            apply_permission_set(&mut policy, layer, ToolAction::Allow);
        }
        if let Some(layer) = &overlay.disallowed {
            apply_permission_set(&mut policy, layer, ToolAction::Deny);
        }
    }
    EffectiveTools { policy }
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
