//! Source string syntax classification.
//!
//! Determines whether a CLI/config source string is a local path or a source identifier.

use std::path::PathBuf;

/// Classify a source string as a local path if it matches local path syntax.
///
/// Accepted POSIX forms:
/// - `/absolute`
/// - `./relative`, `../relative`
/// - `~/home-relative`
/// - `.` and `..`
///
/// Accepted Windows forms:
/// - Drive paths: `C:\path`, `C:/path`, `C:relative`
/// - UNC paths: `\\server\share\path`
/// - Root-relative: `\path`
/// - Relative backslash: `.\path`, `..\path`, `foo\bar`
///
/// Returns `Some(PathBuf)` if the input is a local path, `None` otherwise.
///
/// NOT classified as local paths (URL-like or shorthand):
/// - Contains `://` (URL scheme)
/// - SSH shorthand: `git@host:path`
/// - GitHub/GitLab shorthand: `owner/repo` (no backslash, has forward slash)
/// - Protocol aliases: `github:`, `gitlab:`
pub fn classify_local_source(input: &str) -> Option<PathBuf> {
    // Empty input is not a local path.
    if input.is_empty() {
        return None;
    }

    // URL-like inputs are never local paths.
    if input.contains("://") {
        return None;
    }

    // SSH shorthand (git@host:path) is not a local path.
    if mars_schema::git_url::is_ssh_shorthand(input) {
        return None;
    }

    // Protocol aliases are not local paths.
    if input.starts_with("github:") || input.starts_with("gitlab:") {
        return None;
    }

    // POSIX absolute path.
    if input.starts_with('/') {
        return Some(PathBuf::from(input));
    }

    // POSIX relative (current dir, parent dir).
    if input == "." || input == ".." || input.starts_with("./") || input.starts_with("../") {
        return Some(PathBuf::from(input));
    }

    // Home directory expansion.
    if input.starts_with('~') {
        return Some(PathBuf::from(input));
    }

    // Windows drive path: C:\, C:/, C:relative.
    if is_windows_drive_path(input) {
        return Some(PathBuf::from(input));
    }

    // Windows UNC path: \\server\share.
    if input.starts_with("\\\\") {
        // Reject extended/device paths.
        if input.starts_with("\\\\?\\") || input.starts_with("\\\\.\\") {
            return None;
        }
        return Some(PathBuf::from(input));
    }

    // Windows root-relative: \path.
    if input.starts_with('\\') {
        return Some(PathBuf::from(input));
    }

    // Windows relative with backslash: .\path, ..\path.
    if input.starts_with(".\\") || input.starts_with("..\\") {
        return Some(PathBuf::from(input));
    }

    // Contains backslash but no slash - treat as Windows relative path.
    if input.contains('\\') && !input.contains('/') {
        return Some(PathBuf::from(input));
    }

    // Forward-slash-only owner/repo-style shorthand is not a local path.
    if input.contains('/') && !input.contains('\\') && !input.contains('.') {
        return None;
    }

    None
}

/// Check if input is a Windows drive path (C:\, C:/, C:relative).
fn is_windows_drive_path(input: &str) -> bool {
    let bytes = input.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

#[cfg(test)]
mod tests {
    use super::classify_local_source;

    #[test]
    fn classify_accepts_local_paths() {
        for input in [
            // POSIX absolute and relative
            "/absolute/path",
            "/",
            ".",
            "..",
            "./relative",
            "../parent",
            // home-relative
            "~/path",
            "~",
            // Windows drive, UNC, root-relative and backslash-relative
            "C:\\path",
            "C:/path",
            "D:\\",
            "C:relative",
            "\\\\server\\share",
            "\\\\server\\share\\path",
            "\\path",
            ".\\relative",
            "..\\parent",
            "foo\\bar",
        ] {
            assert!(classify_local_source(input).is_some(), "{input}");
        }
    }

    #[test]
    fn classify_rejects_remote_and_extended_sources() {
        for input in [
            // extended paths
            "\\\\?\\C:\\path",
            "\\\\.\\Device",
            // URLs, ssh shorthand, protocol aliases, owner/repo shorthand
            "https://github.com/owner/repo",
            "git://host/path",
            "ssh://git@host/path",
            "git@github.com:owner/repo",
            "git@host:path",
            "github:owner/repo",
            "gitlab:group/repo",
            "owner/repo",
            "owner/repo/subpath",
        ] {
            assert!(classify_local_source(input).is_none(), "{input}");
        }
    }
}
