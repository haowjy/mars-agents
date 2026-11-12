//! An ordered map from tool key to action: `[settings.tools]` and the rule entries of
//! `[agents.<name>.tools]` in `mars.toml`.

use std::fmt;

use serde::de::{Error as _, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::tool_names::{OverlayToolName, ParsedToolName};
use crate::tool_policy::ToolAction;

/// Tool rules keyed by tool name, in the order the author wrote them.
///
/// Two spellings of one key (`Bash` beside `bash`, or `bash(git push:*)` beside
/// `bash(git push *)`) are one key: a table that holds both fails to load, since file order
/// would otherwise pick the winner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolRuleMap {
    entries: Vec<(OverlayToolName, ToolAction)>,
}

impl ToolRuleMap {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&OverlayToolName, ToolAction)> {
        self.entries.iter().map(|(name, action)| (name, *action))
    }

    /// The message that refuses a `*` rule in `table`, where it was written: `*` is one agent's
    /// default, so only a table that speaks for one agent may set it. `reason` says why this
    /// table cannot.
    pub fn wildcard_refusal(&self, table: &str, reason: &str) -> Option<String> {
        let (name, action) = self
            .iter()
            .find(|(name, _)| name.parsed().capability == crate::tool_names::Capability::Any)?;
        Some(format!(
            "`{table}` `{}` = \"{}\": `*` is not accepted: {reason}",
            String::from(name.clone()),
            action.as_str()
        ))
    }

    pub fn contains_key(&self, key: &ParsedToolName) -> bool {
        self.entries.iter().any(|(name, _)| name.parsed() == key)
    }

    /// `lower` with every key that `higher` names replaced by `higher`'s entry, then the rest
    /// of `higher`. Keys compare by their parsed form, and `higher`'s spelling is the one kept.
    pub fn merge_by_key(lower: &Self, higher: &Self) -> Self {
        let mut entries: Vec<_> = lower
            .entries
            .iter()
            .filter(|(name, _)| !higher.contains_key(name.parsed()))
            .cloned()
            .collect();
        entries.extend(higher.entries.iter().cloned());
        Self { entries }
    }
}

impl Serialize for ToolRuleMap {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for (name, action) in &self.entries {
            map.serialize_entry(&String::from(name.clone()), action)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ToolRuleMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor;

        impl<'de> Visitor<'de> for MapVisitor {
            type Value = ToolRuleMap;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a table of tool name to allow, deny or ask")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries: Vec<(OverlayToolName, ToolAction)> = Vec::new();
                while let Some(raw) = map.next_key::<String>()? {
                    let action = map.next_value::<ToolAction>()?;
                    let name = OverlayToolName::try_from(raw).map_err(A::Error::custom)?;
                    if let Some((earlier, _)) = entries
                        .iter()
                        .find(|(earlier, _)| earlier.parsed() == name.parsed())
                    {
                        return Err(A::Error::custom(format!(
                            "`{}` and `{}` name the same tool",
                            String::from(earlier.clone()),
                            String::from(name)
                        )));
                    }
                    entries.push((name, action));
                }
                Ok(ToolRuleMap { entries })
            }
        }

        deserializer.deserialize_map(MapVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> Result<ToolRuleMap, toml::de::Error> {
        toml::from_str(toml)
    }

    fn keys(map: &ToolRuleMap) -> Vec<(String, &'static str)> {
        map.iter()
            .map(|(name, action)| (String::from(name.clone()), action.as_str()))
            .collect()
    }

    #[test]
    fn reads_in_file_order_and_keeps_the_authored_spelling() {
        let map =
            parse("zeta = \"deny\"\n\"Bash(git push:*)\" = \"ask\"\nalpha = \"allow\"\n").unwrap();
        assert_eq!(
            keys(&map),
            [
                ("zeta".into(), "deny"),
                ("Bash(git push:*)".into(), "ask"),
                ("alpha".into(), "allow"),
            ]
        );
        let saved = toml::to_string(&map).unwrap();
        assert_eq!(parse(&saved).unwrap(), map);
        assert!(saved.contains("\"Bash(git push:*)\""));
    }

    #[test]
    fn two_spellings_of_one_key_fail_naming_both() {
        for (a, b) in [
            ("Bash", "bash"),
            ("shell", "bash"),
            ("bash(git push:*)", "bash(git push *)"),
        ] {
            let err = parse(&format!("\"{a}\" = \"deny\"\n\"{b}\" = \"allow\"\n"))
                .unwrap_err()
                .to_string();
            assert!(err.contains(&format!("`{a}`")), "{err}");
            assert!(err.contains(&format!("`{b}`")), "{err}");
        }
    }

    #[test]
    fn rejects_a_malformed_name_and_an_unknown_action() {
        assert!(parse("\"bash(\" = \"deny\"\n").is_err());
        assert!(parse("mcp = \"deny\"\n").is_err());
        assert!(parse("bash = \"block\"\n").is_err());
        assert!(parse("bash = 1\n").is_err());
    }

    #[test]
    fn merge_replaces_by_parsed_key() {
        let lower =
            parse("Bash = \"allow\"\nread = \"deny\"\n\"bash(git push:*)\" = \"allow\"\n").unwrap();
        let higher =
            parse("shell = \"deny\"\n\"bash(git push *)\" = \"deny\"\nedit = \"ask\"\n").unwrap();
        let merged = ToolRuleMap::merge_by_key(&lower, &higher);
        assert_eq!(
            keys(&merged),
            [
                ("read".into(), "deny"),
                ("shell".into(), "deny"),
                ("bash(git push *)".into(), "deny"),
                ("edit".into(), "ask"),
            ]
        );
        assert_eq!(
            ToolRuleMap::merge_by_key(&lower, &ToolRuleMap::default()),
            lower
        );
    }
}
