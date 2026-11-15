//! What `link` and `unlink` share: target-name validation and the claim on a target.
//!
//! A target is claimed by two files: its lock records and `settings.targets`. `link`
//! writes the lock first and settings last; `unlink` writes settings first and the lock
//! last. A kill between the two writes can therefore only leave the lock claiming a
//! target settings don't list. `unlink` sees that state through `lock_claims` and undoes
//! it; `link` finds the lock claim already installed and finishes it. Neither needs
//! `--force`. Keep that order if either command changes.

use crate::error::MarsError;
use crate::lock::LockFile;

/// Whether the lock records outputs or config entries for `target`, whatever settings say.
pub fn lock_claims(lock: &LockFile, target: &str) -> bool {
    !lock.output_dest_paths_for_target(target).is_empty()
        || lock.config_entries.contains_key(target)
}

/// Normalize and validate a target directory name.
///
/// Strips trailing slashes, rejects paths (containing `/` or `\`),
/// empty/dot names, and `.mars`, the canonical store, which is not a target.
pub fn normalize_target_name(target: &str) -> Result<String, MarsError> {
    let normalized = target.trim_end_matches('/').trim_end_matches('\\');
    if normalized.contains('/') || normalized.contains('\\') {
        return Err(MarsError::Link {
            target: target.to_string(),
            message: "target must be a directory name, not a path".to_string(),
        });
    }
    if normalized.is_empty() || normalized == "." || normalized == ".." {
        return Err(MarsError::Link {
            target: target.to_string(),
            message: "invalid target name".to_string(),
        });
    }
    if normalized == crate::lock::CANONICAL_TARGET_ROOT {
        return Err(MarsError::Link {
            target: target.to_string(),
            message: "`.mars` is the canonical store, not a target".to_string(),
        });
    }
    Ok(normalized.to_string())
}

#[cfg(test)]
mod tests {
    use super::normalize_target_name;

    #[test]
    fn normalize_strips_trailing_slash() {
        assert_eq!(normalize_target_name(".claude/").unwrap(), ".claude");
    }

    #[test]
    fn normalize_rejects_path() {
        assert!(normalize_target_name("foo/bar").is_err());
    }

    #[test]
    fn normalize_rejects_empty() {
        assert!(normalize_target_name("").is_err());
    }

    #[test]
    fn normalize_rejects_dots() {
        assert!(normalize_target_name(".").is_err());
        assert!(normalize_target_name("..").is_err());
    }
}
