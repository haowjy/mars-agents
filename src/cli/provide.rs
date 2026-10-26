//! `mars provide <provider>@<version>=<directory>` — install a host's own skills.
//!
//! A host (Meridian today) ships skills with its binary and runs this on every launch, so the
//! command must be quiet, fast and side-effect free when the bundle is already current. It
//! writes only `.mars/skills/` and `.mars/provided.lock`; `mars.toml`, `mars.lock` and every
//! harness target are untouched, which is why it needs no sync and no config.

use std::path::Path;

use crate::error::{ConfigError, MarsError};
use crate::provided::{ProvideReport, Status};

use super::output;

/// Arguments for `mars provide`.
#[derive(Debug, clap::Args)]
pub struct ProvideArgs {
    /// What the host provides: `<provider>@<version>=<directory>`.
    ///
    /// The directory holds a `skills/` directory; each entry is `<skill>__<provider>`.
    pub spec: String,
}

/// Run `mars provide`.
///
/// `root` is the global `--root`. The walk-up that finds the project must not escape it: a host
/// names the root it wants filled, and silently filling a parent's `.mars/` instead would leave
/// the host reading a copy it did not ask for.
pub fn run(args: &ProvideArgs, root: Option<&Path>, json: bool) -> Result<i32, MarsError> {
    let project_root = super::find_agents_root(root)?.project_root;
    if let Some(given) = root {
        let given = dunce::canonicalize(given).unwrap_or_else(|_| given.to_path_buf());
        if given != project_root {
            return Err(MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "--root {} has no `mars.toml`; the nearest project root is {}. `mars provide` writes only into the root it is given",
                    given.display(),
                    project_root.display()
                ),
            }));
        }
    }

    let report = crate::provided::run(&project_root, &args.spec)?;
    if json {
        output::print_json(&report);
    } else {
        print_report(&report);
    }
    Ok(0)
}

fn print_report(report: &ProvideReport) {
    match report.status {
        Status::UpToDate => output::print_info(&format!(
            "provided {}@{} is up to date",
            report.provider, report.version
        )),
        Status::Busy => output::print_warnings(&[format!(
            "another process holds `.mars/sync.lock`; {}@{} was not refreshed",
            report.provider, report.version
        )]),
        Status::SyncPending => output::print_warnings(&[format!(
            "an interrupted `{}` left `.mars/pending-canonical.json`; run `{}` to recover before refreshing {}@{}",
            crate::types::managed_cmd("mars sync"),
            crate::types::managed_cmd("mars repair"),
            report.provider,
            report.version
        )]),
        Status::Applied => output::print_info(&applied_summary(report)),
    }
    output::print_warnings(&report.warnings);
}

fn applied_summary(report: &ProvideReport) -> String {
    let mut parts = Vec::new();
    for (label, names) in [
        ("installed", &report.installed),
        ("replaced", &report.replaced),
        ("adopted", &report.adopted),
        ("removed", &report.removed),
        ("shadowed", &report.shadowed),
        ("collision", &report.collisions),
    ] {
        if !names.is_empty() {
            parts.push(format!("{} {label}", names.len()));
        }
    }
    let what = format!("provided {}@{}", report.provider, report.version);
    if parts.is_empty() {
        return format!("{what}: record is current");
    }
    format!("{what}: {}", parts.join(", "))
}
