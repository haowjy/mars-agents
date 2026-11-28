use std::path::PathBuf;

use crate::diagnostic::{Diagnostic, DiagnosticLevel};
use crate::error::MarsError;
use crate::platform::home::{self, OldCopy, UserFile};

use super::LocalConfig;
use mars_schema::diagnostic::{SchemaDiagnostic, Severity};

/// The user's `config.toml`: the `mars.local.toml` schema, layered beneath `mars.toml`.
#[derive(Debug, Clone, Default)]
pub struct UserConfig {
    /// Names the file in warnings; empty when no user file exists, and then nothing warns.
    pub path: PathBuf,
    pub local: LocalConfig,
    /// What the user should hear about where the file and the user folder are: the Mars 0.40
    /// copy, `MARS_CONFIG_DIR`, or no folder at all ([`folder_notices`]).
    pub notices: Vec<Diagnostic>,
}

pub(super) fn load() -> Result<UserConfig, MarsError> {
    // Only `--user` needs the folder (`MarsContext::user` fails without it); a project command
    // runs with no user layer, and a lookup outside a project runs in place.
    let Some(file) = crate::platform::home::user_file("config.toml") else {
        return Ok(UserConfig {
            notices: folder_notices(None),
            ..UserConfig::default()
        });
    };
    let notices = folder_notices(Some(&file));
    let path = file.path;
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UserConfig {
                notices,
                ..UserConfig::default()
            });
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
        notices,
    })
}

/// What a command says about locating the user folder and `config.toml`, given the file it
/// read (`None`: the folder cannot be located).
fn folder_notices(file: Option<&UserFile>) -> Vec<Diagnostic> {
    let Some(file) = file else {
        // Nothing is lost that the user set, since nothing could be read.
        return vec![Diagnostic::new(
            DiagnosticLevel::Info,
            "user-config-unlocated",
            format!(
                "no user config and no user level: the home directory is not an absolute path; set {} to use them",
                home::HOME_VARIABLE
            ),
        )];
    };
    let renamed = home::renamed_variable_in_use().map(|used| {
        let message = if used {
            format!(
                "{} is deprecated: rename it {} (a later Mars stops reading it)",
                home::RENAMED_VARIABLE,
                home::HOME_VARIABLE
            )
        } else {
            format!(
                "{} is ignored because {} is set; unset it",
                home::RENAMED_VARIABLE,
                home::HOME_VARIABLE
            )
        };
        Diagnostic::warning("mars-config-dir-renamed", message)
    });
    renamed.into_iter().chain(old_copy_warning(file)).collect()
}

/// The warning for a user file (`config.toml`, `curated.toml`) with a copy where Mars 0.40 kept
/// it (`platform::home::user_file`): read until a later Mars stops reading it, or ignored.
pub(crate) fn old_copy_warning(file: &UserFile) -> Option<Diagnostic> {
    let message = match file.old.as_ref()? {
        OldCopy::Read { to } => format!(
            "read {}, where Mars 0.40 kept it: move it to {} (a later Mars stops reading the old path)",
            file.path.display(),
            to.display()
        ),
        OldCopy::Ignored { old } => format!(
            "{} is ignored: Mars reads {} instead; delete the old one",
            old.display(),
            file.path.display()
        ),
    };
    Some(Diagnostic::warning("user-config-moved", message))
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
