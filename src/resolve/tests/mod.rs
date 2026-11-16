#![cfg(test)]

mod filter_tests;
mod integration_tests;
mod version_tests;

use super::*;
use crate::config::{
    EffectiveConfig, EffectiveDependency, GitSpec, Manifest, PackageInfo, Settings, SourceSpec,
};
use crate::source::AvailableVersion;
use crate::types::{SourceId, SourceName, SourceUrl};
use indexmap::IndexMap;
use mars_schema::dependency::Entry;
use mars_schema::filter::{FilterConfig, FilterMode};
use mars_schema::names::{RenameMap, SourceSubpath};
use mars_schema::resolve::Tag;
use semver::Version;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A provider over temp-dir trees, keyed by source name. A url is bound to a name when the config
/// or a registered manifest names it (`bind`), because the core asks by url and commit.
pub(crate) struct MockProvider {
    /// url → sorted available versions
    versions: HashMap<String, Vec<AvailableVersion>>,
    /// Number of tag listings keyed by URL.
    list_versions_counts: RefCell<HashMap<String, usize>>,
    /// URLs where listing tags should fail.
    list_versions_failures: HashSet<String>,
    /// source tree paths keyed by source name (pre-created temp dirs)
    trees: HashMap<String, PathBuf>,
    /// Optional source tree overrides keyed by (source name, version tag).
    versioned_trees: HashMap<(String, String), PathBuf>,
    /// url → source name
    names: RefCell<HashMap<String, String>>,
    /// Commits for which `fetch_content` fails, simulating an unreachable lock replay.
    unreachable_commits: HashSet<String>,
    /// One entry per content fetch, in order: `Some(commit)` for an exact-commit replay, `None`
    /// for a tag or ref fetch. Tests assert the replay order with it.
    commit_fetches: RefCell<Vec<Option<String>>>,
}

impl MockProvider {
    fn new() -> Self {
        MockProvider {
            versions: HashMap::new(),
            list_versions_counts: RefCell::new(HashMap::new()),
            list_versions_failures: HashSet::new(),
            trees: HashMap::new(),
            versioned_trees: HashMap::new(),
            names: RefCell::new(HashMap::new()),
            unreachable_commits: HashSet::new(),
            commit_fetches: RefCell::new(Vec::new()),
        }
    }

    /// Register available versions for a URL.
    fn add_versions(&mut self, url: &str, versions: Vec<(u64, u64, u64)>) {
        let avs: Vec<AvailableVersion> = versions
            .into_iter()
            .map(|(major, minor, patch)| AvailableVersion {
                tag: format!("v{major}.{minor}.{patch}"),
                version: Version::new(major, minor, patch),
                commit_id: format!("mock-commit-v{major}.{minor}.{patch}"),
            })
            .collect();
        self.versions.insert(url.to_string(), avs);
    }

    fn bind(&self, url: &str, name: &str) {
        self.names
            .borrow_mut()
            .entry(url.to_string())
            .or_insert_with(|| name.to_string());
    }

    fn bind_config(&self, config: &EffectiveConfig) {
        for (name, dependency) in &config.dependencies {
            if let SourceSpec::Git(git) = &dependency.spec {
                self.bind(git.url.as_ref(), name.as_ref());
            }
        }
    }

    /// Register a source tree for a source name; a manifest is written to the tree's `mars.toml`.
    fn add_source(&mut self, name: &str, tree_path: PathBuf, manifest: Option<Manifest>) {
        if let Some(manifest) = &manifest {
            write_manifest(&tree_path, manifest);
            for (dep_name, entry) in &manifest.dependencies {
                if let Some(url) = entry.url() {
                    self.bind(url, dep_name);
                }
            }
        }
        self.trees.insert(name.to_string(), tree_path);
    }

    fn add_versioned_source(
        &mut self,
        name: &str,
        version_tag: &str,
        tree_path: PathBuf,
        manifest: Option<Manifest>,
    ) {
        if let Some(manifest) = &manifest {
            write_manifest(&tree_path, manifest);
            for (dep_name, entry) in &manifest.dependencies {
                if let Some(url) = entry.url() {
                    self.bind(url, dep_name);
                }
            }
        }
        self.versioned_trees
            .insert((name.to_string(), version_tag.to_string()), tree_path);
    }

