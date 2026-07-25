use std::collections::HashSet;

pub mod cursor;
pub mod cursor_cache;
pub mod opencode;
pub mod opencode_cache;
pub mod pi;
pub mod pi_cache;
pub mod probe_refresh;

pub use probe_refresh::ProbeRefreshMode;

pub use cursor::CursorProbeResult;
pub use opencode::OpenCodeProbeResult;
pub use pi::PiProbeResult;

/// Determine whether an OpenCode probe should be attempted.
/// Returns false if offline or opencode is not installed.
pub fn should_probe_opencode(installed: &HashSet<String>, is_offline: bool) -> bool {
    !is_offline && installed.contains("opencode")
}

/// Determine whether a cursor probe should be attempted.
/// Returns false if offline or cursor is not installed.
pub fn should_probe_cursor(installed: &HashSet<String>, is_offline: bool) -> bool {
    !is_offline && installed.contains("cursor")
}
