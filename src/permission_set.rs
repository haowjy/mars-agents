use std::fmt;
use std::marker::PhantomData;

use serde::de::{MapAccess, SeqAccess, Visitor};
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
        struct PermissionSetVisitor<T>(PhantomData<T>);

        impl<'de, T> Visitor<'de> for PermissionSetVisitor<T>
        where
            T: Deserialize<'de>,
        {
            type Value = PermissionSet<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a list or a table containing only `replace = [...]`")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut entries = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
                while let Some(entry) = sequence.next_element()? {
                    entries.push(entry);
                }
                Ok(PermissionSet::Add(entries))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let Some(key) = map.next_key::<String>()? else {
                    return Err(serde::de::Error::missing_field("replace"));
                };
                if key != "replace" {
                    return Err(serde::de::Error::unknown_field(&key, &["replace"]));
                }
                let entries = map.next_value::<Vec<T>>()?;
                if let Some(extra) = map.next_key::<String>()? {
                    return Err(serde::de::Error::unknown_field(&extra, &["replace"]));
                }
                Ok(PermissionSet::Replace { replace: entries })
            }
        }

        deserializer.deserialize_any(PermissionSetVisitor(PhantomData))
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

/// Compose adjacent authored layers while preserving whether they clear still-lower layers.
pub(crate) fn compose_permission_sets<T: Clone + Eq>(
    lower: &PermissionSet<T>,
    higher: &PermissionSet<T>,
) -> PermissionSet<T> {
    let mut entries = lower.entries().to_vec();
    merge_permission_set(&mut entries, higher);
    if lower.replaces() || higher.replaces() {
        PermissionSet::Replace { replace: entries }
    } else {
        PermissionSet::Add(entries)
    }
}
