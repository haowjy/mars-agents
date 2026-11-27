//! Marketplace plugin acquisition: choose one plugin out of a fetched marketplace and bring its
//! files in.
//!
//! The marketplace is an ordinary git or path source, fetched and locked by the usual version
//! machinery. This module reads its listing, finds the plugin the dependency names, and roots the
//! package at the plugin's files: a folder of the marketplace's own checkout, or another
//! repository answered through the same lookups. The plugin's content commit is replayed from the
//! lock while the marketplace is the locked commit, and resolved anew otherwise.

use super::package::{Flow, Pending, Run};
use super::version::{VersionMode, locked_matches};
use super::{
    Dependency, Failure, Level, LockedContent, LockedDependency, Need, Origin, PluginDetail,
    PluginRoot, Ref, Rooted, SourceId, Spec,
};
use crate::claude::manifest::{Checkout, MARKETPLACE_MANIFEST, PluginSource};
use crate::git_url::canonicalize_git_url;
use crate::inspect::{
    Hint, InspectedPlugin, Kind, PluginHint, inspect, inspect_listed_plugin, list_marketplace,
};
use crate::names::SourceSubpath;
use crate::version::SelectionPolicy;

pub fn plugin_list(plugins: &[InspectedPlugin]) -> String {
    plugins
        .iter()
        .map(|plugin| match &plugin.refused {
            Some(reason) => format!("  {} (cannot be installed: {reason})", plugin.entry.name),
            None => format!("  {}", plugin.entry.name),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn optional_subpath(name: &str, path: &str) -> Result<Option<SourceSubpath>, String> {
    if path.is_empty() {
        return Ok(None);
    }
    SourceSubpath::new(path)
        .map(Some)
        .map_err(|error| format!("plugin path `{path}` of `{name}` is not usable: {error}"))
}

impl<'a> Run<'a> {
    fn plugin_failure<T>(
        &mut self,
        dep: &Dependency,
        detail: PluginDetail,
        text: String,
    ) -> Flow<T> {
        self.fail(Failure::Plugin {
            name: dep.name.clone(),
            detail,
            plugin: dep.plugin.clone(),
            text,
        })
    }

    /// Root `pending`'s package in the resolved source `marketplace`: at the plugin it names in
    /// the marketplace the source holds, or at the source below its `subpath`. A dependency that
    /// names no plugin still takes a marketplace's only one (v0.19.0 installed such a dependency);
    /// with several, preparing the source asks for a choice.
    pub(super) fn root_package(
        &mut self,
        pending: &Pending<'_>,
        marketplace: &Ref,
    ) -> Flow<(Rooted, Option<PluginRoot>)> {
        let dep = pending.dep;
        let named = pending.id.plugin();
        let plain = Rooted {
            checkout: marketplace.origin.clone(),
            subpath: dep.subpath.as_ref().map(|subpath| subpath.to_string()),
        };
        if named.is_none() && dep.subpath.is_some() {
            return Ok((plain, None));
        }

        let root = Rooted {
            checkout: marketplace.origin.clone(),
            subpath: None,
        };
        let tree = self.rooted_tree(dep, &root)?;
        if named.is_none() && !crate::tree::TreeSource::is_file(&*tree, MARKETPLACE_MANIFEST) {
            return Ok((plain, None));
        }

        let listing = list_marketplace(&*tree);
        self.read_error(&root)?;
        let name = &dep.name;
        let source = match &dep.spec {
            Spec::Git { url, .. } => url.clone(),
            Spec::Path(path) => path.clone(),
        };
        let entry = match (named, listing.entries.as_slice()) {
            (Some(plugin_name), entries) => {
                match listing.kind {
                    Kind::Marketplace => {}
                    // The manifest decides before a marketplace file does: a Mars package's own
                    // plugin output carries one at its root.
                    Kind::Schema1 | Kind::SchemaNewer => {
                        return self.plugin_failure(dep, PluginDetail::NotAMarketplace, format!(
                            "`{name}` names plugin `{plugin_name}`, but {source} is a Mars package (it has a `mars.toml`) and is read as one: drop `plugin` (`--plugin` on `mars add`)"
                        ));
                    }
                    _ => {
                        return self.plugin_failure(dep, PluginDetail::NotAMarketplace, format!(
                            "`{name}` names plugin `{plugin_name}`, but {source} is not a Claude marketplace (it has no `.claude-plugin/marketplace.json`)"
                        ));
                    }
                }
                let Some(entry) = entries.iter().find(|entry| entry.name == plugin_name) else {
                    let inspection = inspect(&*tree, &Hint::default());
                    return self.plugin_failure(dep, PluginDetail::NotListed, format!(
                        "`{name}` names plugin `{plugin_name}`, which the marketplace at {source} does not list; its plugins are:\n{}",
                        plugin_list(&inspection.plugins)
                    ));
                };
                entry
            }
            (None, [only]) if listing.kind == Kind::Marketplace => only,
            (None, _) => return Ok((plain, None)),
        };
        let inspected = inspect_listed_plugin(&*tree, entry);
        self.read_error(&root)?;
        let plugin_name = entry.name.as_str();
        if let Some(reason) = &inspected.refused {
            return self.plugin_failure(
                dep,
                PluginDetail::Refused,
                format!("plugin `{plugin_name}` of `{name}` cannot be installed: {reason}"),
            );
        }

        let marketplace_url = match pending.id {
            SourceId::Git { url, .. } => Some(url.clone()),
            SourceId::Path { .. } => None,
        };
        let (rooted, content, at_marketplace_root) = match &entry.source {
            PluginSource::Local { path } => {
                let subpath = match optional_subpath(name, path) {
                    Ok(subpath) => subpath,
                    Err(text) => return self.plugin_failure(dep, PluginDetail::UnusablePath, text),
                };
                let rooted = Rooted {
                    checkout: marketplace.origin.clone(),
                    subpath: subpath.as_ref().map(ToString::to_string),
                };
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
                    Some(path) => match optional_subpath(name, path) {
                        Ok(subpath) => subpath,
                        Err(text) => {
                            return self.plugin_failure(dep, PluginDetail::UnusablePath, text);
                        }
                    },
                    None => None,
                };
                let url = canonicalize_git_url(repository);
                let replay =
                    self.replayable_commit(pending, marketplace, &url, subpath.as_ref())?;
                let commit =
                    self.fetch_content(dep, repository, checkout, replay, subpath.as_ref())?;
                let rooted = Rooted {
                    checkout: Origin::Git {
                        url: repository.clone(),
                        commit: commit.clone(),
                    },
                    subpath: subpath.as_ref().map(ToString::to_string),
                };
                let content = LockedContent {
                    url: Some(url),
                    subpath,
                    commit: Some(commit),
                };
                (rooted, content, false)
            }
            PluginSource::Unsupported { reason } => {
                return self.plugin_failure(
                    dep,
                    PluginDetail::Refused,
                    format!("plugin `{plugin_name}` of `{name}` cannot be installed: {reason}"),
                );
            }
        };

        self.report_move(pending, plugin_name, &content);
        Ok((
            rooted,
            Some(PluginRoot {
                hint: PluginHint {
                    entry: entry.clone(),
                    at_marketplace_root,
                },
                content,
                named: named.is_some(),
            }),
        ))
    }

    fn locked_row(&self, pending: &Pending<'_>) -> Option<&'a LockedDependency> {
        let req = self.req;
        req.locked
            .get(&pending.dep.name)
            .filter(|row| locked_matches(row, pending.id))
    }

    /// The locked content commit to fetch again, when the lock still describes this plugin: the
    /// marketplace is the locked commit (so its entry is the one the lock was made from) and names
    /// the same repository and path. `mars upgrade` resolves anew; `--frozen` insists on a replay.
    fn replayable_commit(
        &mut self,
        pending: &Pending<'_>,
        marketplace: &Ref,
        url: &str,
        subpath: Option<&SourceSubpath>,
    ) -> Flow<Option<String>> {
        let dep = pending.dep;
        let mode = self.req.version_mode(dep);
        if mode == VersionMode::Choose(SelectionPolicy::LatestOnly) {
            return Ok(None);
        }
        let replay = self
            .locked_row(pending)
            .filter(|row| row.commit == marketplace.commit)
            .and_then(|row| row.content.as_ref())
            .filter(|content| {
                content.url.as_ref().is_some_and(|locked_url| {
                    canonicalize_git_url(locked_url) == canonicalize_git_url(url)
                }) && content.subpath.as_ref() == subpath
            })
            .and_then(|content| content.commit.clone());
        if replay.is_none() && mode == VersionMode::Frozen {
            return self.fail(Failure::Frozen {
                message: format!(
                    "--frozen requires a locked content commit for plugin `{}` of `{}`",
                    pending.id.plugin().unwrap_or_default(),
                    dep.name
                ),
            });
        }
        Ok(replay)
    }

    /// The plugin's repository (the crate has refused any that is not a network repository) at
    /// the entry's `sha`, the replayed commit, or its `ref` (else the default branch) as it is
    /// now. Returns the commit.
    fn fetch_content(
        &mut self,
        dep: &Dependency,
        repository: &str,
        checkout: &Checkout,
        replay: Option<String>,
        subpath: Option<&SourceSubpath>,
    ) -> Flow<String> {
        let path = subpath.map(SourceSubpath::as_str);
        if let Checkout::Sha { sha } = checkout {
            return self.existing_commit(repository, sha, path);
        }
        if let Some(commit) = replay {
            if self.tree(repository, &commit, path)?.is_some() {
                return Ok(commit);
            }
            if self.req.version_mode(dep) == VersionMode::Frozen {
                return self.missing_commit(repository, &commit, path);
            }
            self.note(
                Level::Warning,
                "locked-commit-unreachable",
                format!(
                    "locked commit {commit} for {repository} is unreachable; re-resolving the plugin's repository"
                ),
                false,
            );
        }
        let reference = match checkout {
            Checkout::Ref { name } => name.as_str(),
            _ => "HEAD",
        };
        let commit = self.reference(repository, reference)?;
        self.existing_commit(repository, &commit, path)
    }

    fn existing_commit(&mut self, url: &str, commit: &str, path: Option<&str>) -> Flow<String> {
        if self.tree(url, commit, path)?.is_none() {
            return self.missing_commit(url, commit, path);
        }
        Ok(commit.to_string())
    }

    fn missing_commit<T>(&mut self, url: &str, commit: &str, path: Option<&str>) -> Flow<T> {
        let package = self.current_package();
        self.fail(Failure::Unavailable {
            package,
            need: Need::Tree {
                url: url.to_string(),
                commit: commit.to_string(),
                path: path.map(str::to_string),
            },
            reason: "the commit does not exist".to_string(),
        })
    }

    /// A plugin whose source changed since the lock names its old and new place.
    fn report_move(&mut self, pending: &Pending<'_>, plugin: &str, now: &LockedContent) {
        let Some(before) = self
            .locked_row(pending)
            .and_then(|row| row.content.as_ref())
        else {
            return;
        };
        let place = |content: &LockedContent| {
            let url = content
                .url
                .clone()
                .unwrap_or_else(|| "the local marketplace".to_owned());
            match &content.subpath {
                Some(subpath) => format!("{url} (path {subpath})"),
                None => url,
            }
        };
        let canonical = |content: &LockedContent| content.url.as_deref().map(canonicalize_git_url);
        if before.subpath == now.subpath && canonical(before) == canonical(now) {
            return;
        }
        let message = format!(
            "plugin `{plugin}` of `{}` moved: {} -> {}",
            pending.dep.name,
            place(before),
            place(now)
        );
        self.note(Level::Warning, "plugin-source-moved", message, true);
    }
}
