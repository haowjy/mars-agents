use crate::diagnostic::DiagnosticCollector;
use crate::sync::target::TargetState;
use mars_schema::item::ItemKind;

use super::spec::{PluginHarnessSpec, Verification, VerificationLevel};
use super::tree::BuiltTree;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Component {
    Skills,
    Agents,
    Hooks,
    Mcp,
    BootstrapDocs,
}

impl Component {
    const SHIPPED: [Self; 4] = [Self::Skills, Self::Agents, Self::Hooks, Self::Mcp];
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SupportState {
    Supported,
    DroppedWithWarning,
    Impossible,
}

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
        if support_state(harness, Component::Agents) == SupportState::Impossible {
            emit_agents_impossible(harness, count_items(target, ItemKind::Agent), diag);
        }
        emit_verification_info(harness, diag);
    }

    // Bootstrap docs have no slot in any plugin tree, so report the omission
    // once rather than repeating the same package-wide fact for every harness.
    let bootstrap_is_impossible = selected_harnesses(harnesses).all(|harness| {
        support_state(harness, Component::BootstrapDocs) == SupportState::Impossible
    });
    if bootstrap_is_impossible && count_items(target, ItemKind::BootstrapDoc) > 0 {
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

fn support_state(harness: &PluginHarnessSpec, component: Component) -> SupportState {
    match component {
        Component::Skills => SupportState::Supported,
        Component::Agents if harness.verification.agents.is_none() => SupportState::Impossible,
        Component::Agents if !harness.tree.dropped_agent_fields.is_empty() => {
            SupportState::DroppedWithWarning
        }
        Component::Agents => SupportState::Supported,
        Component::Hooks => SupportState::DroppedWithWarning,
        Component::Mcp => SupportState::Supported,
        Component::BootstrapDocs => SupportState::Impossible,
    }
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
    let target_dir = built.profile.descriptor().target_dir;
    let tree_name = target_dir.trim_start_matches('.');
    let affected = selected_harnesses(harnesses)
        .filter(|harness| harness.tree.key == built.profile.key)
        .map(harness_display_name)
        .collect::<Vec<_>>();
    let affected = human_list(&affected);

    for hook in accepted_hooks {
        if hook.def.select(&[target_dir.to_owned()]).is_empty() {
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

fn emit_verification_info(harness: &PluginHarnessSpec, diag: &mut DiagnosticCollector) {
    let mut docs_only = Vec::new();
    for component in Component::SHIPPED {
        let Some(verification) = verification(harness, component) else {
            continue;
        };
        match verification.level {
            VerificationLevel::Live => {}
            VerificationLevel::DocsOnly => docs_only.push(component_label(component)),
        }
    }
    if docs_only.is_empty() {
        return;
    }

    let state = format!(
        "{} {} docs-only",
        list(&docs_only),
        agreement(docs_only.len())
    );
    diag.info(
        "plugin-verification-below-live",
        format!(
            "{}: {}; not run on the {} binary",
            harness.key,
            state,
            harness_display_name(harness)
        ),
    );
}

fn component_label(component: Component) -> &'static str {
    match component {
        Component::Skills => "skills",
        Component::Agents => "agents",
        Component::Hooks => "hooks",
        Component::Mcp => "MCP",
        Component::BootstrapDocs => "bootstrap docs",
    }
}

fn agreement(count: usize) -> &'static str {
    if count == 1 { "is" } else { "are" }
}

fn list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [item] => (*item).to_string(),
        _ => items.join(", "),
    }
}

fn harness_display_name(harness: &PluginHarnessSpec) -> &'static str {
    harness.display_name
}

/// The evidence row for a shipped component, or `None` when the harness has no
/// such surface: agent evidence is exactly what makes agents loadable.
fn verification(harness: &PluginHarnessSpec, component: Component) -> Option<Verification> {
    let row = &harness.verification;
    match component {
        Component::Skills => Some(row.skills),
        Component::Agents => row.agents,
        Component::Hooks => Some(row.hooks),
        Component::Mcp => Some(row.mcp),
        Component::BootstrapDocs => None,
    }
}
