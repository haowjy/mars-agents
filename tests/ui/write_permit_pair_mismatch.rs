use std::path::Path;

use mars_agents::target::{ConfigEntry, ConfigWrite, TargetAdapter};

fn round_ten_attack(
    adapter: &dyn TargetAdapter,
    authorized_write: ConfigWrite<'_>,
    unrelated_entries: &[ConfigEntry],
    independently_named_target: &Path,
) {
    // The old boundary accepted all three values. The new boundary accepts only
    // the bound operation plus the project root, so this cannot type-check.
    let _ = adapter.write_config_entries(
        authorized_write,
        unrelated_entries,
        independently_named_target,
    );
}

fn main() {}
