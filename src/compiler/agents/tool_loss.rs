//! Which tool rules a native agent file cannot carry, and what each costs.
//!
//! One classifier for every target: [`rule_loss`] names the kind a rule costs, and a target's
//! [`Carries`] says what its file can write. [`losses`] joins them, so no target guesses a kind
//! from a lowered field.

use crate::compiler::lossiness::{Lossiness, LossyField, Lost, ProjectLoss};
use crate::compiler::tool_policy::ToolPolicyRuntime;
use mars_schema::HarnessId;
use mars_schema::tool_names::Capability;
use mars_schema::tool_policy::{EffectiveToolPolicy, RuleOrigin, ToolAction, ToolRule};

/// How much of the deny side a target's file can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DenyCarry {
    Nothing,
    /// Unscoped names only (OpenCode `permission: {tool: deny}`).
    Unscoped,
    /// Every rule Mars can name (Claude `disallowed-tools`).
    All,
}

/// What one target's native agent file can say about tool rules.
#[derive(Debug, Clone, Copy)]
pub(super) struct Carries {
    pub deny: DenyCarry,
    /// The file has a tool list that restricts the agent to what it names (Claude `tools:`).
    pub allowlist: bool,
    /// A deny of file writes is quiet: the file's read-only sandbox already blocks them.
    pub writes_covered: bool,
    /// How an MCP rule that loses nothing is classed; `None` records nothing for it.
    pub mcp_remainder: Option<Lossiness>,
}

/// What losing `rule` costs the agent: the shell, an MCP server, or another tool.
pub(super) fn rule_loss(rule: &ToolRule) -> Lost {
    match rule.capability {
        Capability::Mcp(_) => Lost::Mcp,
        Capability::Builtin("bash") => Lost::Shell,
        _ => Lost::Tool,
    }
}

fn is_file_write(rule: &ToolRule) -> bool {
    matches!(
        rule.capability,
        Capability::Builtin("write" | "edit" | "notebook")
    )
}

/// What a target's file leaves out of an agent's tool rules.
pub(super) struct ToolLosses {
    /// The agent's own restrictions, one field per kind lost.
    pub agent: Vec<LossyField>,
    /// Project-wide restrictions (`[settings.tools]`): the project's, not any one agent's, so
    /// the caller records each once per target.
    pub project: Vec<ProjectLoss>,
}

/// Every restriction `tools` places on the agent that `carries` cannot write. A plain grant
/// loses nothing and records nothing.
pub(super) fn losses(
    tools: &EffectiveToolPolicy,
    harness: HarnessId,
    target: &str,
    carries: Carries,
) -> ToolLosses {
    let projection = tools.project(harness);
    let mut lost: Vec<(Lost, String)> = Vec::new();
    let mut covered: Vec<String> = Vec::new();
    let mut project: Vec<ProjectLoss> = Vec::new();

    for (rule, projected) in projection.rules() {
        // Only a target that writes names has a naming gap worth reporting.
        let gap = projected
            .gap()
            .filter(|_| carries.deny != DenyCarry::Nothing);
        let label = || match gap {
            Some(note) => format!("{} {} ({note})", rule.action.as_str(), rule.render()),
            None => format!("{} {}", rule.action.as_str(), rule.render()),
        };
        match rule.action {
            ToolAction::Deny if !rule.is_wildcard() => {
                let natives = projected.native();
                let carried = projected.gap().is_none()
                    && match carries.deny {
                        DenyCarry::Nothing => false,
                        DenyCarry::Unscoped => natives.iter().all(|name| !name.contains('(')),
                        DenyCarry::All => true,
                    };
                if carried {
                    continue;
                }
                if carries.writes_covered && is_file_write(rule) {
                    covered_by_sandbox(rule, label(), target, &mut covered, &mut project);
                } else {
                    lose(rule, label(), target, &mut lost, &mut project);
                }
            }
            ToolAction::Ask => lose(rule, label(), target, &mut lost, &mut project),
            _ => {}
        }
    }

    let allowlist_carried = carries.allowlist && !projection.available().is_empty();
    if tools.is_allowlist() && !allowlist_carried {
        record_allowlist_loss(tools, &mut lost);
    }
    let mut fields: Vec<LossyField> = [Lost::Shell, Lost::Tool, Lost::Mcp]
        .into_iter()
        .filter_map(|kind| {
            let rules: Vec<&str> = lost
                .iter()
                .filter(|(k, _)| *k == kind)
                .map(|(_, rule)| rule.as_str())
                .collect();
            (!rules.is_empty()).then(|| {
                let noun = if kind == Lost::Mcp { "mcp" } else { "tools" };
                LossyField {
                    field: format!("{noun} ({})", rules.join(", ")),
                    target: target.into(),
                    classification: Lossiness::Dropped(kind),
                }
            })
        })
        .collect();

    if !covered.is_empty() {
        fields.push(LossyField {
            field: format!("tools ({})", covered.join(", ")),
            target: target.into(),
            classification: Lossiness::Quiet {
                why: SANDBOX_COVERS_WRITES,
            },
        });
    }
    if let Some(remainder) = carries.mcp_remainder
        && tools.rules.iter().any(ToolRule::is_mcp)
        && !lost.iter().any(|(kind, _)| *kind == Lost::Mcp)
    {
        fields.push(LossyField {
            field: "mcp".into(),
            target: target.into(),
            classification: remainder,
        });
    }
    ToolLosses {
        agent: fields,
        project,
    }
}

