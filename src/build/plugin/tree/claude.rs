use crate::compiler::agents::HarnessKind;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::target::TargetItem;

use super::{StandardConfigFormat, StandardTreeOutput, TreeProfile, invalid_item};

const NATIVE_MODEL_ALIASES: &[&str] = &["opus", "sonnet", "haiku"];
pub(super) static PROFILE: TreeProfile = TreeProfile {
    format: &crate::plugin::CLAUDE_TREE_SPEC,
    harness: HarnessKind::Claude,
    agents: Some(apply_agent_policy),
    native_model_aliases: NATIVE_MODEL_ALIASES,
    plugin_root_token: Some("${CLAUDE_PLUGIN_ROOT}"),
    check_hook,
    standard_output: Some(StandardTreeOutput {
        manifest_path: ".claude-plugin/plugin.json",
        manifest_description: "Claude plugin manifest",
        config_format: StandardConfigFormat::Claude,
    }),
    write_config: super::write_standard_config,
    finish: super::finish_standard_tree,
};

fn check_hook(_hook: &crate::compiler::hooks::ParsedHookItem, _diag: &mut DiagnosticCollector) {}

fn apply_agent_policy(
    lowered: &str,
    source: &crate::frontmatter::Frontmatter,
    item: &TargetItem,
    diag: &mut DiagnosticCollector,
) -> Result<String, MarsError> {
    let mut plugin = crate::frontmatter::parse(lowered)
        .map_err(|error| invalid_item("agent", item, error.to_string()))?;

    // The universal agent schema does not interpret these Claude-native fields,
    // but Claude plugins do. Preserve only the documented plugin-safe subset.
    for field in [
        "maxTurns",
        "autoCompactWindow",
        "memory",
        "background",
        "omitClaudeMd",
        "isolation",
        "color",
    ] {
        if let Some(value) = source.get(field) {
            plugin.insert(field, value.clone());
        }
    }
    if let Some(value) = plugin.remove("disallowed-tools") {
        plugin.insert("disallowedTools", value);
    }

    for &field in crate::plugin::CLAUDE_TREE_SPEC.dropped_agent_fields {
        if source.get(field).is_some() {
            diag.warn(
                "plugin-agent-field-dropped",
                format!(
                    "agent `{}`: `{field}` is ignored by Claude plugin agents and was dropped",
                    item.id.name
                ),
            );
        }
        plugin.remove(field);
    }

    Ok(plugin.render())
}
