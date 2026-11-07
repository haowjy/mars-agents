//! Collision resolution for config entries.
//!
//! Config entries resolve collisions per target root. A collision in one
//! harness target must not globally prune an item that applies cleanly to a
//! different target.

use std::collections::BTreeMap;

use crate::compiler::mcp::ParsedMcpItem;
use crate::diagnostic::DiagnosticCollector;

/// Resolve MCP name collisions for a single target root.
///
/// `items` must already be filtered for `target_root`; target selection belongs
/// to `TargetState`, while this module owns only collision precedence.
///
/// `local_source` names the consumer's own package. It wins even where graph
/// depth does not put it first: a plugin build lists its root package among its
/// own dependencies.
///
/// Precedence: local package > shallower package > earlier `decl_order`; equal
/// graph position breaks alphabetically by source package name.
pub fn resolve_mcp_collisions_for_target<'a>(
    items: impl IntoIterator<Item = &'a ParsedMcpItem>,
    target_root: &str,
    local_source: &str,
    diag: &mut DiagnosticCollector,
) -> Vec<&'a ParsedMcpItem> {
    let mut groups: BTreeMap<&str, Vec<&ParsedMcpItem>> = BTreeMap::new();
    for item in items {
        groups.entry(item.name.as_str()).or_default().push(item);
    }

    groups
        .into_values()
        .map(|group| resolve_mcp_group(group, target_root, local_source, diag))
        .collect()
}

fn resolve_mcp_group<'a>(
    mut group: Vec<&'a ParsedMcpItem>,
    target_root: &str,
    local_source: &str,
    diag: &mut DiagnosticCollector,
) -> &'a ParsedMcpItem {
    debug_assert!(!group.is_empty());
    if group.len() == 1 {
        return group[0];
    }

    group.sort_by(|a, b| {
        a.depth
            .cmp(&b.depth)
            .then_with(|| a.decl_order.cmp(&b.decl_order))
            .then_with(|| a.source_name.cmp(&b.source_name))
    });
    if let Some(index) = group
        .iter()
        .position(|item| item.source_name == local_source)
    {
        group.swap(0, index);
    }

    let winner = group[0];
    if winner.source_name == local_source
        && group
            .iter()
            .skip(1)
            .all(|loser| loser.source_name != local_source)
    {
        return winner;
    }

    for loser in group.iter().skip(1) {
        diag.warn_advisory(
            "config-entry-collision",
            format!(
                "MCP server `{}` collision in target `{target_root}`: `{}` wins over `{}`",
                winner.name, winner.source_name, loser.source_name
            ),
        );
    }
    winner
}
