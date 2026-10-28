//! Marketplace plugin acquisition: choose one plugin out of a fetched marketplace and bring its
//! files in.
//!
//! The marketplace is an ordinary git or path source, fetched and locked by the usual version
//! machinery. This module reads it (`mars_schema::inspect`), finds the plugin the dependency
//! names, and roots the package at the plugin's files: a directory of the marketplace's own
//! checkout, or another repository fetched through the same `SourceFetcher`. The plugin's content
//! commit is replayed from the lock while the marketplace is the locked commit, and resolved anew
//! otherwise (`mars upgrade`, or a marketplace that moved). `mars-schema` never fetches.

use mars_schema::claude::manifest::{Checkout, MARKETPLACE_MANIFEST, PluginSource};
use mars_schema::inspect::{Hint, InspectedPlugin, Kind, PluginHint, inspect};

use super::SourceProvider;
use super::package::PendingSource;
use super::path::apply_subpath;
use super::types::{PluginRoot, ResolveOptions, RootedSourceRef, VersionMode};
use super::version::locked_source_matches_pending;
use crate::diagnostic::DiagnosticCollector;
use crate::error::MarsError;
use crate::lock::{LockFile, LockedContent, LockedSource};
use crate::package_prepare::DiskTreeSource;
use crate::source::ResolvedRef;
use crate::types::{CommitHash, SourceId, SourceUrl};
use mars_schema::git_url::canonicalize_git_url;
use mars_schema::names::SourceSubpath;
use mars_schema::version::SelectionPolicy;

