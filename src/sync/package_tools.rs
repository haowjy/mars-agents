//! `[package.tools]` folded into the agents the package ships.
//!
//! The installed `.mars/agents/<name>.md` carries the rules in its own `tools:` map, so every
//! reader of installed agents (native files, the launch bundle, `mars agents show`, the plugin
//! tree) sees ordinary profile rules and none of them knows the key exists.

use mars_schema::frontmatter;
use mars_schema::item::ItemKind;
use mars_schema::tool_names::OverlayToolName;
use mars_schema::tool_rule_map::ToolRuleMap;
use serde_yaml::{Mapping, Value};

use crate::resolve::ResolvedGraph;
use crate::sync::target::TargetState;

/// Add each key of an agent's source package `[package.tools]` that the profile's own `tools:`
/// does not name. Keys compare by their parsed form, so `bash(git push:*)` and
/// `bash(git push *)` are one key. `rewritten_content` is set only when a key was added.
///
/// A dependency's map comes from its resolved manifest; `local_tools` is the project's own
/// `[package]`, for the items whose source is not in the graph.
pub(crate) fn fold(target: &mut TargetState, graph: &ResolvedGraph, local_tools: &ToolRuleMap) {
    for item in target.items.values_mut() {
        if item.id.kind != ItemKind::Agent {
            continue;
        }
        let package_tools = match graph.nodes.get(&item.source_name) {
            Some(node) => node
                .manifest
                .as_ref()
                .map(|manifest| &manifest.package.tools),
            None => Some(local_tools),
        };
        let Some(package_tools) = package_tools.filter(|tools| !tools.is_empty()) else {
            continue;
        };
        let content = match &item.rewritten_content {
            Some(content) => content.clone(),
            None => match std::fs::read_to_string(&item.source_path) {
                Ok(content) => content,
                Err(_) => continue,
            },
        };
        // An unreadable profile or a list-form `tools:` is reported where the profile is
        // compiled; the fold has nothing to add to it.
        let Ok(mut profile) = frontmatter::parse(&content) else {
            continue;
        };
        let mut tools = match profile.get("tools") {
            None | Some(Value::Null) => Mapping::new(),
            Some(Value::Mapping(map)) => map.clone(),
            Some(_) => continue,
        };
        let own: Vec<OverlayToolName> = tools
            .keys()
            .filter_map(|key| key.as_str())
            .filter_map(|key| OverlayToolName::try_from(key.to_owned()).ok())
            .collect();
        let mut added = false;
        for (name, action) in package_tools.iter() {
            if own.iter().any(|own| own.parsed() == name.parsed()) {
                continue;
            }
            tools.insert(
                Value::String(String::from(name.clone())),
                Value::String(action.as_str().to_owned()),
            );
            added = true;
        }
        if added {
            profile.insert("tools", Value::Mapping(tools));
            item.rewritten_content = Some(profile.render());
        }
    }
}
