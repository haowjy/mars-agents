//! `.mars/provided.lock`: the names each provider installed here.
//!
//! Machine-local and derived, like everything under `.mars/`, so nothing committed records a
//! provider, a path or a version. Readers (`mars sync`, and `mars provide`'s own unlocked fast
//! path) read it without the sync lock; every write is therefore tmp + rename, which leaves a
//! reader either the old file or the new one.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, MarsError};

const PROVIDED_LOCK_FILE: &str = "provided.lock";
const PROVIDED_LOCK_VERSION: u32 = 1;

/// The digest recorded between the pre-record and the final record of an apply.
///
/// It never equals a computed digest, so the fast path cannot fire while this run may still
/// create directories, and the next run replans from the files on disk.
pub(crate) const PENDING_DIGEST: &str = "pending";

/// Every provider's record. Providers other than the one a run touches are kept verbatim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProvidedLock {
    providers: BTreeMap<String, ProviderRecord>,
}

/// What one provider installed in this project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProviderRecord {
    /// The host's version, opaque: host versions need not be semver (Meridian RCs are PEP 440).
    /// Recorded only for display and change detection.
    pub version: String,
    /// The digest of the provided tree as last applied, or [`PENDING_DIGEST`].
    pub digest: String,
    /// The names this provider installed. A name a package owns, or that collided with a file
    /// Mars did not create, is absent.
    #[serde(default)]
    pub skills: Vec<String>,
}

/// A loaded record, with anything the file got wrong that the caller should report.
pub(crate) struct LoadedProvidedLock {
    pub record: ProvidedLock,
    /// The file is unusable: the record is empty and the next write replaces the file.
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
struct WireProvidedLock {
    version: Option<i64>,
    #[serde(default)]
    providers: BTreeMap<String, ProviderRecord>,
}

#[derive(Serialize)]
struct OutgoingProvidedLock<'a> {
    version: u32,
    providers: &'a BTreeMap<String, ProviderRecord>,
}

impl ProvidedLock {
    /// Read the project's record.
    ///
    /// A corrupt file is a warning and an empty record: its contents cannot be trusted, and
    /// adoption against the files on disk recovers the ownership it can still prove. An
    /// unknown version is an error, because a newer Mars wrote it and refusing is the only
    /// answer that cannot lose that build's state.
    pub(crate) fn load(root: &Path) -> Result<LoadedProvidedLock, MarsError> {
        let path = root.join(".mars").join(PROVIDED_LOCK_FILE);
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LoadedProvidedLock {
                    record: ProvidedLock::default(),
                    warnings: Vec::new(),
                });
            }
            Err(source) => {
                return Err(MarsError::Io {
                    operation: "read provided lock".to_string(),
                    path,
                    source,
                });
            }
        };
        let wire: WireProvidedLock = match toml::from_str(&content) {
            Ok(wire) => wire,
            Err(error) => return Ok(corrupt(&path, format!("failed to parse it: {error}"))),
        };
        match wire.version {
            Some(version) if version == i64::from(PROVIDED_LOCK_VERSION) => {}
            Some(version) => {
                return Err(MarsError::Config(ConfigError::Invalid {
                    message: format!(
                        "{} uses provided lock version {version}; this Mars reads version {PROVIDED_LOCK_VERSION} — upgrade Mars to read it",
                        path.display()
                    ),
                }));
            }
            None => return Ok(corrupt(&path, "it has no integer `version`".to_string())),
        }
        for (provider, record) in &wire.providers {
            if let Some(name) = record.skills.iter().find(|name| !is_skill_dir_name(name)) {
                return Ok(corrupt(
                    &path,
                    format!(
                        "provider `{provider}` records `{name}`, which is not a skill directory name"
                    ),
                ));
            }
        }
        Ok(LoadedProvidedLock {
            record: ProvidedLock {
                providers: wire.providers,
            },
            warnings: Vec::new(),
        })
    }

    pub(crate) fn provider(&self, provider: &str) -> Option<&ProviderRecord> {
        self.providers.get(provider)
    }

    /// Replace one provider's record, keeping every other provider's.
    pub(crate) fn set_provider(&mut self, provider: &str, record: ProviderRecord) {
        self.providers.insert(provider.to_string(), record);
    }

    /// Write the record atomically. Missing `.mars/` is created as part of the write.
    pub(crate) fn save(&self, root: &Path) -> Result<(), MarsError> {
        let path = root.join(".mars").join(PROVIDED_LOCK_FILE);
        let content = toml::to_string_pretty(&OutgoingProvidedLock {
            version: PROVIDED_LOCK_VERSION,
            providers: &self.providers,
        })
        .map_err(|error| MarsError::Internal(format!("provided lock serialize: {error}")))?;
        crate::fs::atomic_write(&path, content.as_bytes())
    }
}

fn corrupt(path: &Path, reason: String) -> LoadedProvidedLock {
    LoadedProvidedLock {
        record: ProvidedLock::default(),
        warnings: vec![format!(
            "{} is unusable ({reason}); starting from an empty provided record",
            path.display()
        )],
    }
}

/// A recorded name resolves to one path component under `.mars/skills/`; a record from another
/// build must not be able to point a removal anywhere else.
fn is_skill_dir_name(name: &str) -> bool {
    use std::path::Component;
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}
