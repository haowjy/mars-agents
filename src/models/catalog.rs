//! The snapshot's rows, held per provider and decoded when a provider is first read.
//!
//! The snapshot holds every models.dev provider (about 9,900 rows), and a command reads two
//! slices of it: the project's `catalog_providers` and the one provider a model's reasoning is
//! joined from. Rows stay raw JSON until one of those slices asks for them.

use mars_schema::models::{CachedModel, catalog_provider_canonical};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::value::RawValue;
use std::sync::{Arc, OnceLock};

#[derive(Debug)]
struct Provider {
    key: String,
    /// What the file holds; `None` for rows that were never raw (a fetch, a legacy seed).
    raw: Option<Box<RawValue>>,
    rows: OnceLock<Vec<CachedModel>>,
}

impl Provider {
    fn rows(&self) -> &[CachedModel] {
        self.rows.get_or_init(|| {
            let raw = self.raw.as_ref().expect("a provider without rows is raw");
            // Reached for a provider [`Catalog::undecodable`] did not vouch for; the cache
            // load vets every provider a command reads, so this is the rest, read as empty.
            serde_json::from_str(raw.get()).unwrap_or_default()
        })
    }

    fn decodes(&self) -> bool {
        match (&self.raw, self.rows.get()) {
            (_, Some(_)) | (None, _) => true,
            (Some(raw), None) => match serde_json::from_str::<Vec<CachedModel>>(raw.get()) {
                Ok(rows) => {
                    let _ = self.rows.set(rows);
                    true
                }
                Err(_) => false,
            },
        }
    }
}

/// Every provider's rows, in snapshot order. Cloning shares them.
#[derive(Debug, Clone, Default)]
pub struct Catalog(Arc<Vec<Provider>>);

impl Catalog {
    /// Groups `rows` by `provider_key`, in the order each key first appears.
    pub fn from_rows(rows: Vec<CachedModel>) -> Self {
        let mut providers: Vec<(String, Vec<CachedModel>)> = Vec::new();
        for row in rows {
            match providers
                .iter_mut()
                .find(|(key, _)| *key == row.provider_key)
            {
                Some((_, group)) => group.push(row),
                None => providers.push((row.provider_key.clone(), vec![row])),
            }
        }
        Self(Arc::new(
            providers
                .into_iter()
                .map(|(key, rows)| Provider {
                    key,
                    raw: None,
                    rows: OnceLock::from(rows),
                })
                .collect(),
        ))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The key of the first provider `needed` accepts whose rows do not decode. The snapshot is
    /// written from typed rows, so this is a file another build wrote under the same name.
    pub fn undecodable(&self, needed: impl Fn(&str) -> bool) -> Option<&str> {
        self.0
            .iter()
            .filter(|provider| needed(&provider.key))
            .find(|provider| !provider.decodes())
            .map(|provider| provider.key.as_str())
    }

    /// The rows of every provider whose key `allowed` accepts.
    pub fn rows_where(&self, allowed: impl Fn(&str) -> bool) -> Vec<CachedModel> {
        self.0
            .iter()
            .filter(|provider| allowed(&provider.key))
            .flat_map(|provider| provider.rows().iter().cloned())
            .collect()
    }

    /// The rows of the provider `canonical` names, as [`catalog_provider_canonical`] spells it.
    pub fn provider_rows(&self, canonical: &str) -> impl Iterator<Item = &CachedModel> {
        let canonical = canonical.to_string();
        self.0
            .iter()
            .filter(move |provider| catalog_provider_canonical(&provider.key) == canonical)
            .flat_map(|provider| provider.rows())
    }
}

impl Serialize for Catalog {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Stored<'a> {
            key: &'a str,
            models: &'a [CachedModel],
        }
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for provider in self.0.iter() {
            seq.serialize_element(&Stored {
                key: &provider.key,
                models: provider.rows(),
            })?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Catalog {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stored {
            key: String,
            models: Box<RawValue>,
        }
        let stored = Vec::<Stored>::deserialize(deserializer)?;
        Ok(Self(Arc::new(
            stored
                .into_iter()
                .map(|stored| Provider {
                    key: stored.key,
                    raw: Some(stored.models),
                    rows: OnceLock::new(),
                })
                .collect(),
        )))
    }
}
