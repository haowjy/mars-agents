use crate::diagnostic::DiagnosticCollector;
use crate::sync::target::TargetState;
use mars_schema::item::ItemKind;

use super::spec::PluginHarnessSpec;
use super::tree::BuiltTree;

pub(super) fn emit(
    harnesses: &[&'static PluginHarnessSpec],
    target: &TargetState,
    built_trees: &[BuiltTree],
    diag: &mut DiagnosticCollector,
) {
    for built in built_trees {
        warn_hooks_without_tree_native_hook(harnesses, built, target.accepted_hooks(), diag);
    }

    for harness in selected_harnesses(harnesses) {
        if !harness.loads_agents {
            emit_agents_impossible(harness, count_items(target, ItemKind::Agent), diag);
        }
    }

    // Bootstrap docs have no slot in any plugin tree, so report the omission
    // once rather than repeating the same package-wide fact for every harness.
    if count_items(target, ItemKind::BootstrapDoc) > 0 {
        diag.info(
            "plugin-bootstrap-skipped",
            "bootstrap documents have no plugin surface and were skipped",
        );
    }
}

fn selected_harnesses<'a>(
    harnesses: &'a [&'static PluginHarnessSpec],
) -> impl Iterator<Item = &'static PluginHarnessSpec> + 'a {
    harnesses.iter().copied()
}

fn count_items(target: &TargetState, kind: ItemKind) -> usize {
    target
        .items
        .values()
        .filter(|item| item.id.kind == kind)
        .count()
}

fn warn_hooks_without_tree_native_hook(
    harnesses: &[&'static PluginHarnessSpec],
    built: &BuiltTree,
    accepted_hooks: &[crate::compiler::hooks::ParsedHookItem],
    diag: &mut DiagnosticCollector,
) {
    let tree_name = built.profile.harness.as_str();
    let target_dir = built.profile.harness.target_dir();
    let affected = selected_harnesses(harnesses)
        .filter(|harness| harness.tree.key == built.profile.key)
        .map(harness_display_name)
        .collect::<Vec<_>>();
    let affected = human_list(&affected);

    for hook in accepted_hooks {
        if hook
            .def
            .select(std::slice::from_ref(&target_dir))
            .is_empty()
        {
            diag.warn(
                "plugin-native-hook-dropped",
                format!(
                    "hook `{}` from `{}` has no `{tree_name}` native hook and its `[hooks]` render nothing in this tree; {affected} won't get it",
                    hook.def.name, hook.source_name
                ),
            );
            continue;
        }
        for (other, file) in hook.def.native_hooks() {
            if other != target_dir {
                diag.warn(
                    "plugin-native-hook-dropped",
                    format!(
                        "hook `{}` from `{}`: `{}` native hook `{}` does not reach the `{tree_name}` plugin tree; {affected} won't get it",
                        hook.def.name,
                        hook.source_name,
                        other.trim_start_matches('.'),
                        file.file
                    ),
                );
            }
        }
    }
}

fn human_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [item] => (*item).to_string(),
        [left, right] => format!("{left} and {right}"),
        _ => format!(
            "{} and {}",
            items[..items.len() - 1].join(", "),
            items[items.len() - 1]
        ),
    }
}

fn emit_agents_impossible(
    harness: &PluginHarnessSpec,
    agents: usize,
    diag: &mut DiagnosticCollector,
) {
    if agents == 0 {
        return;
    }
    let noun = if agents == 1 {
        "agent is"
    } else {
        "agents are"
    };
    diag.warn(
        "plugin-agents-impossible",
        format!(
            "{}: {agents} {noun} not loaded by {} plugins",
            harness.key,
            harness_display_name(harness)
        ),
    );
}

fn harness_display_name(harness: &PluginHarnessSpec) -> &'static str {
    harness.display_name
}
