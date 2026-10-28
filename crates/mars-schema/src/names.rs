//! Names and relative coordinates the dependency vocabulary is built from: item names, source
//! subpaths, and rename maps. Pure string handling; separators normalize to `/` on every host.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Component, Path, PathBuf};

#[macro_export]
macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            ::serde::Serialize, ::serde::Deserialize, Hash, Eq, PartialEq, Clone, Debug, Ord, PartialOrd,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl ::std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl ::std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<$name> for String {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }
    };
}

string_newtype!(
    /// The slug an item is selected, renamed and installed by.
    ItemName
);

impl ItemName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a relative coordinate was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativePathError {
    Empty,
    Absolute,
    Escaping,
}

/// Normalize a relative path coordinate to forward-slash segments: `\` becomes `/`, `.` and
/// trailing separators drop, and an empty, rooted, drive-lettered (`C:/`) or `..` coordinate is
/// refused.
pub fn normalize_relative(raw: &str) -> Result<String, RelativePathError> {
    if raw.is_empty() {
        return Err(RelativePathError::Empty);
    }
    let normalized_separators = raw.replace('\\', "/");
    if is_windows_absolute(&normalized_separators) {
        return Err(RelativePathError::Absolute);
    }

    let mut segments = Vec::new();
    for component in Path::new(&normalized_separators).components() {
        match component {
            Component::Normal(seg) => segments.push(seg.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => return Err(RelativePathError::Escaping),
            Component::RootDir | Component::Prefix(_) => return Err(RelativePathError::Absolute),
        }
    }

    if segments.is_empty() {
        return Err(RelativePathError::Empty);
    }

    Ok(segments.join("/"))
}

fn is_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'/')
}

/// Normalized relative package coordinate under a fetched source root.
#[derive(Hash, Eq, PartialEq, Clone, Debug, Ord, PartialOrd)]
pub struct SourceSubpath(String);

impl SourceSubpath {
    pub fn new(value: impl AsRef<str>) -> Result<Self, SourceSubpathError> {
        let raw = value.as_ref();
        let normalized = normalize_relative(raw).map_err(|err| match err {
            RelativePathError::Empty => SourceSubpathError::Empty,
            RelativePathError::Absolute => SourceSubpathError::Absolute {
                input: raw.to_string(),
            },
            RelativePathError::Escaping => SourceSubpathError::Escaping {
                input: raw.to_string(),
            },
        })?;

        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
    /// Join this relative subpath under `base`, rejecting traversal attempts.
    pub fn join_under(&self, base: &Path) -> Result<PathBuf, SourceSubpathError> {
        let mut joined = base.to_path_buf();
        for component in self.as_path().components() {
            match component {
                Component::Normal(seg) => joined.push(seg),
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err(SourceSubpathError::Escaping {
                        input: self.0.clone(),
                    });
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(SourceSubpathError::Absolute {
                        input: self.0.clone(),
                    });
                }
            }
        }

        if joined.strip_prefix(base).is_err() {
            return Err(SourceSubpathError::Escaping {
                input: self.0.clone(),
            });
        }

        Ok(joined)
    }
}

