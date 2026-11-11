//! Info-level notes about tool rules that only `mars validate` prints: what a project-wide deny
//! set aside, which profile forms change next release, and the deprecated overlay lists.
//!
//! None of these is a lost restriction, so `mars sync` stays quiet about them.

use std::collections::BTreeMap;
use std::path::Path;

use crate::compiler::{ToolConfig, installed_agent_tools};
use crate::config::LoadedProjectConfig;
use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCollector, DiagnosticLevel};
use mars_schema::tool_cover::{Relation, ToolKey, relation};
use mars_schema::tool_policy::ToolAction;

pub(crate) fn tool_rule_notices(
    project_root: &Path,
    loaded: &LoadedProjectConfig,
) -> Vec<Diagnostic> {
    let mut notes = Vec::new();
    let mut scan_diag = DiagnosticCollector::new();
    let agents = installed_agent_tools(
        &project_root.join(".mars"),
        ToolConfig {
            agent_overlays: &loaded.effective.agents,
            project_tools: &loaded.effective.settings.tools,
        },
        &mut scan_diag,
    );
    for (name, tools, _) in &agents {
        // An unscoped deny or ask beside an allow it covers, such as `bash: deny` beside
        // `bash(rg *): allow` or `edit: deny` beside `write: allow`, is settled by each harness
        // today and by the deny in the next release.
        for deny in tools.rules.iter().filter(|rule| {
            rule.action != ToolAction::Allow && !rule.is_wildcard() && rule.pattern.is_none()
        }) {
            for allow in tools.with_action(ToolAction::Allow) {
                if relation(ToolKey::from(deny), ToolKey::from(allow)) == Relation::Covers {
                    notes.push(info(
                        "tool-rule-form-changes",
                        format!(
                            "agent `{name}`: `{} {}` beside `allow {}` stops allowing it in the next release, when a deny covers every allow of its tool; keep the allow by narrowing the deny, or drop the allow",
                            deny.action.as_str(),
                            deny.render(),
                            allow.render(),
                        ),
                    ));
                }
            }
        }
    }

    // One line per set-aside rule and covering deny, naming the agents it applies to.
    let mut set_aside: BTreeMap<(String, String), Vec<&str>> = BTreeMap::new();
    for (name, _, settlement) in &agents {
        for entry in &settlement.set_aside {
            let key = (
                format!("{} {}", entry.rule.action.as_str(), entry.rule.render()),
                format!("{} {}", entry.by.action.as_str(), entry.by.render()),
            );
            set_aside.entry(key).or_default().push(name);
        }
    }
    for ((rule, by), names) in set_aside {
        notes.push(info(
            "tool-rule-set-aside",
            format!(
                "`{rule}` is set aside because the project-wide `{by}` covers it, in {} agent(s): {}",
                names.len(),
                names.join(", "),
            ),
        ));
    }

    let user_file = loaded.user.path.display().to_string();
    for (file, overlays) in [
        (user_file.as_str(), &loaded.user.local.agents),
        ("mars.toml", &loaded.config.agents),
        ("mars.local.toml", &loaded.local.agents),
    ] {
        for (name, overlay) in overlays {
            let lists: Vec<&str> = [
                ("allowed", overlay.tools.allowed.is_some()),
                ("disallowed", overlay.tools.disallowed.is_some()),
            ]
            .into_iter()
            .filter_map(|(list, present)| present.then_some(list))
            .collect();
            if !lists.is_empty() {
                let (verb, pronoun) = if lists.len() == 1 {
                    ("is", "it still works and keeps its")
                } else {
                    ("are", "they still work and keep their")
                };
                notes.push(info(
                    "tool-overlay-lists-deprecated",
                    format!(
                        "{file} [agents.{name}.tools] `{}` {verb} deprecated: {pronoun} meaning, but rule entries (`\"bash(git push *)\" = \"ask\"`) are the form to write new overlays in",
                        lists.join("` and `"),
                    ),
                ));
            }
        }
    }
    notes
}

fn info(code: &'static str, message: String) -> Diagnostic {
    Diagnostic {
        level: DiagnosticLevel::Info,
        code: code.into(),
        message,
        context: None,
        category: Some(DiagnosticCategory::Validation),
    }
}
