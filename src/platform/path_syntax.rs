//! Source string syntax classification.
//!
//! Determines whether a CLI/config source string is a local path or a source identifier.

use std::path::PathBuf;

/// Classify a source string as a local path if it matches local path syntax.
///
/// Accepted POSIX forms: `/absolute`, `./relative`, `../relative`, `~/home-relative`
/// Accepted Windows forms: `C:\path`, `C:/path`, `C:rel`, `\\server\share`, `\root`, `.\rel`, `..\rel`, `foo\bar`
///
/// Returns `Some(PathBuf)` if the input is a local path, `None` otherwise.
/// URL-like strings (containing `://` or matching git shorthand patterns) are never local paths.
pub fn classify_local_source(input: &str) -> Option<PathBuf> {
    // Placeholder - will be implemented in Slice 4
    // For now, delegate to existing is_local_path logic
    if is_local_path(input) {
        Some(PathBuf::from(input))
    } else {
        None
    }
}

// Temporary: copy existing logic for compatibility
fn is_local_path(input: &str) -> bool {
    input == "."
        || input == ".."
        || input.starts_with("./")
        || input.starts_with("../")
        || input.starts_with('/')
        || input.starts_with('~')
        || is_windows_drive_path(input)
}

fn is_windows_drive_path(input: &str) -> bool {
    let bytes = input.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}
