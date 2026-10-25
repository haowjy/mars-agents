use crate::config::PluginHarness;
use crate::diagnostic::DiagnosticCollector;
use crate::lock::ItemKind;
use crate::sync::target::TargetState;

use super::PluginTree;
use super::tree::{self, BuiltTree, TreeProfile};

const CODEX_PROBED_VERSION: &str = "0.164.0";

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum VerificationLevel {
    Live,
    // The support model permits this level even though no current cell uses it.
    #[allow(dead_code)]
    InstallOnly,
    DocsOnly,
}

#[cfg(test)]
impl VerificationLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::InstallOnly => "install-only",
            Self::DocsOnly => "docs-only",
        }
    }
}

impl PluginHarness {
    const fn loads_agents(self) -> bool {
        !matches!(self, Self::Codex)
    }

    const fn mcp_callable(self) -> bool {
        !matches!(self, Self::Codex)
    }
}

pub(super) fn emit(
    harnesses: &[PluginHarness],
    target: &TargetState,
    built_trees: &[BuiltTree],
    diag: &mut DiagnosticCollector,
) {
    for built in built_trees {
        warn_hooks_without_tree_fragment(built, target.exported_hooks(), diag);
    }

    for harness in selected_harnesses(harnesses) {
        let built = built_tree_for_harness(harness, built_trees);
        let profile = tree::profile(harness.tree());

        if support_state(harness, profile, Component::Agents) == SupportState::Impossible {
            emit_agents_impossible(harness, count_items(target, ItemKind::Agent), diag);
        }
        if support_state(harness, profile, Component::Mcp) == SupportState::DroppedWithWarning {
            emit_mcp_dropped(harness, built.components.mcp_servers, diag);
        }
        emit_verification_info(harness, profile, diag);
    }

    // Bootstrap docs have no slot in any plugin tree, so report the omission
    // once rather than repeating the same package-wide fact for every harness.
    let bootstrap_is_impossible = selected_harnesses(harnesses).all(|harness| {
        let profile = tree::profile(harness.tree());
        support_state(harness, profile, Component::BootstrapDocs) == SupportState::Impossible
    });
    if bootstrap_is_impossible && count_items(target, ItemKind::BootstrapDoc) > 0 {
        diag.info(
            "plugin-bootstrap-skipped",
            "bootstrap documents have no plugin surface and were skipped",
        );
    }
}

fn selected_harnesses(harnesses: &[PluginHarness]) -> impl Iterator<Item = PluginHarness> + '_ {
    PluginHarness::DEFAULT
        .iter()
        .copied()
        .filter(|harness| harnesses.contains(harness))
}

fn built_tree_for_harness(harness: PluginHarness, built_trees: &[BuiltTree]) -> &BuiltTree {
    built_trees
        .iter()
        .find(|built| built.tree() == harness.tree())
        .expect("a listed harness has a built tree")
}

fn support_state(
    harness: PluginHarness,
    profile: &TreeProfile,
    component: Component,
) -> SupportState {
    match component {
        Component::Skills => SupportState::Supported,
        Component::Agents if profile.agents.is_none() || !harness.loads_agents() => {
            SupportState::Impossible
        }
        Component::Agents
            if profile.tree() == PluginTree::Claude
                && !tree::claude::DROPPED_AGENT_FIELDS.is_empty() =>
        {
            SupportState::DroppedWithWarning
        }
        Component::Agents => SupportState::Supported,
        Component::Hooks => SupportState::DroppedWithWarning,
        Component::Mcp if !harness.mcp_callable() => SupportState::DroppedWithWarning,
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
    built: &BuiltTree,
    exported_hooks: &[crate::compiler::hooks::ParsedHookItem],
    diag: &mut DiagnosticCollector,
) {
    let profile = tree::profile(built.tree());
    let target_dir = profile.descriptor().target_dir;
    let fragment = target_dir.trim_start_matches('.');
    let tree = built.tree().as_str();

    for hook in exported_hooks {
        if hook.def.targets.contains_key(target_dir) {
            continue;
        }
        diag.warn(
            "plugin-hook-fragment-dropped",
            format!(
                "hook `{}` from `{}` has no `{fragment}` fragment; not in the `{tree}` tree",
                hook.def.name, hook.source_name
            ),
        );
    }
}

fn emit_agents_impossible(harness: PluginHarness, agents: usize, diag: &mut DiagnosticCollector) {
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
            harness.as_str(),
            harness_display_name(harness)
        ),
    );
}