/// Root `pending`'s package in the resolved source `marketplace`: at the plugin it names in the
/// marketplace the source holds, or at the source below its `subpath`. A dependency that names no
/// plugin still takes a marketplace's only one (v0.19.0 installed such a dependency); with several,
/// preparing the source asks for a choice.
///
/// The manifest check on disk stays here because `inspect` walks and lists the whole tree: asking
/// it of every unnamed dependency on every resolve would cost a second walk for a case only
/// marketplaces have. Moving the one-plugin rule into `inspect` as a default plugin would not
/// remove that check, so it is not moved.
pub(crate) fn root_package(
    pending: &PendingSource,
    marketplace: &ResolvedRef,
    provider: &dyn SourceProvider,
    locked: Option<&LockFile>,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Result<RootedSourceRef, MarsError> {
    let name = &pending.name;
    let invalid = |message: String| MarsError::InvalidRequest { message };
    let named = pending.source_id.plugin();
    let plain = || apply_subpath(name, &marketplace.tree_path, pending.subpath.as_ref());
    if named.is_none()
        && (pending.subpath.is_some()
            || !marketplace.tree_path.join(MARKETPLACE_MANIFEST).is_file())
    {
        return plain();
    }

    let tree = DiskTreeSource::new(&marketplace.tree_path)?;
    let inspection = inspect(&tree, &Hint::default());
    if let Some(error) = tree.take_error() {
        return Err(error);
    }
    let source = match &pending.spec {
        crate::config::SourceSpec::Git(git) => git.url.to_string(),
        crate::config::SourceSpec::Path(path) => path.display().to_string(),
    };
    let plugin = match (named, inspection.plugins.as_slice()) {
        (Some(plugin_name), _) => {
            match inspection.kind {
                Kind::Marketplace => {}
                // The manifest decides before a marketplace file does: a Mars package's own
                // plugin output carries one at its root.
                Kind::Schema1 | Kind::MarsLegacy | Kind::SchemaNewer => {
                    return Err(invalid(format!(
                        "`{name}` names plugin `{plugin_name}`, but {source} is a Mars package (it has a `mars.toml`) and is read as one: drop `plugin` (`--plugin` on `mars add`)"
                    )));
                }
                _ => {
                    return Err(invalid(format!(
                        "`{name}` names plugin `{plugin_name}`, but {source} is not a Claude marketplace (it has no `.claude-plugin/marketplace.json`)"
                    )));
                }
            }
            inspection
                .plugins
                .iter()
                .find(|plugin| plugin.entry.name == plugin_name)
                .ok_or_else(|| {
                    invalid(format!(
                        "`{name}` names plugin `{plugin_name}`, which the marketplace at {source} does not list; its plugins are:\n{}",
                        plugin_list(&inspection.plugins)
                    ))
                })?
        }
        (None, [only]) if inspection.kind == Kind::Marketplace => only,
        // Several plugins: preparing the marketplace reports them and asks for a choice.
        (None, _) => return plain(),
    };
    let plugin_name = plugin.entry.name.as_str();
    if let Some(reason) = &plugin.refused {
        return Err(invalid(format!(
            "plugin `{plugin_name}` of `{name}` cannot be installed: {reason}"
        )));
    }

    let marketplace_url = match &pending.source_id {
        SourceId::Git { url, .. } => Some(url.clone()),
        SourceId::Path { .. } => None,
    };
    let (rooted, content, at_marketplace_root) = match &plugin.entry.source {
        PluginSource::Local { path } => {
            let subpath = optional_subpath(name.as_ref(), path)?;
            let rooted = apply_subpath(name, &marketplace.tree_path, subpath.as_ref())?;
            let content = LockedContent {
                url: marketplace_url,
                subpath,
                commit: marketplace.commit.clone(),
            };
            (rooted, content, path.is_empty())
        }
        PluginSource::Remote {
            repository,
            path,
            checkout,
        } => {
            let subpath = match path {
                Some(path) => optional_subpath(name.as_ref(), path)?,
                None => None,
            };
            let url = SourceUrl::from(canonicalize_git_url(repository));
            let replay = replayable_commit(
                pending,
                marketplace,
                locked,
                options,
                &url,
                subpath.as_ref(),
            )?;
            let fetched = fetch_content(
                pending, repository, checkout, replay, provider, options, diag,
            )?;
            let rooted = apply_subpath(name, &fetched.tree_path, subpath.as_ref())?;
            let content = LockedContent {
                url: Some(url),
                subpath,
                commit: fetched.commit,
            };
            (rooted, content, false)
        }
        PluginSource::Unsupported { reason } => {
            return Err(invalid(format!(
                "plugin `{plugin_name}` of `{name}` cannot be installed: {reason}"
            )));
        }
    };

    report_move(pending, locked, plugin_name, &content, diag);
    Ok(RootedSourceRef {
        plugin: Some(PluginRoot {
            hint: PluginHint {
                entry: plugin.entry.clone(),
                at_marketplace_root,
            },
            content,
            named: named.is_some(),
        }),
        ..rooted
    })
}

pub(crate) fn plugin_list(plugins: &[InspectedPlugin]) -> String {
    plugins
        .iter()
        .map(|plugin| match &plugin.refused {
            Some(reason) => format!("  {} (cannot be installed: {reason})", plugin.entry.name),
            None => format!("  {}", plugin.entry.name),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn optional_subpath(name: &str, path: &str) -> Result<Option<SourceSubpath>, MarsError> {
    if path.is_empty() {
        return Ok(None);
    }
    SourceSubpath::new(path)
        .map(Some)
        .map_err(|error| MarsError::InvalidRequest {
            message: format!("plugin path `{path}` of `{name}` is not usable: {error}"),
        })
}

/// The lock's record of this very dependency (same source and plugin).
fn locked_source<'a>(
    pending: &PendingSource,
    locked: Option<&'a LockFile>,
) -> Option<&'a LockedSource> {
    locked?
        .dependencies
        .get(&pending.name)
        .filter(|source| locked_source_matches_pending(source, pending))
}

/// The locked content commit to fetch again, when the lock still describes this plugin: the
/// marketplace is the locked commit (so its entry is the one the lock was made from) and names
/// the same repository and path. `mars upgrade` resolves anew; `--frozen` insists on a replay.
fn replayable_commit(
    pending: &PendingSource,
    marketplace: &ResolvedRef,
    locked: Option<&LockFile>,
    options: &ResolveOptions,
    url: &SourceUrl,
    subpath: Option<&SourceSubpath>,
) -> Result<Option<CommitHash>, MarsError> {
    let mode = options.version_mode(&pending.name);
    if mode == VersionMode::Choose(SelectionPolicy::LatestOnly) {
        return Ok(None);
    }
    let replay = locked_source(pending, locked)
        .filter(|source| source.commit == marketplace.commit)
        .and_then(|source| source.content.as_ref())
        .filter(|content| {
            content.url.as_ref().is_some_and(|locked_url| {
                canonicalize_git_url(locked_url.as_ref()) == canonicalize_git_url(url.as_ref())
            }) && content.subpath.as_ref() == subpath
        })
        .and_then(|content| content.commit.clone());
    if replay.is_none() && mode == VersionMode::Frozen {
        return Err(MarsError::FrozenViolation {
            message: format!(
                "--frozen requires a locked content commit for plugin `{}` of `{}`",
                pending.source_id.plugin().unwrap_or_default(),
                pending.name
            ),
        });
    }
    Ok(replay)
}

/// Fetch the plugin's repository (the crate has refused any that is not a network repository)
/// at the entry's `sha`, the replayed commit, or its `ref` (else
/// the default branch) as it is now.
fn fetch_content(
    pending: &PendingSource,
    repository: &str,
    checkout: &Checkout,
    replay: Option<CommitHash>,
    provider: &dyn SourceProvider,
    options: &ResolveOptions,
    diag: &mut DiagnosticCollector,
) -> Result<ResolvedRef, MarsError> {
    let url = SourceUrl::from(repository);
    let name = pending.name.as_ref();
    if let Checkout::Sha { sha } = checkout {
        return provider.fetch_git_commit(&url, sha, name, diag);
    }
    if let Some(commit) = replay {
        match provider.fetch_git_commit(&url, &commit, name, diag) {
            Err(MarsError::LockedCommitUnreachable {
                commit,
                url: source_url,
            }) if options.version_mode(&pending.name) != VersionMode::Frozen => {
                diag.warn(
                    "locked-commit-unreachable",
                    format!(
                        "locked commit {commit} for {source_url} is unreachable; re-resolving the plugin's repository"
                    ),
                );
            }
            result => return result,
        }
    }
    let reference = match checkout {
        Checkout::Ref { name } => name.as_str(),
        _ => "HEAD",
    };
    provider.fetch_git_ref(&url, reference, name, None, diag)
}

/// A plugin whose source changed since the lock names its old and new place.
fn report_move(
    pending: &PendingSource,
    locked: Option<&LockFile>,
    plugin: &str,
    now: &LockedContent,
    diag: &mut DiagnosticCollector,
) {
    let Some(before) = locked_source(pending, locked).and_then(|source| source.content.as_ref())
    else {
        return;
    };
    let place = |content: &LockedContent| {
        let url = content
            .url
            .as_ref()
            .map_or_else(|| "the local marketplace".to_owned(), ToString::to_string);
        match &content.subpath {
            Some(subpath) => format!("{url} (path {subpath})"),
            None => url,
        }
    };
    let same_place = |a: &LockedContent, b: &LockedContent| {
        a.subpath == b.subpath
            && a.url.as_ref().map(|url| canonicalize_git_url(url.as_ref()))
                == b.url.as_ref().map(|url| canonicalize_git_url(url.as_ref()))
    };
    if !same_place(before, now) {
        diag.warn_once(
            "plugin-source-moved",
            format!(
                "plugin `{plugin}` of `{}` moved: {} -> {}",
                pending.name,
                place(before),
                place(now)
            ),
        );
    }
}
