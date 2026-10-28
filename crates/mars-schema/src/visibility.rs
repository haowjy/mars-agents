//! Whether an effectful package item (a hook or an MCP server) crosses a package boundary.

/// `local` (the default) stays with the package that declares it; `exported` reaches its
/// dependents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Local,
    Exported,
}

impl Visibility {
    /// The authored spelling, or `None` for anything else; the caller decides how loud that is.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "local" => Some(Self::Local),
            "exported" => Some(Self::Exported),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Exported => "exported",
        }
    }
}
