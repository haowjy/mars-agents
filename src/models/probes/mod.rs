use std::collections::HashSet;

pub mod opencode;
pub mod opencode_cache;
pub mod pi;
pub mod pi_cache;

pub use opencode::{OpenCodeProbeResult, probe, probe_with_timeout};
pub use pi::{PiProbeResult, probe as probe_pi, probe_with_timeout as probe_pi_with_timeout};

/// Determine whether an OpenCode probe should be attempted.
/// Returns false if offline or opencode is not installed.
pub fn should_probe_opencode(installed: &HashSet<String>, is_offline: bool) -> bool {
    !is_offline && installed.contains("opencode")
}
