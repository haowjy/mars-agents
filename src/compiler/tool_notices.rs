//! Info-level notes about tool rules that only `mars validate` prints: rules a stricter rule
//! covers, and the deprecated overlay lists.
//!
//! None of these is a lost restriction, so `mars sync` stays quiet about them.

use std::collections::BTreeMap;
use std::path::Path;

use crate::compiler::effective_tools::covered;
use crate::compiler::{ToolConfig, installed_agent_tools};
use crate::config::LoadedProjectConfig;
use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCollector, DiagnosticLevel};

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
    // One line per covered rule and covering rule, naming the agents it applies to.
    let mut covered_rules: BTreeMap<(String, String), Vec<&str>> = BTreeMap::new();
    for (name, tools) in &agents {
        for (index, by) in covered(tools) {
            let rule = &tools.rules[index];
            let key = (
                format!("{} {}", rule.action.as_str(), rule.render()),
                format!("{} {}", by.action.as_str(), by.render()),
            );
            covered_rules.entry(key).or_default().push(name);
        }
    }
    for ((rule, by), names) in covered_rules {
        notes.push(info(
            "tool-rule-covered",
            format!(
                "`{rule}` never applies: `{by}` covers it, in {} {}: {}",
                names.len(),
                if names.len() == 1 { "agent" } else { "agents" },
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
