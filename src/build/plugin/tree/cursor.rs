use crate::compiler::agents::HarnessKind;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::sync::target::TargetItem;

use super::{StandardConfigFormat, StandardTreeOutput, TreeProfile};

pub(super) static PROFILE: TreeProfile = TreeProfile {
    format: &crate::plugin::CURSOR_TREE_SPEC,
    harness: HarnessKind::Cursor,
    agents: Some(preserve_agent),
    native_model_aliases: &[],
    plugin_root_token: Some("${CURSOR_PLUGIN_ROOT}"),
    check_hook,
    standard_output: Some(StandardTreeOutput {
        manifest_path: ".cursor-plugin/plugin.json",
        manifest_description: "Cursor plugin manifest",
        config_format: StandardConfigFormat::Cursor,
    }),
    write_config: super::write_standard_config,
    finish: super::finish_standard_tree,
};

fn preserve_agent(
    lowered: &str,
    _source: &crate::frontmatter::Frontmatter,
    _item: &TargetItem,
    _diag: &mut DiagnosticCollector,
) -> Result<String, MarsError> {
    Ok(lowered.to_string())
}

fn check_hook(_hook: &crate::compiler::hooks::ParsedHookItem, _diag: &mut DiagnosticCollector) {}
