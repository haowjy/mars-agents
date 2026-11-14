//! `[package.tools]` and the Agents the package ships.
//!
//! The package's rules act as if written in each shipped Agent's `tools:` map, except for a
//! key the Agent's own map already names. `mars sync` writes the missing keys into the
//! installed file; `prepare` adds them to the Agent's reading. Both ask [`missing`], so the
//! precedence is one rule.

use serde_yaml::Value;

use crate::frontmatter::Frontmatter;
use crate::tool_names::OverlayToolName;
use crate::tool_policy::ToolAction;
use crate::tool_rule_map::ToolRuleMap;

/// The entries of `package_tools` that `profile`'s own `tools:` does not name, in the package's
/// order. Keys compare by their parsed form, so `bash(git push:*)` and `bash(git push *)` are
/// one key. Empty for a list-form `tools:`, which is reported where the profile is read.
pub fn missing<'a>(
    profile: &Frontmatter,
    package_tools: &'a ToolRuleMap,
) -> Vec<(&'a OverlayToolName, ToolAction)> {
    let own: Vec<OverlayToolName> = match profile.get("tools") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Mapping(map)) => map
            .keys()
            .filter_map(|key| key.as_str())
            .filter_map(|key| OverlayToolName::try_from(key.to_owned()).ok())
            .collect(),
        Some(_) => return Vec::new(),
    };
    package_tools
        .iter()
        .filter(|(name, _)| !own.iter().any(|own| own.parsed() == name.parsed()))
        .collect()
}
