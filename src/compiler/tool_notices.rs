//! Info-level notes about tool rules that only `mars validate` prints: rules a stricter rule
//! covers.
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
