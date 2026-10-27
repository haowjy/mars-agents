use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use mars_schema::names::{ItemName, RelativePathError, SourceSubpath, normalize_relative};
use mars_schema::string_newtype;

string_newtype!(SourceName);
string_newtype!(SourceUrl);
string_newtype!(CommitHash);
string_newtype!(ContentHash);

macro_rules! impl_as_str {
    ($($name:ident),+ $(,)?) => {
        $(
            impl $name {
                pub fn as_str(&self) -> &str {
                    &self.0
                }
            }
        )+
    };
}

impl_as_str!(SourceName, SourceUrl, ContentHash);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DestPathError {
    #[error("destination path cannot be empty")]
    Empty,
    #[error("destination path must be relative, got absolute value: {input:?}")]
    Absolute { input: String },
    #[error("destination path cannot escape target root: {input:?}")]
    Escaping { input: String },
}

fn is_windows_drive_relative(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Where an item came from — used for lock provenance and display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceOrigin {
    /// From a dependency (git or path source).
    Dependency(SourceName),
    /// From the local project's [package] declaration.
    LocalPackage,
}

impl fmt::Display for SourceOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dependency(name) => write!(f, "{name}"),
            Self::LocalPackage => write!(f, "_self"),
        }
    }
}

use mars_schema::item::ItemKind;

/// Stable identity for an installed item — decoupled from source URL.
///
/// Items are identified by `(kind, name)`, not by source URL.
/// If a package moves to a different git host, the item identity is preserved.
#[derive(Debug, Clone, Hash, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ItemId {
    pub kind: ItemKind,
    pub name: ItemName,
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.kind, self.name)
    }
}

/// Normalized relative path coordinate (always forward-slash).
/// Use `resolve(root)` to get a native filesystem path.
#[derive(Eq, PartialEq, Clone, Debug, Ord, PartialOrd)]
pub struct DestPath(String);

impl DestPath {
    /// Create from any string, normalizing separators and rejecting invalid paths.
    pub fn new(value: impl AsRef<str>) -> Result<Self, DestPathError> {
        let raw = value.as_ref();
        if is_windows_drive_relative(&raw.replace('\\', "/")) {
            return Err(DestPathError::Absolute {
                input: raw.to_string(),
            });
        }

        let normalized = normalize_relative(raw).map_err(|err| match err {
            RelativePathError::Empty => DestPathError::Empty,
            RelativePathError::Absolute => DestPathError::Absolute {
                input: raw.to_string(),
            },
            RelativePathError::Escaping => DestPathError::Escaping {
                input: raw.to_string(),
            },
        })?;

        Ok(Self(normalized))
    }

    /// The normalized string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Resolve to a native filesystem path under the given root.
    pub fn resolve(&self, root: &Path) -> PathBuf {
        let mut result = root.to_path_buf();
        for component in self.components() {
            result.push(component);
        }
        result
    }

    /// Split into path components (by forward slash).
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// Extract the installed item name from this path.
    /// Agents strip a trailing `.md`; bootstrap docs use their containing directory
    /// because their canonical path is `bootstrap/<name>/BOOTSTRAP.md`; all other
    /// directory-based kinds use the leaf name.
    pub fn item_name(&self, kind: ItemKind) -> String {
        match kind {
            ItemKind::BootstrapDoc => self
                .0
                .strip_suffix("/BOOTSTRAP.md")
                .and_then(|path| path.rsplit('/').next())
                .unwrap_or_else(|| self.0.rsplit('/').next().unwrap_or(""))
                .to_string(),
            _ => {
                let last = self.0.rsplit('/').next().unwrap_or("");
                match kind {
                    ItemKind::Agent => last.strip_suffix(".md").unwrap_or(last).to_string(),
                    ItemKind::Skill | ItemKind::Hook | ItemKind::McpServer => last.to_string(),
                    ItemKind::BootstrapDoc => unreachable!("handled above"),
                }
            }
        }
    }
}

impl From<&str> for DestPath {
    fn from(value: &str) -> Self {
        Self::new(value).expect("invalid destination path")
    }
}

impl From<String> for DestPath {
    fn from(value: String) -> Self {
        Self::new(value).expect("invalid destination path")
    }
}

impl AsRef<str> for DestPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for DestPath {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl Hash for DestPath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Display for DestPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for DestPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DestPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        DestPath::new(value).map_err(serde::de::Error::custom)
    }
}

/// Resolved context for a mars command.
#[derive(Debug, Clone)]
pub struct MarsContext {
    /// Project root containing mars.toml and mars.lock.
    pub project_root: PathBuf,
    /// Whether mars is running under Meridian management.
    ///
    /// Captured at context construction time so compilation decisions are stable
    /// for the duration of a sync.
    pub meridian_managed: bool,
}

