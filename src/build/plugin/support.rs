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
        warn_hooks_without_tree_fragment(harnesses, built, target.accepted_hooks(), diag);
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

fn warn_hooks_without_tree_fragment(
    harnesses: &[&'static PluginHarnessSpec],
    built: &BuiltTree,
    accepted_hooks: &[crate::compiler::hooks::ParsedHookItem],
    diag: &mut DiagnosticCollector,
) {
    let target_dir = built.profile.descriptor().target_dir;
    let fragment = target_dir.trim_start_matches('.');
    let affected = selected_harnesses(harnesses)
        .filter(|harness| harness.tree.key == built.profile.key)
        .map(harness_display_name)
        .collect::<Vec<_>>();
    let affected = human_list(&affected);

    for hook in accepted_hooks {
        if hook.def.select(&[target_dir.to_owned()]).is_empty() {
            diag.warn(
                "plugin-hook-fragment-dropped",
                format!(
                    "hook `{}` from `{}` has no `{fragment}` fragment and its `[hooks]` render nothing in this tree; {affected} won't get it",
                    hook.def.name, hook.source_name
                ),
            );
            continue;
        }
        for (other, file) in hook.def.fragments() {
            if other != target_dir {
                diag.warn(
                    "plugin-hook-fragment-dropped",
                    format!(
                        "hook `{}` from `{}`: `{}` fragment `{}` does not reach the `{fragment}` plugin tree; {affected} won't get it",
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

/// The README support and verification tables are published documentation, so
/// their renderer lives with the test that keeps README.md matching the harness
/// rows: none of it ships in the binary.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::plugin::spec::{SupportNotes, harness_specs};

    fn render_readme_tables() -> String {
        let harnesses = harness_specs().collect::<Vec<_>>();
        let mut rendered = String::from("### Component support\n\n");
        render_header(&mut rendered, &harnesses);
        render_row(&mut rendered, "Tree", &harnesses, |harness| {
            harness.tree.key.to_string()
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
            |harness| render_verification(harness.verification.install),
        );
        for component in Component::SHIPPED {
            render_row(
                &mut rendered,
                component_table_label(component),
                &harnesses,
                |harness| match verification(harness, component) {
                    Some(verification) => render_verification(verification),
                    None => "n/a (impossible)".to_string(),
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

    /// A support cell is the state this module computes plus an optional harness note.
    ///
    /// The note may qualify the state but never restates it, so it cannot drift into
    /// claiming support the tree does not have.
    fn component_support_cell(harness: &PluginHarnessSpec, component: Component) -> String {
        let state = support_state(harness, component);
        let state_text = match (component, state) {
            (Component::Skills, SupportState::Supported) => "supported".to_string(),
            (Component::Agents, SupportState::DroppedWithWarning) => format!(
                "supported; {} dropped with warning",
                harness
                    .tree
                    .dropped_agent_fields
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
                let fragment = harness.tree.descriptor().target_dir.trim_start_matches('.');
                if harness.tree.key == "claude" {
                    "supported for `[hooks]` (rendered to what Codex also parses) and hooks with the `claude` fragment; other targets' fragments dropped with warning".to_string()
                } else {
                    format!(
                        "supported for hooks with the `{fragment}` fragment; `[hooks]` and others dropped with warning"
                    )
                }
            }
            (Component::Mcp, SupportState::Supported) => "supported".to_string(),
            (Component::BootstrapDocs, SupportState::Impossible) => "impossible".to_string(),
            _ => unreachable!("support state and component are internally consistent"),
        };
        match support_note(harness.notes, component) {
            Some(note) => format!("{state_text} ({note})"),
            None => state_text,
        }
    }

    fn support_note(notes: SupportNotes, component: Component) -> Option<&'static str> {
        match component {
            Component::Skills => notes.skills,
            Component::Agents => notes.agents,
            Component::Hooks => notes.hooks,
            Component::Mcp => notes.mcp,
            Component::BootstrapDocs => None,
        }
    }

    fn render_verification(verification: Verification) -> String {
        format!("{}{}", verification.level.as_str(), verification.detail)
    }

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
