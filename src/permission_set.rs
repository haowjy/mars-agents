use serde::{Deserialize, Serialize};

/// One layer of a permission set.
///
/// Plain lists add to lower layers. `replace` clears lower entries before adding its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum PermissionSet<T> {
    Add(Vec<T>),
    Replace { replace: Vec<T> },
}

impl<'de, T> Deserialize<'de> for PermissionSet<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Replacement<T> {
            replace: Vec<T>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire<T> {
            Add(Vec<T>),
            Replace(Replacement<T>),
        }

        match Wire::deserialize(deserializer)? {
            Wire::Add(entries) => Ok(Self::Add(entries)),
            Wire::Replace(replacement) => Ok(Self::Replace {
                replace: replacement.replace,
            }),
        }
    }
}

impl<T> Default for PermissionSet<T> {
    fn default() -> Self {
        Self::Add(Vec::new())
    }
}

impl<T> From<Vec<T>> for PermissionSet<T> {
    fn from(entries: Vec<T>) -> Self {
        Self::Add(entries)
    }
}

impl<T> PermissionSet<T> {
    pub fn entries(&self) -> &[T] {
        match self {
            Self::Add(entries) | Self::Replace { replace: entries } => entries,
        }
    }

    pub fn replaces(&self) -> bool {
        matches!(self, Self::Replace { .. })
    }

    /// An empty replacement is meaningful because it clears lower layers.
    pub fn is_noop(&self) -> bool {
        matches!(self, Self::Add(entries) if entries.is_empty())
    }
}

/// Apply the permission-set rule once for every permission kind.
pub(crate) fn merge_permission_set<T: Clone + Eq>(merged: &mut Vec<T>, layer: &PermissionSet<T>) {
    if layer.replaces() {
        merged.clear();
    }
    for entry in layer.entries() {
        if !merged.contains(entry) {
            merged.push(entry.clone());
        }
    }
}
