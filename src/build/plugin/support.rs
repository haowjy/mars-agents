use crate::diagnostic::DiagnosticCollector;
use crate::sync::target::TargetState;
use mars_schema::item::ItemKind;

use super::tree::{self, BuiltTree, TreeProfile};
use crate::plugin::{PluginHarnessSpec, Verification, VerificationLevel};

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
        warn_hooks_without_tree_fragment(harnesses, built, target.exported_hooks(), diag);
    }

    for harness in selected_harnesses(harnesses) {
        let profile = tree::profile(harness.tree);

        if support_state(harness, profile, Component::Agents) == SupportState::Impossible {
            emit_agents_impossible(harness, count_items(target, ItemKind::Agent), diag);
        }
        emit_verification_info(harness, profile, diag);
    }

    // Bootstrap docs have no slot in any plugin tree, so report the omission
    // once rather than repeating the same package-wide fact for every harness.
    let bootstrap_is_impossible = selected_harnesses(harnesses).all(|harness| {
        let profile = tree::profile(harness.tree);
        support_state(harness, profile, Component::BootstrapDocs) == SupportState::Impossible
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

fn support_state(
    harness: &PluginHarnessSpec,
    profile: &TreeProfile,
    component: Component,
) -> SupportState {
    match component {
        Component::Skills => SupportState::Supported,
        Component::Agents if profile.agents.is_none() || !harness.loads_agents => {
            SupportState::Impossible
        }
        Component::Agents if !profile.dropped_agent_fields().is_empty() => {
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

fn warn_hooks_without_tree_fragment(
    harnesses: &[&'static PluginHarnessSpec],
    built: &BuiltTree,
    exported_hooks: &[crate::compiler::hooks::ParsedHookItem],
    diag: &mut DiagnosticCollector,
) {
    let profile = tree::profile(built.tree());
    let target_dir = profile.descriptor().target_dir;
    let fragment = target_dir.trim_start_matches('.');
    let affected = selected_harnesses(harnesses)
        .filter(|harness| harness.tree == built.tree())
        .map(harness_display_name)
        .collect::<Vec<_>>();
    let affected = human_list(&affected);

    for hook in exported_hooks {
        if hook.def.targets.contains_key(target_dir) {
            continue;
        }
        diag.warn(
            "plugin-hook-fragment-dropped",
            format!(
                "hook `{}` from `{}` has no `{fragment}` fragment; {affected} won't get it",
                hook.def.name, hook.source_name
            ),
        );
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

fn emit_verification_info(
    harness: &PluginHarnessSpec,
    profile: &TreeProfile,
    diag: &mut DiagnosticCollector,
) {
    let mut docs_only = Vec::new();
    for component in Component::SHIPPED {
        if support_state(harness, profile, component) == SupportState::Impossible {
            continue;
        }
        match verification(harness, component).level {
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

fn verification(harness: &PluginHarnessSpec, component: Component) -> Verification {
    let row = &harness.verification;
    match component {
        Component::Skills => row.skills,
        Component::Agents => row
            .agents
            .expect("supported agents have verification evidence"),
        Component::Hooks => row.hooks,
        Component::Mcp => row.mcp,
        Component::BootstrapDocs => unreachable!("bootstrap documents have no plugin surface"),
    }
}

fn install_verification(harness: &PluginHarnessSpec) -> Verification {
    harness.verification.install
}

/// Render the generated README component-support and verification tables.
pub fn render_readme_tables() -> String {
    let harnesses = crate::plugin::harness_specs().collect::<Vec<_>>();
    let mut rendered = String::from("### Component support\n\n");
    render_header(&mut rendered, &harnesses);
    render_row(&mut rendered, "Tree", &harnesses, |harness| {
        harness.tree.as_str().to_string()
    });
    for component in [
        Component::Skills,
        Component::Agents,
        Component::Hooks,
        Component::Mcp,
        Component::BootstrapDocs,
    ] {
        render_row(
            &mut rendered,
            component_table_label(component),
            &harnesses,
            |harness| component_support_cell(harness, component),
        );
    }

    rendered.push_str("\n### Verification level\n\n");
    render_header(&mut rendered, &harnesses);
    render_row(
        &mut rendered,
        "Install from catalog",
        &harnesses,
        |harness| render_verification(install_verification(harness)),
    );
    for component in Component::SHIPPED {
        render_row(
            &mut rendered,
            component_table_label(component),
            &harnesses,
            |harness| {
                let profile = tree::profile(harness.tree);
                if support_state(harness, profile, component) == SupportState::Impossible {
                    "n/a (impossible)".to_string()
                } else {
                    render_verification(verification(harness, component))
                }
            },
        );
    }
    rendered
}

fn render_header(rendered: &mut String, harnesses: &[&PluginHarnessSpec]) {
    rendered.push_str("| |");
    for harness in harnesses {
        rendered.push_str(&format!(" {} |", harness_display_name(harness)));
    }
    rendered.push_str("\n|---|");
    for _ in harnesses {
        rendered.push_str("---|");
    }
    rendered.push('\n');
}

fn render_row(
    rendered: &mut String,
    label: &str,
    harnesses: &[&PluginHarnessSpec],
    mut cell: impl FnMut(&PluginHarnessSpec) -> String,
) {
    rendered.push_str(&format!("| {label} |"));
    for harness in harnesses {
        rendered.push_str(&format!(" {} |", cell(harness)));
    }
    rendered.push('\n');
}

fn component_table_label(component: Component) -> &'static str {
    match component {
        Component::Skills => "Skills",
        Component::Agents => "Agents",
        Component::Hooks => "Hooks",
        Component::Mcp => "MCP",
        Component::BootstrapDocs => "Bootstrap docs",
    }
}

fn component_support_cell(harness: &PluginHarnessSpec, component: Component) -> String {
    let profile = tree::profile(harness.tree);
    let state = support_state(harness, profile, component);
    if let Some(cell) = support_cell_override(harness, component) {
        return cell.to_string();
    }
    match (component, state) {
        (Component::Skills, SupportState::Supported) => "supported".to_string(),
        (Component::Agents, SupportState::DroppedWithWarning) => format!(
            "supported; {} dropped with warning",
            profile
                .dropped_agent_fields()
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (Component::Agents, SupportState::Supported) => "supported".to_string(),
        (Component::Agents, SupportState::Impossible) => format!(
            "**impossible**: {} plugins don't load agents",
            harness_display_name(harness)
        ),
        (Component::Hooks, SupportState::DroppedWithWarning) => {
            let fragment = profile.descriptor().target_dir.trim_start_matches('.');
            format!("supported for hooks with a `{fragment}` fragment; others dropped with warning")
        }
        (Component::Mcp, SupportState::Supported) => "supported".to_string(),
        (Component::BootstrapDocs, SupportState::Impossible) => "impossible".to_string(),
        _ => unreachable!("support state and component are internally consistent"),
    }
}

fn support_cell_override(
    harness: &PluginHarnessSpec,
    component: Component,
) -> Option<&'static str> {
    let cells = harness.support;
    match component {
        Component::Skills => cells.skills,
        Component::Agents => cells.agents,
        Component::Hooks => cells.hooks,
        Component::Mcp => cells.mcp,
        Component::BootstrapDocs => None,
    }
}

fn render_verification(verification: Verification) -> String {
    format!("{}{}", verification.level.as_str(), verification.detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readme_plugin_tables_match_renderer() {
        const START: &str = "<!-- BEGIN GENERATED PLUGIN SUPPORT -->\n";
        const END: &str = "<!-- END GENERATED PLUGIN SUPPORT -->";
        // Windows checkouts may convert the README to CRLF.
        let readme = include_str!("../../../README.md").replace("\r\n", "\n");
        let actual = readme
            .split_once(START)
            .and_then(|(_, rest)| rest.split_once(END).map(|(tables, _)| tables))
            .expect("README plugin support markers must exist");
        let rendered = render_readme_tables();

        assert_eq!(
            actual, rendered,
            "README plugin support tables are stale. Regenerate by replacing the text between {START:?} and {END:?} with:\n\n{rendered}"
        );
    }
}