/// The project key a [`ProjectLoss`] is named by, as in `[settings.tools]`.
fn project_key(rule: &ToolRule) -> String {
    format!("tools.{}", rule.render())
}

/// A project-wide rule's loss is recorded once for the project; the label of any other rule
/// comes back for the agent's own record.
fn agent_label(
    rule: &ToolRule,
    label: String,
    target: &str,
    classification: Lossiness,
    project: &mut Vec<ProjectLoss>,
) -> Option<String> {
    if rule.origin != RuleOrigin::Project {
        return Some(label);
    }
    project.push(ProjectLoss {
        key: project_key(rule),
        field: LossyField {
            field: label,
            target: target.into(),
            classification,
        },
    });
    None
}

fn lose(
    rule: &ToolRule,
    label: String,
    target: &str,
    lost: &mut Vec<(Lost, String)>,
    project: &mut Vec<ProjectLoss>,
) {
    let loss = rule_loss(rule);
    if let Some(label) = agent_label(rule, label, target, Lossiness::Dropped(loss), project) {
        lost.push((loss, label));
    }
}

fn covered_by_sandbox(
    rule: &ToolRule,
    label: String,
    target: &str,
    covered: &mut Vec<String>,
    project: &mut Vec<ProjectLoss>,
) {
    let quiet = Lossiness::Quiet {
        why: SANDBOX_COVERS_WRITES,
    };
    covered.extend(agent_label(rule, label, target, quiet, project));
}

const SANDBOX_COVERS_WRITES: &str =
    "the read-only sandbox written to this file already blocks file writes";

/// An allow list the file cannot hold withholds every tool it does not name; the shell too,
/// unless the list grants it plainly.
fn record_allowlist_loss(tools: &EffectiveToolPolicy, lost: &mut Vec<(Lost, String)>) {
    let withholds_shell = !tools.rules.iter().any(|rule| {
        rule.action == ToolAction::Allow
            && rule.pattern.is_none()
            && rule.capability == Capability::Builtin("bash")
    });
    let rules: Vec<String> = tools
        .rules
        .iter()
        .filter(|rule| !rule.is_mcp())
        .filter(|rule| rule.is_wildcard() || rule.action == ToolAction::Allow)
        .map(|rule| format!("{} {}", rule.action.as_str(), rule.render()))
        .collect();
    lost.push((Lost::Tool, rules.join(", ")));
    if withholds_shell {
        lost.push((Lost::Shell, "deny bash (not in the allow list)".into()));
    }
}
