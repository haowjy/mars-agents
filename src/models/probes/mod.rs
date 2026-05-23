use std::collections::HashSet;

pub mod cursor;
pub mod cursor_cache;
pub mod opencode;
pub mod opencode_cache;
pub mod pi;
pub mod pi_cache;

pub use cursor::{
    CursorProbeResult, probe as probe_cursor, probe_with_timeout as probe_cursor_with_timeout,
};
pub use opencode::{OpenCodeProbeResult, probe, probe_with_timeout};
pub use pi::{PiProbeResult, probe as probe_pi, probe_with_timeout as probe_pi_with_timeout};

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