impl fmt::Display for SourceSubpath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SourceSubpath {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::str::FromStr for SourceSubpath {
    type Err = SourceSubpathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for SourceSubpath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SourceSubpath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        SourceSubpath::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SourceSubpathError {
    #[error("subpath cannot be empty")]
    Empty,
    #[error("subpath must be relative, got absolute value: {input:?}")]
    Absolute { input: String },
    #[error("subpath cannot escape package root: {input:?}")]
    Escaping { input: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameRule {
    pub from: ItemName,
    pub to: ItemName,
}

/// Ordered rename rules, serialized as TOML inline table/map for compatibility.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenameMap(Vec<RenameRule>);

impl RenameMap {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn insert(&mut self, from: ItemName, to: ItemName) {
        if let Some(existing) = self.0.iter_mut().find(|r| r.from == from) {
            existing.to = to;
            return;
        }
        self.0.push(RenameRule { from, to });
    }
    pub fn get(&self, from: &str) -> Option<&ItemName> {
        self.0.iter().find(|r| r.from == from).map(|r| &r.to)
    }
}

impl Serialize for RenameMap {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for rule in &self.0 {
            map.serialize_entry(rule.from.as_str(), rule.to.as_str())?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for RenameMap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map = indexmap::IndexMap::<String, String>::deserialize(deserializer)?;
        Ok(Self(
            map.into_iter()
                .map(|(from, to)| RenameRule {
                    from: ItemName::from(from),
                    to: ItemName::from(to),
                })
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn source_subpath_normalizes_windows_and_unix_separators() {
        let subpath = SourceSubpath::new(r"plugins\foo/bar\baz").unwrap();
        assert_eq!(subpath.as_str(), "plugins/foo/bar/baz");
    }

    #[test]
    fn source_subpath_rejects_empty() {
        let err = SourceSubpath::new("").unwrap_err();
        assert_eq!(err, SourceSubpathError::Empty);
    }

    #[test]
    fn source_subpath_rejects_absolute() {
        let err = SourceSubpath::new("/abs/path").unwrap_err();
        assert!(matches!(err, SourceSubpathError::Absolute { .. }));
    }

    #[test]
    fn source_subpath_rejects_root_only() {
        let err = SourceSubpath::new("/").unwrap_err();
        assert!(matches!(err, SourceSubpathError::Absolute { .. }));
    }

    #[test]
    fn source_subpath_rejects_windows_absolute() {
        let err = SourceSubpath::new(r"C:\abs\path").unwrap_err();
        assert!(matches!(err, SourceSubpathError::Absolute { .. }));
    }

    #[test]
    fn source_subpath_rejects_escape() {
        let err = SourceSubpath::new("../escape").unwrap_err();
        assert!(matches!(err, SourceSubpathError::Escaping { .. }));
    }

    #[test]
    fn source_subpath_accepts_nested_relative_path() {
        let subpath = SourceSubpath::new("a/b/c").unwrap();
        assert_eq!(subpath.as_str(), "a/b/c");
    }

    #[test]
    fn source_subpath_serializes_with_forward_slashes() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct SubpathWrapper {
            subpath: SourceSubpath,
        }

        let wrapper = SubpathWrapper {
            subpath: SourceSubpath::new(r"plugins\foo").unwrap(),
        };
        let toml = toml::to_string(&wrapper).unwrap();
        assert!(toml.contains("subpath = \"plugins/foo\""));
    }

    #[test]
    fn source_subpath_join_under_base() {
        let base = PathBuf::from("/tmp/mars");
        let subpath = SourceSubpath::new("plugins/foo").unwrap();
        let joined = subpath.join_under(&base).unwrap();
        assert_eq!(joined, base.join("plugins").join("foo"));
    }

    #[test]
    fn source_subpath_join_under_rejects_escape_path() {
        let escaped = SourceSubpath(String::from("../escape"));
        let err = escaped.join_under(Path::new("/tmp/base")).unwrap_err();
        assert!(matches!(err, SourceSubpathError::Escaping { .. }));
    }

    #[test]
    fn source_subpath_rejects_windows_drive_forward_slash() {
        let err = SourceSubpath::new("C:/foo").unwrap_err();
        assert!(matches!(err, SourceSubpathError::Absolute { .. }));
    }

    #[test]
    fn source_subpath_rejects_current_dir_dot() {
        let err = SourceSubpath::new(".").unwrap_err();
        assert_eq!(err, SourceSubpathError::Empty);
    }

    #[test]
    fn source_subpath_rejects_mid_path_double_parent_escape() {
        let err = SourceSubpath::new("a/../../escape").unwrap_err();
        assert!(matches!(err, SourceSubpathError::Escaping { .. }));
    }

    #[test]
    fn source_subpath_rejects_harmless_parent_in_middle() {
        let err = SourceSubpath::new("a/b/../c").unwrap_err();
        assert!(matches!(err, SourceSubpathError::Escaping { .. }));
    }

    #[test]
    fn source_subpath_normalizes_trailing_slash() {
        let subpath = SourceSubpath::new("plugins/foo/").unwrap();
        assert_eq!(subpath.as_str(), "plugins/foo");
    }

    #[test]
    fn source_subpath_normalizes_leading_dot_slash() {
        let subpath = SourceSubpath::new("./plugins/foo").unwrap();
        assert_eq!(subpath.as_str(), "plugins/foo");
    }

    #[test]
    fn rename_map_toml_roundtrip_compat() {
        #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
        struct RenameWrapper {
            rename: RenameMap,
        }

        let input = r#"rename = { "coder" = "cool-coder" }"#;
        let parsed: RenameWrapper = toml::from_str(input).unwrap();
        assert_eq!(
            parsed.rename.get("coder").map(|v| v.as_str()),
            Some("cool-coder")
        );

        let serialized = toml::to_string(&parsed).unwrap();
        let reparsed: RenameWrapper = toml::from_str(&serialized).unwrap();
        assert_eq!(parsed, reparsed);
    }
}