    fn mark_unreachable_commit(&mut self, commit: &str) {
        self.unreachable_commits.insert(commit.to_string());
    }

    fn commit_fetches(&self) -> Vec<Option<String>> {
        self.commit_fetches.borrow().clone()
    }

    fn list_versions_count(&self, url: &str) -> usize {
        self.list_versions_counts
            .borrow()
            .get(url)
            .copied()
            .unwrap_or(0)
    }

    fn name_of(&self, url: &SourceUrl) -> String {
        self.names
            .borrow()
            .get(url.as_ref())
            .cloned()
            .unwrap_or_default()
    }
}

impl SourceProvider for MockProvider {
    fn list_tags(&self, url: &SourceUrl) -> Result<Vec<Tag>, MarsError> {
        *self
            .list_versions_counts
            .borrow_mut()
            .entry(url.to_string())
            .or_insert(0) += 1;
        if self.list_versions_failures.contains(url.as_ref()) {
            return Err(MarsError::Source {
                source_name: url.to_string(),
                message: "mock list_versions failure".to_string(),
            });
        }
        Ok(self
            .versions
            .get(url.as_ref())
            .into_iter()
            .flatten()
            .map(|version| Tag {
                tag: version.tag.clone(),
                commit: version.commit_id.clone(),
            })
            .collect())
    }

    fn resolve_ref(&self, _url: &SourceUrl, reference: &str) -> Result<String, MarsError> {
        Ok(format!("ref:{reference}"))
    }

    fn fetch_content(
        &self,
        url: &SourceUrl,
        commit: &str,
        tag: Option<&str>,
    ) -> Result<PathBuf, MarsError> {
        let replay = tag.is_none() && !commit.starts_with("ref:");
        self.commit_fetches
            .borrow_mut()
            .push(replay.then(|| commit.to_string()));
        if self.unreachable_commits.contains(commit) {
            return Err(MarsError::LockedCommitUnreachable {
                commit: commit.to_string(),
                url: url.to_string(),
            });
        }
        let name = self.name_of(url);
        Ok(tag
            .and_then(|tag| self.versioned_trees.get(&(name.clone(), tag.to_string())))
            .or_else(|| self.trees.get(&name))
            .cloned()
            .unwrap_or_default())
    }

    fn fetch_path(&self, path: &Path, _source_name: &str) -> Result<PathBuf, MarsError> {
        Ok(path.to_path_buf())
    }
}

fn make_config(sources: Vec<(&str, SourceSpec)>) -> EffectiveConfig {
    let mut map = IndexMap::new();
    for (name, spec) in sources {
        map.insert(
            name.into(),
            EffectiveDependency {
                declared_source_id: source_id_for_spec(&spec, None),
                source_id: source_id_for_spec(&spec, None),
                spec,
                subpath: None,
                filter: FilterMode::All,
                rename: RenameMap::new(),
                dialect: None,
            },
        );
    }
    EffectiveConfig {
        dependencies: map,
        settings: Settings::default(),
        models: indexmap::IndexMap::new(),
        agents: indexmap::IndexMap::new(),
        skills: indexmap::IndexMap::new(),
    }
}

fn git_spec(url: &str, version: Option<&str>) -> SourceSpec {
    SourceSpec::Git(GitSpec {
        url: SourceUrl::from(url),
        version: version.map(|s| s.to_string()),
    })
}

fn make_manifest(name: &str, version: &str, deps: Vec<(&str, &str, &str)>) -> Manifest {
    let mut dependencies = IndexMap::new();
    for (dep_name, dep_url, dep_ver) in deps {
        dependencies.insert(
            dep_name.to_string(),
            Entry {
                version: Some(dep_ver.to_string()),
                ..Entry::git(dep_url)
            },
        );
    }
    Manifest {
        package: PackageInfo {
            name: name.to_string(),
            version: version.to_string(),
            description: None,
            authors: None,
            homepage: None,
            license: None,
            schema: None,
            uses_providers: Vec::new(),
            requires_mars: None,
            requires_meridian: None,
            tools: Default::default(),
        },
        dependencies,
        models: indexmap::IndexMap::new(),
    }
}

