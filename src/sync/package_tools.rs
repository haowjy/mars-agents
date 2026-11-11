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
        if let Some(folded) = fold_content(&content, package_tools) {
            item.rewritten_content = Some(folded);
        }
    }
}

/// The profile text with the missing keys of `package_tools` added, or `None` when nothing was
/// added or the profile cannot take them.
fn fold_content(content: &str, package_tools: &ToolRuleMap) -> Option<String> {
    // An unreadable profile or a list-form `tools:` is reported where the profile is
    // compiled; the fold has nothing to add to it.
    let mut profile = frontmatter::parse(content).ok()?;
    let mut tools = match profile.get("tools") {
        None | Some(Value::Null) => Mapping::new(),
        Some(Value::Mapping(map)) => map.clone(),
        Some(_) => return None,
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
    if !added {
        return None;
    }
    profile.insert("tools", Value::Mapping(tools));
    Some(profile.render())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(toml: &str) -> ToolRuleMap {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn two_spellings_are_one_key_and_content_changes_only_when_a_key_is_added() {
        let profile = "---\nname: a\ntools:\n  \"bash(pkill -f *)\": allow\n---\nbody\n";

        let same = rules("\"bash(pkill -f:*)\" = \"deny\"");
        assert_eq!(fold_content(profile, &same), None);

        let more = rules("\"bash(pkill -f:*)\" = \"deny\"\nweb = \"ask\"");
        let folded = fold_content(profile, &more).unwrap();
        assert!(folded.contains("web: ask"), "{folded}");
        assert!(folded.contains("bash(pkill -f *): allow"), "{folded}");
        assert!(!folded.contains("deny"), "{folded}");
    }
}