#[cfg(test)]
impl MarsContext {
    /// Create a MarsContext for tests without any validation.
    pub fn for_test(project_root: PathBuf) -> Self {
        MarsContext {
            project_root,
            meridian_managed: meridian_managed_from_env(),
        }
    }
}

pub fn meridian_managed_from_env() -> bool {
    std::env::var("MERIDIAN_MANAGED").is_ok_and(|value| value == "1")
}

/// Return a command reference string suitable for the current runtime mode.
/// When `MERIDIAN_MANAGED=1` is set (i.e. invoked through `meridian`), prefix
/// the command with `meridian ` so that user-facing hints read correctly.
pub fn managed_cmd(cmd: &str) -> std::borrow::Cow<'_, str> {
    if meridian_managed_from_env() {
        format!("meridian {cmd}").into()
    } else {
        cmd.into()
    }
}

/// Stable source identity used for resolver deduplication.
///
/// `plugin` selects one plugin of a marketplace at the source: two plugins of one marketplace
/// are two packages, so the selector is part of the identity.
#[derive(Hash, Eq, PartialEq, Clone, Debug, Ord, PartialOrd, Serialize, Deserialize)]
pub enum SourceId {
    Git {
        url: SourceUrl,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subpath: Option<SourceSubpath>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plugin: Option<String>,
    },
    Path {
        canonical: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subpath: Option<SourceSubpath>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plugin: Option<String>,
    },
}

impl SourceId {
    pub fn git_with_subpath(url: SourceUrl, subpath: Option<SourceSubpath>) -> Self {
        Self::Git {
            url,
            subpath,
            plugin: None,
        }
    }
    pub fn path_with_subpath(
        base: &Path,
        relative_or_absolute: &Path,
        subpath: Option<SourceSubpath>,
    ) -> std::io::Result<Self> {
        let candidate = if relative_or_absolute.is_absolute() {
            relative_or_absolute.to_path_buf()
        } else {
            base.join(relative_or_absolute)
        };
        let canonical = dunce::canonicalize(&candidate)?;
        Ok(Self::Path {
            canonical,
            subpath,
            plugin: None,
        })
    }

    pub fn with_plugin(mut self, selector: Option<String>) -> Self {
        match &mut self {
            Self::Git { plugin, .. } | Self::Path { plugin, .. } => *plugin = selector,
        }
        self
    }