fn make_engine_manifest(
    name: &str,
    version: &str,
    requires_mars: Option<&str>,
    requires_meridian: Option<&str>,
) -> Manifest {
    let mut manifest = make_manifest(name, version, vec![]);
    manifest.package.requires_mars = requires_mars.map(str::to_string);
    manifest.package.requires_meridian = requires_meridian.map(str::to_string);
    manifest
}

fn make_manifest_with_filters(
    name: &str,
    version: &str,
    deps: Vec<(&str, &str, &str, FilterConfig)>,
) -> Manifest {
    let mut dependencies = IndexMap::new();
    for (dep_name, dep_url, dep_ver, dep_filter) in deps {
        dependencies.insert(
            dep_name.to_string(),
            Entry {
                version: Some(dep_ver.to_string()),
                filter: dep_filter,
                ..Entry::git(dep_url)
            },
        );
    }
    Manifest {
        package: PackageInfo {
            name: name.to_string(),
            version: version.to_string(),
            description: None,
            authors: None,
            homepage: None,
            license: None,
            schema: None,
            uses_providers: Vec::new(),
            requires_mars: None,
            requires_meridian: None,
            tools: Default::default(),
        },
        dependencies,
        models: indexmap::IndexMap::new(),
    }
}

fn default_options() -> ResolveOptions {
    ResolveOptions::default()
}

fn resolve(
    config: &EffectiveConfig,
    provider: &MockProvider,
    locked: Option<&LockFile>,
    options: &ResolveOptions,
) -> Result<ResolvedGraph, MarsError> {
    resolve_with_diagnostics(config, provider, locked, options).0
}

fn resolve_with_diagnostics(
    config: &EffectiveConfig,
    provider: &MockProvider,
    locked: Option<&LockFile>,
    options: &ResolveOptions,
) -> (
    Result<ResolvedGraph, MarsError>,
    Vec<crate::diagnostic::Diagnostic>,
) {
    provider.bind_config(config);
    let mut diag = DiagnosticCollector::new();
    let result = super::resolve(config, provider, locked, options, &mut diag);
    (result, diag.drain())
}

/// The tree's `mars.toml`, as a package author would write it.
fn write_manifest(tree: &Path, manifest: &Manifest) {
    #[derive(serde::Serialize)]
    struct File<'a> {
        package: &'a PackageInfo,
        dependencies: &'a IndexMap<String, Entry>,
    }
    let text = toml::to_string(&File {
        package: &manifest.package,
        dependencies: &manifest.dependencies,
    })
    .expect("serialize manifest")
    .replacen("[package]\n", "[package]\nschema = 1\n", 1);
    std::fs::write(tree.join("mars.toml"), text).expect("write mars.toml");
}

fn write_minimal_package_marker(tree: &Path) {
    std::fs::write(
        tree.join("mars.toml"),
        "[package]\nname = \"pkg\"\nversion = \"1.0.0\"\nschema = 1\n",
    )
    .expect("write mars.toml");
}

fn write_skill(tree: &Path, name: &str) {
    let dir = tree.join("skills").join(name);
    std::fs::create_dir_all(&dir).expect("create skill dir");
    std::fs::write(dir.join("SKILL.md"), "---\n---\n").expect("write SKILL.md");
}

fn write_agent(tree: &Path, name: &str, skills: &[&str]) {
    let agents = tree.join("agents");
    std::fs::create_dir_all(&agents).expect("create agents dir");
    let frontmatter = if skills.is_empty() {
        "---\n---\n".to_string()
    } else {
        format!("---\nskills: [{}]\n---\n", skills.join(", "))
    };
    std::fs::write(agents.join(format!("{name}.md")), frontmatter).expect("write agent");
}

fn source_id_for_spec(spec: &SourceSpec, subpath: Option<SourceSubpath>) -> SourceId {
    match spec {
        SourceSpec::Git(g) => {
            let canonical_url =
                SourceUrl::from(mars_schema::git_url::canonicalize_git_url(g.url.as_ref()));
            SourceId::git_with_subpath(canonical_url, subpath)
        }
        SourceSpec::Path(path) => SourceId::Path {
            canonical: path.clone(),
            subpath,
            plugin: None,
        },
    }
}
