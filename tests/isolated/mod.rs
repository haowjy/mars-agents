//! A `mars()` for tests that do not need a particular home, config or cache: every process
//! environment path points into a per-binary directory under `CARGO_TARGET_TMPDIR`, so a run
//! never reads the developer's `HOME` or `~/.config/mars`, nor writes `~/.cache/mars` (git
//! clones of `file://` sources land there otherwise).

use assert_cmd::Command;
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::common;

fn env_root() -> &'static PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        // One directory per test process: concurrent runs of the same binary must not share
        // (or delete) each other's state. Directories from earlier runs are pruned by age.
        let parent = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("isolated-env");
        std::fs::create_dir_all(&parent).unwrap();
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        for entry in std::fs::read_dir(&parent).unwrap().flatten() {
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > day));
            if stale {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        let root = parent.join(std::process::id().to_string());
        std::fs::create_dir_all(&root).unwrap();
        root
    })
}

/// The mars binary with `HOME`, XDG, config and cache directories isolated, and the models
/// catalog URL unreachable.
pub fn mars() -> Command {
    let mut cmd = common::mars();
    common::configure_assert_cmd(&mut cmd, env_root(), "http://127.0.0.1:9/api.json");
    cmd
}
