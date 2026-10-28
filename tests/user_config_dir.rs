//! The user-tier `curated.toml` is read from `MARS_CONFIG_DIR`.
//!
//! This is the only test in its binary, so setting the environment cannot race a reader
//! (a lib test cannot make that promise).
use mars_agents::curation::{CuratedRules, Decision, TierId};
use tempfile::tempdir;

#[test]
fn user_curated_rules_come_from_the_shared_mars_config_directory() {
    let config = tempdir().unwrap();
    std::fs::write(config.path().join("curated.toml"), "default='hide'\n").unwrap();
    let project = tempdir().unwrap();

    unsafe { std::env::set_var("MARS_CONFIG_DIR", config.path()) };
    let loaded = CuratedRules::load(project.path()).unwrap();
    assert_eq!(
        loaded.default_for_unmatched(),
        Decision::Hidden {
            tier: Some(TierId::User)
        }
    );

    // An empty variable is unset: the platform directory applies, never an empty path.
    unsafe { std::env::set_var("MARS_CONFIG_DIR", "") };
    assert_ne!(
        mars_agents::platform::config::mars_config_dir(),
        Some(std::path::PathBuf::new())
    );
}
