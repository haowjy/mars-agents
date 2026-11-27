use std::path::{Path, PathBuf};

use crate::error::{ConfigError, MarsError};

use super::LocalConfig;
use mars_schema::diagnostic::{SchemaDiagnostic, Severity};

/// The user's `config.toml`: the `mars.local.toml` schema, layered beneath `mars.toml`.
#[derive(Debug, Clone, Default)]
pub struct UserConfig {
    /// Names the file in warnings; empty when no user file exists, and then nothing warns.
    pub path: PathBuf,
    pub local: LocalConfig,
    /// Where the file should be, when it was read from the old macOS folder instead.
    pub moved_to: Option<PathBuf>,
}

pub(super) fn load() -> Result<UserConfig, MarsError> {
    let (path, moved_to) =
        crate::platform::config::user_file("config.toml").ok_or_else(|| ConfigError::Invalid {
            message: "cannot determine config directory; set MARS_CONFIG_DIR".to_string(),
        })?;
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UserConfig::default());
        }
        Err(source) => return Err(super::read_error(&path, source)),
    };
    let mut local = super::parse_local(&content, &path, mars_schema::keys::Document::Consumer)?;
    // A user-wide `targets` or override would apply to every project unseen, and ignoring either
    // loses no restriction: this layer does not read them, like any other key it has no use for.
    let unread = [
        (
            "settings",
            "targets",
            local.settings.targets.take().is_some(),
        ),
        (
            "settings",
            "managed_root",
            local.settings.managed_root.take().is_some(),
        ),
        (
            "",
            "overrides",
            !std::mem::take(&mut local.overrides).is_empty(),
        ),
    ];
    for (table, key, present) in unread {
        if present {
            local.unknown_keys.push(not_read_here(table, key));
        }
    }
    Ok(UserConfig {
        path,
        local,
        moved_to,
    })
}

/// The warning for a user file read from the old macOS folder (`platform::config::user_file`).
/// The user loses nothing yet, but a later Mars stops reading the old path.
pub(crate) fn moved_warning(read: &Path, moved_to: &Path) -> crate::diagnostic::Diagnostic {
    crate::diagnostic::Diagnostic::warning(
        "user-config-moved",
        format!(
            "read {}, where Mars no longer looks for user config; move it to {} (a later Mars stops reading the old path)",
            read.display(),
            moved_to.display()
        ),
    )
}

/// A real key of another config file that this one leaves unread. It is not a typo or a newer
/// key, so the message does not guess at why it is here.
fn not_read_here(table: &str, key: &str) -> SchemaDiagnostic {
    let table = if table.is_empty() {
        String::new()
    } else {
        format!("[{table}] ")
    };
    SchemaDiagnostic::new(
        "field.unknown",
        Severity::Info,
        Vec::new(),
        format!("{table}`{key}` is not read in the user config; it stays in the file"),
    )
}
