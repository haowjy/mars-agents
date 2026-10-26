//! `.mars/provided.lock`: the names each provider installed here.
//!
//! Machine-local and derived, like everything under `.mars/`, so nothing committed records a
//! provider, a path or a version. Readers (`mars sync`, and `mars provide`'s own unlocked fast
//! path) read it without the sync lock; every write is therefore tmp + rename, which leaves a
//! reader either the old file or the new one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use mars_schema::provider::provider_of;

use crate::error::{ConfigError, MarsError};

const PROVIDED_LOCK_FILE: &str = "provided.lock";
const PROVIDED_LOCK_VERSION: u32 = 1;

/// The digest recorded between the pre-record and the final record of an apply.
///
/// It never equals a computed digest, so the fast path cannot fire while this run may still
/// create directories, and the next run replans from the files on disk.
pub(crate) const PENDING_DIGEST: &str = "pending";

/// Every provider's record. Providers other than the one a run touches are kept verbatim.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProvidedLock {
    providers: BTreeMap<String, ProviderRecord>,
}

/// What one provider installed in this project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

/// Why a record cannot be read as this build's.
///
/// The two reasons differ in what the writer may do about them: a corrupt file can be rebuilt
/// from the files on disk, while a newer one must not be touched.
#[derive(Debug)]
pub(crate) enum Unusable {
    /// The file is not a record this build can parse: nothing in it can be trusted.
    Corrupt { path: PathBuf, reason: String },
    /// A newer Mars wrote it: reading part of another build's state is worse than reading none.
    Newer { path: PathBuf, version: i64 },
}

impl Unusable {
    /// What a reader says where it prints: the record is ignored for this run.
    pub(crate) fn reader_message(&self) -> String {
        match self {
            Self::Corrupt { path, reason } => format!(
                "{} is unusable ({reason}); ignoring it, so nothing is provided for this run",
                path.display()
            ),
            Self::Newer { path, version } => format!(
                "{} uses provided lock version {version}, which this Mars cannot read (it reads version {PROVIDED_LOCK_VERSION}); ignoring it, so nothing is provided for this run",
                path.display()
            ),
        }
    }

    /// The writer's reading: the empty record it starts from, with the warning to report, because
    /// adoption against the files on disk recovers the ownership a corrupt record can no longer
    /// prove. A newer record is refused instead: this build must not overwrite another build's.
    pub(crate) fn for_write(self) -> Result<(ProvidedLock, Option<String>), MarsError> {
        let message = self.writer_message();
        match self {
            Self::Corrupt { .. } => Ok((ProvidedLock::default(), Some(message))),
            Self::Newer { .. } => Err(MarsError::Config(ConfigError::Invalid { message })),
        }
    }

    /// What the writer says: a corrupt record is replaced, a newer one is refused.
    fn writer_message(&self) -> String {
        match self {
            Self::Corrupt { path, reason } => format!(
                "{} is unusable ({reason}); starting from an empty provided record",
                path.display()
            ),
            Self::Newer { path, version } => format!(
                "{} uses provided lock version {version}; this Mars reads version {PROVIDED_LOCK_VERSION} — upgrade Mars to read it",
                path.display()
            ),
        }
    }
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
    /// The outer error is I/O. The inner one says why the file cannot be read as this build's:
    /// a corrupt file can be rebuilt from the files on disk, a newer one must not be touched.
    pub(crate) fn load(root: &Path) -> Result<Result<ProvidedLock, Unusable>, MarsError> {
        let path = root.join(".mars").join(PROVIDED_LOCK_FILE);
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Ok(ProvidedLock::default()));
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
            Err(error) => return Ok(Err(corrupt(&path, format!("failed to parse it: {error}")))),
        };
        match wire.version {
            Some(version) if version == i64::from(PROVIDED_LOCK_VERSION) => {}
            Some(version) => return Ok(Err(Unusable::Newer { path, version })),
            None => {
                return Ok(Err(corrupt(
                    &path,
                    "it has no integer `version`".to_string(),
                )));
            }
        }
        for (provider, record) in &wire.providers {
            if let Some(name) = record.skills.iter().find(|name| !is_skill_dir_name(name)) {
                return Ok(Err(corrupt(
                    &path,
                    format!(
                        "provider `{provider}` records `{name}`, which is not a skill directory name"
                    ),
                )));
            }
            if let Some(name) = record
                .skills
                .iter()
                .find(|name| provider_of(name) != Some(provider.as_str()))
            {
                return Ok(Err(corrupt(
                    &path,
                    format!(
                        "provider `{provider}` records `{name}`, which is not one of its own `<skill>__{provider}` names"
                    ),
                )));
            }
        }
        Ok(Ok(ProvidedLock {
            providers: wire.providers,
        }))
    }

    pub(crate) fn provider(&self, provider: &str) -> Option<&ProviderRecord> {
        self.providers.get(provider)
    }

    /// Every provider's record, by provider name.
    pub(crate) fn providers(&self) -> impl Iterator<Item = (&str, &ProviderRecord)> {
        self.providers
            .iter()
            .map(|(name, record)| (name.as_str(), record))
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

fn corrupt(path: &Path, reason: String) -> Unusable {
    Unusable::Corrupt {
        path: path.to_path_buf(),
        reason,
    }
}

/// A recorded name resolves to one path component under `.mars/skills/`; a record from another
/// build must not be able to point a removal anywhere else.
fn is_skill_dir_name(name: &str) -> bool {
    use std::path::Component;
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}