fn emit_mcp_dropped(harness: PluginHarness, mcp_servers: usize, diag: &mut DiagnosticCollector) {
    if mcp_servers == 0 {
        return;
    }
    let noun = if mcp_servers == 1 {
        "MCP server is"
    } else {
        "MCP servers are"
    };
    diag.warn(
        "plugin-mcp-unavailable",
        format!(
            "{}: {mcp_servers} {noun} listed but not callable in {} {CODEX_PROBED_VERSION}",
            harness.as_str(),
            harness_display_name(harness)
        ),
    );
}

fn emit_verification_info(
    harness: PluginHarness,
    profile: &TreeProfile,
    diag: &mut DiagnosticCollector,
) {
    let mut install_only = Vec::new();
    let mut docs_only = Vec::new();
    for component in Component::SHIPPED {
        if support_state(harness, profile, component) == SupportState::Impossible {
            continue;
        }
        match verification(harness, component).level {
            VerificationLevel::Live => {}
            VerificationLevel::InstallOnly => install_only.push(component_label(component)),
            VerificationLevel::DocsOnly => docs_only.push(component_label(component)),
        }
    }
    if install_only.is_empty() && docs_only.is_empty() {
        return;
    }

    let mut states = Vec::new();
    if !install_only.is_empty() {
        states.push(format!(
            "{} {} install-only",
            list(&install_only),
            agreement(install_only.len())
        ));
    }
    if !docs_only.is_empty() {
        states.push(format!(
            "{} {} docs-only",
            list(&docs_only),
            agreement(docs_only.len())
        ));
    }
    diag.info(
        "plugin-verification-below-live",
        format!(
            "{}: {}; not run on the {} binary",
            harness.as_str(),
            states.join("; "),
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

fn harness_display_name(harness: PluginHarness) -> &'static str {
    match harness {
        PluginHarness::Claude => "Claude",
        PluginHarness::Codex => "Codex",
        PluginHarness::Grok => "Grok",
        PluginHarness::Cursor => "Cursor",
        PluginHarness::OpenCode => "OpenCode",
    }
}

#[derive(Clone, Copy)]
struct Verification {
    level: VerificationLevel,
    #[cfg(test)]
    detail: &'static str,
}

const fn verified(level: VerificationLevel, _detail: &'static str) -> Verification {
    Verification {
        level,
        #[cfg(test)]
        detail: _detail,
    }
}

fn verification(harness: PluginHarness, component: Component) -> Verification {
    use Component::{Agents, Hooks, Mcp, Skills};
    use PluginHarness::{Claude, Codex, Cursor, Grok, OpenCode};
    use VerificationLevel::{DocsOnly, Live};

    match (harness, component) {
        (Claude, Skills | Mcp) => verified(Live, ""),
        (Claude, Agents) => verified(Live, " (subagent preload)"),
        (Claude, Hooks) => verified(Live, " (`${CLAUDE_PLUGIN_ROOT}`)"),
        (Codex, Skills) => verified(Live, ""),
        (Codex, Hooks) => verified(
            Live,
            " on a hand-built Claude-format tree (p9291); V re-runs it on the built tree",
        ),
        (Codex, Mcp) => verified(Live, ": listed, not callable"),
        (Grok, Skills) => verified(Live, ", with `resources/` (p9479)"),
        (Grok, Agents) => verified(Live, ": subagent, native preload, skills line (p9479)"),
        (Grok, Hooks) => verified(
            Live,
            ": `SessionEnd`, `SubagentStop`, `CLAUDE_PLUGIN_ROOT` (p9479)",
        ),
        (Grok, Mcp) => verified(DocsOnly, " (no server in any fixture)"),
        (Cursor, Skills | Agents | Hooks | Mcp) => verified(DocsOnly, ""),
        (OpenCode, Skills) => verified(Live, ", with `resources/`"),
        (OpenCode, Hooks) => verified(Live, " (composed fragment from a local path, p9477)"),
        (OpenCode, Mcp) => verified(Live, " (`<plugin>:<server>`, p9477)"),
        (Codex | OpenCode, Agents) | (_, Component::BootstrapDocs) => {
            unreachable!("impossible components have no verification level")
        }
    }
}

#[cfg(test)]
fn install_verification(harness: PluginHarness) -> Verification {
    use PluginHarness::{Claude, Codex, Cursor, Grok, OpenCode};
    use VerificationLevel::{DocsOnly, Live};

    match harness {
        Claude | Codex | Grok => verified(Live, ""),
        Cursor => verified(DocsOnly, ""),
        OpenCode => verified(Live, " (git)"),
    }
}

#[cfg(test)]
fn render_readme_tables(harnesses: &[PluginHarness]) -> String {
    let mut rendered = String::from("### Component support\n\n");
    render_header(&mut rendered, harnesses);
    render_row(&mut rendered, "Tree", harnesses, |harness| {
        harness.tree().as_str().to_string()
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
            harnesses,
            |harness| component_support_cell(harness, component),
        );
    }

    rendered.push_str("\n### Verification level\n\n");
    render_header(&mut rendered, harnesses);
    render_row(
        &mut rendered,
        "Install from catalog",
        harnesses,
        |harness| render_verification(install_verification(harness)),
    );
    for component in Component::SHIPPED {
        render_row(
            &mut rendered,
            component_table_label(component),
            harnesses,
            |harness| {
                let profile = tree::profile(harness.tree());
                if support_state(harness, profile, component) == SupportState::Impossible {
                    "n/a (impossible)".to_string()
                } else {
                    render_verification(verification(harness, component))
                }
            },
        );
    }
    render_row(&mut rendered, "Evidence", harnesses, |harness| {
        evidence(harness).to_string()
    });
    rendered
}

#[cfg(test)]
fn render_header(rendered: &mut String, harnesses: &[PluginHarness]) {
    rendered.push_str("| |");
    for harness in harnesses {
        rendered.push_str(&format!(" {} |", harness_display_name(*harness)));
    }
    rendered.push_str("\n|---|");
    for _ in harnesses {
        rendered.push_str("---|");
    }
    rendered.push('\n');
}

#[cfg(test)]
fn render_row(
    rendered: &mut String,
    label: &str,
    harnesses: &[PluginHarness],
    mut cell: impl FnMut(PluginHarness) -> String,
) {
    rendered.push_str(&format!("| {label} |"));
    for harness in harnesses {
        rendered.push_str(&format!(" {} |", cell(*harness)));
    }
    rendered.push('\n');
}

#[cfg(test)]
fn component_table_label(component: Component) -> &'static str {
    match component {
        Component::Skills => "Skills",
        Component::Agents => "Agents",
        Component::Hooks => "Hooks",
        Component::Mcp => "MCP",
        Component::BootstrapDocs => "Bootstrap docs",
    }
}

#[cfg(test)]
fn component_support_cell(harness: PluginHarness, component: Component) -> String {
    let profile = tree::profile(harness.tree());
    let state = support_state(harness, profile, component);
    match (component, state, harness) {
        (Component::Skills, SupportState::Supported, PluginHarness::OpenCode) => {
            "supported (registered by `index.mjs`)".to_string()
        }
        (Component::Skills, SupportState::Supported, _) => "supported".to_string(),
        (Component::Agents, SupportState::DroppedWithWarning, PluginHarness::Grok) => {
            "supported (same file as Claude)".to_string()
        }
        (Component::Agents, SupportState::DroppedWithWarning, _) => format!(
            "supported; {} dropped with warning",
            tree::claude::DROPPED_AGENT_FIELDS
                .iter()
                .map(|field| format!("`{field}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (Component::Agents, SupportState::Supported, _) => "supported".to_string(),
        (Component::Agents, SupportState::Impossible, PluginHarness::Codex) => {
            "**impossible**: Codex plugins don't load agents".to_string()
        }
        (Component::Agents, SupportState::Impossible, PluginHarness::OpenCode) => {
            "**impossible**: no API adds an agent".to_string()
        }
        (Component::Hooks, SupportState::DroppedWithWarning, PluginHarness::Codex) => {
            "as Claude (reads the Claude tree's `hooks/hooks.json`)".to_string()
        }
        (Component::Hooks, SupportState::DroppedWithWarning, PluginHarness::Grok) => {
            "as Claude".to_string()
        }
        (Component::Hooks, SupportState::DroppedWithWarning, harness) => {
            let fragment = profile.descriptor().target_dir.trim_start_matches('.');
            let requirement = if harness == PluginHarness::OpenCode {
                " that has a default `{id, setup}` export"
            } else {
                ""
            };
            format!(
                "supported for hooks with a `{fragment}` fragment{requirement}; others dropped with warning"
            )
        }
        (Component::Mcp, SupportState::DroppedWithWarning, PluginHarness::Codex) => {
            format!("dropped with warning: listed, not callable in {CODEX_PROBED_VERSION}")
        }
        (Component::Mcp, SupportState::Supported, PluginHarness::Grok) => {
            "supported (same file as Claude)".to_string()
        }
        (Component::Mcp, SupportState::Supported, PluginHarness::OpenCode) => {
            "supported (registered by `index.mjs`)".to_string()
        }
        (Component::Mcp, SupportState::Supported, _) => "supported".to_string(),
        (Component::BootstrapDocs, SupportState::Impossible, _) => "impossible".to_string(),
        _ => unreachable!("support state and component are internally consistent"),
    }
}

#[cfg(test)]
fn render_verification(verification: Verification) -> String {
    format!("{}{}", verification.level.as_str(), verification.detail)
}

#[cfg(test)]
fn evidence(harness: PluginHarness) -> &'static str {
    match harness {
        PluginHarness::Claude => "p9291, p9305, p9329, p9478",
        PluginHarness::Codex => "p9291, p9478",
        PluginHarness::Grok => "p9473, p9478, p9479",
        PluginHarness::Cursor => "p9474",
        PluginHarness::OpenCode => "p9475, p9477",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readme_tables_render_from_tree_support_and_verification_cells() {
        let rendered = render_readme_tables(&[
            PluginHarness::Claude,
            PluginHarness::Codex,
            PluginHarness::Grok,
        ]);

        assert_eq!(
            rendered,
            r#"### Component support

| | Claude | Codex | Grok |
|---|---|---|---|
| Tree | claude | claude | claude |
| Skills | supported | supported | supported |
| Agents | supported; `hooks`, `mcpServers`, `permissionMode`, `initialPrompt` dropped with warning | **impossible**: Codex plugins don't load agents | supported (same file as Claude) |
| Hooks | supported for hooks with a `claude` fragment; others dropped with warning | as Claude (reads the Claude tree's `hooks/hooks.json`) | as Claude |
| MCP | supported | dropped with warning: listed, not callable in 0.164.0 | supported (same file as Claude) |
| Bootstrap docs | impossible | impossible | impossible |

### Verification level

| | Claude | Codex | Grok |
|---|---|---|---|
| Install from catalog | live | live | live |
| Skills | live | live | live, with `resources/` (p9479) |
| Agents | live (subagent preload) | n/a (impossible) | live: subagent, native preload, skills line (p9479) |
| Hooks | live (`${CLAUDE_PLUGIN_ROOT}`) | live on a hand-built Claude-format tree (p9291); V re-runs it on the built tree | live: `SessionEnd`, `SubagentStop`, `CLAUDE_PLUGIN_ROOT` (p9479) |
| MCP | live | live: listed, not callable | docs-only (no server in any fixture) |
| Evidence | p9291, p9305, p9329, p9478 | p9291, p9478 | p9473, p9478, p9479 |
"#
        );
    }
}