    pub fn plugin(&self) -> Option<&str> {
        match self {
            Self::Git { plugin, .. } | Self::Path { plugin, .. } => plugin.as_deref(),
        }
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (subpath, plugin) = match self {
            Self::Git {
                url,
                subpath,
                plugin,
            } => {
                write!(f, "git:{url}")?;
                (subpath, plugin)
            }
            Self::Path {
                canonical,
                subpath,
                plugin,
            } => {
                write!(f, "path:{}", canonical.display())?;
                (subpath, plugin)
            }
        };
        if let Some(subpath) = subpath {
            write!(f, "@{subpath}")?;
        }
        if let Some(plugin) = plugin {
            write!(f, " (plugin {plugin})")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Wrapper<T> {
        value: T,
    }

    #[test]
    fn dest_path_roundtrip() {
        let v = Wrapper {
            value: DestPath::from("agents/coder.md"),
        };
        let s = toml::to_string(&v).unwrap();
        let out: Wrapper<DestPath> = toml::from_str(&s).unwrap();
        assert_eq!(v, out);
    }

    #[test]
    fn source_subpath_and_dest_path_share_normalization_rules() {
        let raw = r"./plugins\foo/bar\";
        let subpath = SourceSubpath::new(raw).unwrap();
        let dest = DestPath::new(raw).unwrap();

        assert_eq!(subpath.as_str(), "plugins/foo/bar");
        assert_eq!(dest.as_str(), "plugins/foo/bar");
        assert_eq!(subpath.as_str(), dest.as_str());
    }

    // Edge case 7: Windows drive letter with forward slash (C:/foo)

    // Edge case 9: "." alone — CurDir is skipped → segments empty → Empty error

    #[test]
    fn dest_path_normalizes_windows_and_unix_separators() {
        let path = DestPath::new(r"agents\foo/bar\baz.md").unwrap();
        assert_eq!(path.as_str(), "agents/foo/bar/baz.md");
    }

    #[test]
    fn dest_path_rejects_empty() {
        let err = DestPath::new("").unwrap_err();
        assert_eq!(err, DestPathError::Empty);
    }

    #[test]
    fn dest_path_rejects_absolute() {
        let err = DestPath::new("/abs/path").unwrap_err();
        assert!(matches!(err, DestPathError::Absolute { .. }));
    }

    #[test]
    fn dest_path_rejects_root_only() {
        let err = DestPath::new("/").unwrap_err();
        assert!(matches!(err, DestPathError::Absolute { .. }));
    }

    #[test]
    fn dest_path_rejects_windows_absolute() {
        let err = DestPath::new(r"C:\abs\path").unwrap_err();
        assert!(matches!(err, DestPathError::Absolute { .. }));
    }

    #[test]
    fn dest_path_rejects_windows_drive_relative() {
        let err = DestPath::new("C:relative").unwrap_err();
        assert!(matches!(err, DestPathError::Absolute { .. }));
    }

    #[test]
    fn dest_path_rejects_escape() {
        let err = DestPath::new("../escape").unwrap_err();
        assert!(matches!(err, DestPathError::Escaping { .. }));
    }

    #[test]
    fn dest_path_normalizes_trailing_slash() {
        let path = DestPath::new("skills/planning/").unwrap();
        assert_eq!(path.as_str(), "skills/planning");
    }

    #[test]
    fn dest_path_normalizes_leading_dot_slash() {
        let path = DestPath::new("./skills/planning").unwrap();
        assert_eq!(path.as_str(), "skills/planning");
    }

    #[test]
    fn dest_path_item_name_extracts_agent_leaf() {
        let path = DestPath::new("agents/coder.md").unwrap();
        assert_eq!(path.item_name(ItemKind::Agent), "coder");
    }

    #[test]
    fn dest_path_item_name_extracts_skill_leaf() {
        let path = DestPath::new("skills/planning").unwrap();
        assert_eq!(path.item_name(ItemKind::Skill), "planning");
    }

    #[test]
    fn dest_path_item_name_extracts_bootstrap_doc_container() {
        let path = DestPath::new("bootstrap/global-auth/BOOTSTRAP.md").unwrap();
        assert_eq!(path.item_name(ItemKind::BootstrapDoc), "global-auth");
    }

    #[test]
    fn dest_path_item_name_extracts_nested_agent_leaf() {
        let path = DestPath::new("agents/sub/deep.md").unwrap();
        assert_eq!(path.item_name(ItemKind::Agent), "deep");
    }

    #[test]
    fn dest_path_item_name_handles_no_slash_edge_case() {
        let path = DestPath::new("solo.md").unwrap();
        assert_eq!(path.item_name(ItemKind::Agent), "solo");
    }

    // Edge case 11: mid-path parent escape "a/../../escape" — hits ParentDir immediately after
    // pushing "a", so it is rejected as Escaping (conservative: any ".." rejected)

    // Edge case 12: "a/b/../c" — conservative policy: any ".." is rejected as Escaping,
    // even when logically harmless. This documents and pins the chosen policy.

    // Edge case 13: trailing slash normalizes (no trailing slash in canonical form)

    // Edge case 14: leading "./" normalizes to the bare path

    #[test]
    fn config_roundtrip_preserves_subpath() {
        let config = r#"
[dependencies.base]
url = "https://github.com/org/base.git"
subpath = "plugins\\foo"
"#;
        let parsed: crate::config::Config = toml::from_str(config).unwrap();
        assert_eq!(
            parsed.dependencies["base"]
                .subpath
                .as_ref()
                .map(SourceSubpath::as_str),
            Some("plugins/foo")
        );

        let serialized = toml::to_string(&parsed).unwrap();
        assert!(serialized.contains("subpath = \"plugins/foo\""));
        let reparsed: crate::config::Config = toml::from_str(&serialized).unwrap();
        assert_eq!(
            reparsed.dependencies["base"]
                .subpath
                .as_ref()
                .map(SourceSubpath::as_str),
            Some("plugins/foo")
        );
    }

    // ========== RES-001: EffectiveDependency carries subpath after merge ==========

    /// RES-001 (config side): after merge_with_root the EffectiveDependency.subpath
    /// matches what was in the Config.  This confirms the subpath survives the
    /// config-load → merge step.
    #[test]
    fn effective_dependency_subpath_preserved_through_merge() {
        use crate::config::{Config, merge_with_root};

        let toml_str = r#"
[dependencies.dep]
url = "https://github.com/org/repo.git"
subpath = "plugins/foo"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let (effective, _) = merge_with_root(
            config,
            crate::config::LocalConfig::default(),
            std::path::Path::new("."),
        )
        .unwrap();
        assert_eq!(
            effective.dependencies["dep"]
                .subpath
                .as_ref()
                .map(SourceSubpath::as_str),
            Some("plugins/foo")
        );
        // SourceId must embed the same subpath
        assert!(matches!(
            &effective.dependencies["dep"].source_id,
            SourceId::Git { subpath: Some(sp), .. } if sp.as_str() == "plugins/foo"
        ));
    }
}
