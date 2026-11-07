//! Source string syntax classification.
//!
//! Determines whether a CLI/config source string is a local path or a source identifier.

use std::path::PathBuf;

pub fn classify_local_source(input: &str) -> Option<PathBuf> {
    if input.is_empty() {
        return None;
    }

    if input.contains("://") {
        return None;
    }

    if mars_schema::git_url::is_ssh_shorthand(input) {
        return None;
    }

    if input.starts_with("github:") || input.starts_with("gitlab:") {
        return None;
    }

    if input.starts_with('/') {
        return Some(PathBuf::from(input));
    }

    if input == "." || input == ".." || input.starts_with("./") || input.starts_with("../") {
        return Some(PathBuf::from(input));
    }

    if input.starts_with('~') {
        return Some(PathBuf::from(input));
    }

    if is_windows_drive_path(input) {
        return Some(PathBuf::from(input));
    }

    if input.starts_with("\\\\") {
        // Reject extended/device paths.
        if input.starts_with("\\\\?\\") || input.starts_with("\\\\.\\") {
            return None;
        }
        return Some(PathBuf::from(input));
    }

    if input.starts_with('\\') {
        return Some(PathBuf::from(input));
    }

    if input.starts_with(".\\") || input.starts_with("..\\") {
        return Some(PathBuf::from(input));
    }

    if input.contains('\\') && !input.contains('/') {
        return Some(PathBuf::from(input));
    }

    if input.contains('/') && !input.contains('\\') && !input.contains('.') {
        return None;
    }

    None
}

fn is_windows_drive_path(input: &str) -> bool {
    let bytes = input.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

#[cfg(test)]
mod tests {
    use super::classify_local_source;

    #[test]
    fn classify_posix_absolute() {
        assert!(classify_local_source("/absolute/path").is_some());
        assert!(classify_local_source("/").is_some());
    }

    #[test]
    fn classify_posix_relative() {
        assert!(classify_local_source(".").is_some());
        assert!(classify_local_source("..").is_some());
        assert!(classify_local_source("./relative").is_some());
        assert!(classify_local_source("../parent").is_some());
    }

    #[test]
    fn classify_home_relative() {
        assert!(classify_local_source("~/path").is_some());
        assert!(classify_local_source("~").is_some());
    }

    #[test]
    fn classify_windows_drive_paths() {
        assert!(classify_local_source("C:\\path").is_some());
        assert!(classify_local_source("C:/path").is_some());
        assert!(classify_local_source("D:\\").is_some());
        assert!(classify_local_source("C:relative").is_some());
    }

    #[test]
    fn classify_windows_unc_paths() {
        assert!(classify_local_source("\\\\server\\share").is_some());
        assert!(classify_local_source("\\\\server\\share\\path").is_some());
    }

    #[test]
    fn classify_windows_root_relative() {
        assert!(classify_local_source("\\path").is_some());
    }

    #[test]
    fn classify_windows_backslash_relative() {
        assert!(classify_local_source(".\\relative").is_some());
        assert!(classify_local_source("..\\parent").is_some());
        assert!(classify_local_source("foo\\bar").is_some());
    }

    #[test]
    fn classify_rejects_extended_paths() {
        assert!(classify_local_source("\\\\?\\C:\\path").is_none());
        assert!(classify_local_source("\\\\.\\Device").is_none());
    }

    #[test]
    fn classify_rejects_urls() {
        assert!(classify_local_source("https://github.com/owner/repo").is_none());
        assert!(classify_local_source("git://host/path").is_none());
        assert!(classify_local_source("ssh://git@host/path").is_none());
    }

    #[test]
    fn classify_rejects_ssh_shorthand() {
        assert!(classify_local_source("git@github.com:owner/repo").is_none());
        assert!(classify_local_source("git@host:path").is_none());
    }

    #[test]
    fn classify_rejects_protocol_aliases() {
        assert!(classify_local_source("github:owner/repo").is_none());
        assert!(classify_local_source("gitlab:group/repo").is_none());
    }

    #[test]
    fn classify_rejects_shorthand() {
        assert!(classify_local_source("owner/repo").is_none());
        assert!(classify_local_source("owner/repo/subpath").is_none());
    }
}
