//! `mars provide <provider>@<version>=<directory>` — install a host's own skills, hooks and
//! MCP servers.
//!
//! A host (Meridian today) ships them with its binary and runs this on every launch, so the
//! command must be quiet, fast and side-effect free when the bundle is already current. It
//! writes only `.mars/skills/`, `.mars/provided/` and `.mars/provided.lock`; `mars.toml`, `mars.lock` and every
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
    /// The directory holds any of `skills/` (entries are `<skill>__<provider>`), `hooks/` and `mcp/`.
    pub spec: String,
}

/// `root` is the global `--root`. The walk-up that finds the project must not escape it: a host
/// names the root it wants filled, and silently filling a parent's `.mars/` instead would leave
/// the host reading a copy it did not ask for.
pub fn run(args: &ProvideArgs, root: Option<&Path>, json: bool) -> Result<i32, MarsError> {
    let layout = super::find_agents_root(root)?.layout;
    if let Some(given) = root {
        let given = dunce::canonicalize(given).unwrap_or_else(|_| given.to_path_buf());
        if given != layout.root() {
            return Err(MarsError::Config(ConfigError::Invalid {
                message: format!(
                    "--root {} has no `mars.toml`; the nearest project root is {}. `mars provide` writes only into the root it is given",
                    given.display(),
                    layout.root().display()
                ),
            }));
        }
    }

    // On stderr only: an older host runs this on every launch and parses the JSON.
    output::warn(
        "provide-deprecated",
        "`mars provide` is deprecated and goes in the next minor release: give the bundle to `mars sync --provided-root <host>@<version>=<dir>`, and to lookups (`build launch-bundle`, `agents`, `skills`) as `--provided-root <dir>`, which read it in place",
    );
    let report = crate::provided::reconcile(&layout, &args.spec)?;
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
        Status::Busy => output::warn(
            "provide-busy",
            format!(
                "another process holds `.mars/sync.lock`; {}@{} was not refreshed",
                report.provider, report.version
            ),
        ),
        Status::SyncPending => output::warn(
            "provide-sync-pending",
            format!(
                "an interrupted `{}` left `.mars/pending-canonical.json`; run `{}` to recover before refreshing {}@{}",
                crate::layout::root_cmd("sync"),
                crate::layout::root_cmd("repair"),
                report.provider,
                report.version
            ),
        ),
        Status::Applied => output::print_info(&applied_summary(report)),
    }
    // Shadowed names and collisions describe the disk, not this run: report them while they last,
    // so a host that fixed the cause sees the signal until the next run installs the skill.
    for message in leftover_warnings(report) {
        output::warn("provided-leftover", message);
    }
    // Source diagnostics describe the bundle; a launch that wrote nothing must stay quiet. JSON
    // keeps them on every status.
    if report.status == Status::Applied {
        for message in &report.warnings {
            output::warn("provided-source", message.as_str());
        }
    }
}

fn leftover_warnings(report: &ProvideReport) -> Vec<String> {
    let shadowed = report.shadowed.iter().map(|name| {
        format!(
            "provided skill `{name}` is owned by a package in `mars.lock`; leaving it to the package"
        )
    });
    let collisions = report.collisions.iter().map(|name| {
        format!(
            "`.mars/skills/{name}` exists, is not recorded as provided or installed by a package, and differs from the provided copy; left untouched"
        )
    });
    shadowed.chain(collisions).collect()
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
        return format!("{what}: record updated");
    }
    format!("{what}: {}", parts.join(", "))
}
