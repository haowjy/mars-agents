//! The one decision for what a sync writes on behalf of this machine only: the content of a
//! layer other than the project's (`crate::layer`: the user level's `_user-*`, a host's
//! `_provided-*`), and whatever serves a target only `mars.local.toml` names. None of it may reach
//! a commit, and none of it may take a path the project holds. For every such output (an item's
//! folder or file, an entry in a shared config file, a file made only to hold such entries) the
//! sync decides, from its plan and before it writes anything, exactly one of:
//!
//! - **yield**: the project claims the name or the path. Its own `.mars-src` items, its
//!   dependencies (renamed or not), and any path at the output's location that git tracks or Mars
//!   did not install, are rank-1 claims in the claim rule (`layer::yielded`, applied in
//!   `sync::target`), so the item is not installed and `--force` never adopts over the path.
//!   [`ProjectClaims`] is what the claim rule reads.
//! - **withhold**: the entry would go into a shared file that can be committed: git tracks it,
//!   the project's own entries go in it too, or it is someone else's file git does not ignore.
//!   One entry of a file cannot be kept out of a commit, so it is not written. [`decide`].
//! - **install and exclude**: everything it creates is listed in this work tree's block of git's
//!   `info/exclude` (`sync::exclude`) before it is written. [`decide`].
//!
//! The exclude block, the tracked check and the claim rule read these and nothing else: what git
//! tracks and what the block lists ([`Facts`]) are read once per sync, paths are resolved through
//! `Layout` to the project root, and without git nothing is tracked and nothing is excluded.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use mars_schema::hooks::HookOutput;
use mars_schema::item::ItemKind;

use crate::compiler::config_entries::render::RenderedHooks;
use crate::diagnostic::DiagnosticCollector;
use crate::layout::Layout;
use crate::lock::{CANONICAL_TARGET_ROOT, LockFile, LockIndex, RecordRef};
use crate::sync::package_dir::PackageDir;
use crate::sync::target::TargetState;
use crate::target::{HOOK_ENTRY_KIND, HOOK_KEY_PREFIX, mcp_file::MCP_KIND};
use crate::types::DestPath;

/// What the work tree says about the paths a sync may write: which files git tracks under the
/// managed targets and their config files, and what this work tree's exclude block lists.
#[derive(Debug, Default)]
pub(crate) struct Facts {
    root: PathBuf,
    /// Root-relative, with `/`.
    tracked: BTreeSet<String>,
    listed: BTreeSet<PathBuf>,
}

impl Facts {
    pub(crate) fn load(layout: &Layout, managed_targets: &[String]) -> Self {
        let root = layout.root().to_path_buf();
        let Some(_) = crate::sync::exclude::work_tree(&root) else {
            return Self {
                root,
                ..Self::default()
            };
        };
        let mut specs: BTreeSet<String> = managed_targets
            .iter()
            .flat_map(|target| {
                std::iter::once(target.clone()).chain(
                    crate::target::keyed_sections(target)
                        .into_iter()
                        .map(|(_, section)| section.path.to_string()),
                )
            })
            .collect();
        if let Ok(store) = layout.store().strip_prefix(&root) {
            specs.insert(slash(store));
        }
        let tracked = git_lines(&root, &["ls-files", "-z", "--"], &specs);
        Self {
            listed: crate::sync::exclude::listed(layout),
            root,
            tracked,
        }
    }

    fn relative(&self, path: &Path) -> Option<String> {
        path.strip_prefix(&self.root).ok().map(slash)
    }

    /// Whether git tracks `path` or, for a folder, anything in it.
    fn tracked(&self, path: &Path) -> bool {
        let Some(relative) = self.relative(path) else {
            return false;
        };
        let folder = format!("{relative}/");
        self.tracked.contains(&relative)
            || self
                .tracked
                .range(folder.clone()..)
                .next()
                .is_some_and(|first| first.starts_with(&folder))
    }

    /// Whether `path` is the project's: git tracks it, or it exists and is neither recorded by
    /// the lock nor listed in this work tree's block (a path a sync killed midway wrote for a
    /// machine-local item is listed, since the block is written first).
    fn claims(&self, path: &Path, recorded: bool) -> bool {
        self.tracked(path)
            || (!recorded && path.symlink_metadata().is_ok() && !self.listed.contains(path))
    }
}

fn slash(relative: &Path) -> String {
    relative.to_string_lossy().replace('\\', "/")
}

/// The NUL-separated lines of `git <args> <specs>` run in `root`; none when git fails or is
/// missing, which counts as nothing tracked.
fn git_lines(root: &Path, args: &[&str], specs: &BTreeSet<String>) -> BTreeSet<String> {
    if specs.is_empty() {
        return BTreeSet::new();
    }
    let output = std::process::Command::new("git")
        .args(args)
        .args(specs)
        .current_dir(root)
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(output) = output else {
        return BTreeSet::new();
    };
    // `check-ignore` exits 1 when nothing is ignored, with nothing printed.
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// A claim as the claim rule keys it: `(kind, target, name)`, the target being `.mars` for the
/// kinds that install one item for every target (agents, skills, bootstrap docs).
pub(crate) type ClaimKey = (ItemKind, String, String);

/// The project's rank-1 claims a machine-local item yields to, beside its dependencies' items:
/// the names its own `.mars-src` items take, and the paths on disk it holds.
#[derive(Default)]
pub(crate) struct ProjectClaims<'a> {
    names: HashSet<ClaimKey>,
    disk: Option<DiskClaims<'a>>,
}

struct DiskClaims<'a> {
    layout: &'a Layout,
    facts: &'a Facts,
    recorded: LockIndex<'a>,
    owns: crate::sync::ownership::Ownership<'a>,
    managed_targets: &'a [String],
}

impl<'a> ProjectClaims<'a> {
    /// A sync's claims: `local_items` are the project's own, and `facts` with the lock it read
    /// say which paths are the project's.
    pub(crate) fn new(
        local_items: &[crate::local_source::LocalDiscoveredItem],
        layout: &'a Layout,
        facts: &'a Facts,
        old_lock: &'a LockFile,
        provided: &'a crate::provided::ProvidedNames,
        managed_targets: &'a [String],
    ) -> Self {
        use crate::local_source::LocalItemContent;
        let mut names = HashSet::new();
        let mcp_targets = crate::sync::target::mcp_config_targets(managed_targets);
        for item in local_items {
            match &item.content {
                LocalItemContent::Hook(hook) => {
                    for target in hook.def.select(managed_targets) {
                        names.insert((ItemKind::Hook, target.to_string(), hook.def.name.clone()));
                    }
                }
                LocalItemContent::Mcp(mcp) => {
                    for target in &mcp_targets {
                        if mcp.def.targets.is_empty() || mcp.def.targets.contains(target) {
                            names.insert((ItemKind::McpServer, target.clone(), mcp.name.clone()));
                        }
                    }
                }
                _ => {
                    names.insert((
                        item.discovered.id.kind,
                        CANONICAL_TARGET_ROOT.to_string(),
                        item.discovered.id.name.to_string(),
                    ));
                }
            }
        }
        Self {
            names,
            disk: Some(DiskClaims {
                layout,
                facts,
                recorded: LockIndex::new(old_lock),
                owns: crate::sync::ownership::Ownership::new(old_lock, provided),
                managed_targets,
            }),
        }
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &ClaimKey> {
        self.names.iter()
    }

    /// Why the project claims what the machine-local item of `source` at `dest` would install,
    /// if it does by a path: the first of its outputs whose path the project holds.
    pub(crate) fn path_claim(
        &self,
        kind: ItemKind,
        dest: &DestPath,
        source: &str,
    ) -> Option<String> {
        let disk = self.disk.as_ref()?;
        outputs(disk.layout, kind, dest, source, disk.managed_targets)
            .into_iter()
            .find(|output| {
                let recorded = if output.target == CANONICAL_TARGET_ROOT {
                    disk.owns.owns(dest)
                } else {
                    DestPath::new(&output.relative).is_ok_and(|relative| {
                        disk.recorded
                            .find_output(&output.target, &relative)
                            .is_some()
                    })
                };
                disk.facts.claims(&output.path, recorded)
            })
            .and_then(|output| disk.facts.relative(&output.path))
    }
}

/// One place an item writes: its target, the path relative to it, and the absolute path.
struct Output {
    target: String,
    relative: String,
    path: PathBuf,
}

/// Every place the item of `source` at canonical `dest` writes: its store copy, and its copy in
/// each managed target the source is offered to, an agent's native file included.
fn outputs(
    layout: &Layout,
    kind: ItemKind,
    dest: &DestPath,
    source: &str,
    managed_targets: &[String],
) -> Vec<Output> {
    let mut outputs = vec![Output {
        target: CANONICAL_TARGET_ROOT.to_string(),
        relative: dest.to_string(),
        path: dest.resolve(layout.store()),
    }];
    for target in managed_targets {
        if !crate::layer::offered_to(source, target) {
            continue;
        }
        let relative = match PackageDir::parse_canonical(kind, dest) {
            Some(dir) if dir.belongs_to(target) => dir.native(),
            Some(_) => continue,
            None => dest.to_string(),
        };
        let mut push = |relative: String| {
            outputs.push(Output {
                path: layout.in_target(target, &relative),
                target: target.clone(),
                relative,
            })
        };
        push(relative);
        if kind == ItemKind::Agent
            && let Some(harness) = crate::harness::registry::harness_for_target(target)
        {
            let extension = crate::compiler::lowering::lowering(harness)
                .agent
                .extension();
            push(format!("agents/{}.{extension}", dest.item_name(kind)));
        }
    }
    outputs
}

/// What a sync's plan writes for this machine only, decided before anything is written.
#[derive(Debug, Default)]
pub(crate) struct Decision {
    /// Every path to list in the exclude block.
    pub exclude: BTreeSet<PathBuf>,
    /// The lock keys of the hook and MCP directories whose config entry is not written.
    withheld: HashSet<String>,
    /// The withheld entries, for the lock.
    pub records: Vec<crate::lock::YieldedRecord>,
}

impl Decision {
    pub(crate) fn withholds(&self, dir: &PackageDir) -> bool {
        self.withheld.contains(&dir.lock_key())
    }
}

/// An entry the plan puts in a shared file.
struct Entry<'a> {
    machine_local: bool,
    /// The machine-local entry's directory, source and name, for withholding it.
    of: Option<(&'a PackageDir, &'a str, &'a str, &'static str)>,
}

/// Decide, for `target`'s plan, what is excluded and which machine-local entries are withheld,
/// warning once per withheld record. `project_targets` are the targets `mars.toml` names: what
/// serves any other target is machine-local whatever its source.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide<'t>(
    layout: &Layout,
    facts: &Facts,
    target: &'t TargetState,
    rendered: &RenderedHooks,
    managed_targets: &[String],
    project_targets: &BTreeSet<String>,
    old_lock: &LockFile,
    diag: &mut DiagnosticCollector,
) -> Decision {
    let machine_local = |source: &str, target_root: &str| {
        crate::layer::is_machine_local(source) || !project_targets.contains(target_root)
    };
    let mut decision = Decision::default();
    for item in target.items.values() {
        for output in outputs(
            layout,
            item.id.kind,
            &item.dest_path,
            &item.source_name,
            managed_targets,
        ) {
            if machine_local(&item.source_name, &output.target) {
                decision.exclude.insert(output.path);
            }
        }
    }

    // Each shared file of each target, with the entries the plan puts in it.
    let mut files: BTreeMap<PathBuf, Vec<Entry<'t>>> = BTreeMap::new();
    for target_root in managed_targets {
        let by_prefix = config_files(layout, target_root);
        let mut add = |prefix: &str, entry: Entry<'t>| {
            if let Some((path, _)) = by_prefix
                .iter()
                .find(|(_, prefixes)| prefixes.contains(&prefix))
            {
                files.entry(path.clone()).or_default().push(entry);
            }
        };
        for (dir, item) in target.selected_mcp_for_target(target_root) {
            add(
                MCP_KIND.key_prefix,
                Entry {
                    machine_local: machine_local(&item.source_name, target_root),
                    of: Some((dir, &item.source_name, &item.name, "MCP server")),
                },
            );
        }
        if target.mcp_scan_failed() {
            add(
                MCP_KIND.key_prefix,
                Entry {
                    machine_local: false,
                    of: None,
                },
            );
        }
        for (dir, item) in target.selected_hooks_for_target(target_root) {
            let local = machine_local(&item.source_name, target_root);
            let prefix = match rendered.get(dir) {
                HookOutput::Merge(_) => HOOK_KEY_PREFIX,
                HookOutput::Keyed(output) => {
                    if local {
                        decision.exclude.extend(
                            output
                                .files
                                .iter()
                                .map(|file| layout.in_target(target_root, &file.path)),
                        );
                    }
                    HOOK_ENTRY_KIND.key_prefix
                }
                HookOutput::File(output) => {
                    if local {
                        decision.exclude.extend(
                            output
                                .files
                                .iter()
                                .map(|file| layout.in_target(target_root, &file.path)),
                        );
                    }
                    continue;
                }
            };
            add(
                prefix,
                Entry {
                    machine_local: local,
                    of: Some((dir, &item.source_name, &item.def.name, "hook")),
                },
            );
        }
        for kept in target.kept_hooks() {
            if kept.dirs.iter().any(|dir| dir.belongs_to(target_root)) {
                for prefix in [HOOK_KEY_PREFIX, HOOK_ENTRY_KIND.key_prefix] {
                    add(
                        prefix,
                        Entry {
                            machine_local: false,
                            of: None,
                        },
                    );
                }
            }
        }
        if let Some(spec) = crate::target::spec(target_root) {
            for (kind, _) in spec.fixed_entries(layout) {
                add(
                    kind.key_prefix,
                    Entry {
                        machine_local: !project_targets.contains(target_root),
                        of: None,
                    },
                );
            }
        }
    }

    // A file Mars has written machine-local entries to before is its own to keep writing: the
    // block lists it, or (from a build before the block) the lock records only such entries in it.
    let (_, local_lock) = crate::lock::machine::split(old_lock.clone(), project_targets);
    let recorded_local = locked_files(layout, &local_lock);
    let foreign: Vec<&PathBuf> = files
        .keys()
        .filter(|path| {
            path.symlink_metadata().is_ok()
                && !facts.listed.contains(*path)
                && !recorded_local.contains(*path)
        })
        .collect();
    let ignored = ignored(facts, &foreign);

    for (path, entries) in &files {
        if !entries.iter().any(|entry| entry.machine_local) {
            continue;
        }
        let file = facts
            .relative(path)
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let why = if facts.tracked(path) {
            Some(format!(
                "`{file}` is committed, and an entry in it would be too"
            ))
        } else if entries.iter().any(|entry| !entry.machine_local) {
            Some(format!(
                "`{file}` holds the project's entries too, so it can be committed with them"
            ))
        } else if foreign.contains(&path) && !ignored.contains(path) {
            Some(format!(
                "`{file}` is not Mars's and git does not ignore it, so it can be committed with the entry; add it to `.gitignore` to install it"
            ))
        } else {
            None
        };
        let Some(why) = why else {
            decision.exclude.insert(path.clone());
            continue;
        };
        for (dir, source, name, noun) in entries.iter().filter_map(|entry| {
            entry
                .of
                .filter(|(_, source, _, _)| crate::layer::is_machine_local(source))
        }) {
            if !decision.withheld.insert(dir.lock_key()) {
                continue;
            }
            leave_out(
                &mut decision.records,
                diag,
                "machine-local-entry-withheld",
                (dir.kind(), dir.target().to_string(), name.to_string()),
                source,
                format!("{noun} `{name}`"),
                why.clone(),
            );
        }
    }
    decision
}

/// Joins a source's label and the reason in a [`leave_out`] group's subject.
const CAUSE: char = '\u{1f}';

fn describe_left_out(subject: &str, count: usize, names: &str) -> String {
    let (source, why) = subject.split_once(CAUSE).unwrap_or((subject, ""));
    if count == 1 {
        format!("{names} from {source} is not installed here: {why}")
    } else {
        format!("{count} items from {source} are not installed here: {names}. For each: {why}")
    }
}

/// Which of `paths` git ignores.
fn ignored(facts: &Facts, paths: &[&PathBuf]) -> BTreeSet<PathBuf> {
    let specs: BTreeSet<String> = paths
        .iter()
        .filter_map(|path| facts.relative(path))
        .collect();
    if specs.is_empty() || facts.root.as_os_str().is_empty() {
        return BTreeSet::new();
    }
    git_lines(
        &facts.root,
        &["check-ignore", "-z", "--no-index", "--"],
        &specs,
    )
    .into_iter()
    .map(|relative| facts.root.join(relative))
    .collect()
}

/// The shared files a target's config entries go in, each with the lock-key prefixes of the
/// entries it holds.
fn config_files(layout: &Layout, target_root: &str) -> Vec<(PathBuf, Vec<&'static str>)> {
    let mut files: Vec<(PathBuf, Vec<&'static str>)> = Vec::new();
    let mut add = |path: PathBuf, prefix: &'static str| match files
        .iter_mut()
        .find(|(held, _)| *held == path)
    {
        Some((_, prefixes)) => prefixes.push(prefix),
        None => files.push((path, vec![prefix])),
    };
    for (kind, section) in crate::target::keyed_sections(target_root) {
        add(layout.file(section.path), kind.key_prefix);
    }
    if let Some((file, _)) = crate::compiler::config_entries::hook_file(target_root) {
        add(layout.in_target(target_root, file), HOOK_KEY_PREFIX);
    }
    files
}

/// The shared files `lock` records any config entry in.
fn locked_files(layout: &Layout, lock: &LockFile) -> BTreeSet<PathBuf> {
    lock.config_entries
        .iter()
        .flat_map(|(target_root, records)| {
            let files = config_files(layout, target_root);
            records
                .keys()
                .filter_map(move |key| {
                    files
                        .iter()
                        .find(|(_, prefixes)| prefixes.iter().any(|prefix| key.starts_with(prefix)))
                        .map(|(path, _)| path.clone())
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// What a published `lock` says the exclude block holds (`exclude::settle`): every output of its
/// machine-local half, the shared files only machine-local entries are in, and, to check against
/// the other work trees' blocks, every path of its committed half.
pub(crate) fn settled(
    layout: &Layout,
    lock: &LockFile,
    project_targets: &BTreeSet<String>,
) -> crate::sync::exclude::Settled {
    let (committed, local) = crate::lock::machine::split(lock.clone(), project_targets);
    let paths = |lock: &LockFile| -> BTreeSet<PathBuf> {
        lock.items
            .values()
            .flat_map(|item| &item.outputs)
            .map(|output| {
                output
                    .dest_path
                    .resolve(&layout.target(&output.target_root))
            })
            .collect()
    };
    let committed_files = locked_files(layout, &committed);
    // The store is the project's own record, not what it commits: `mars init` ignores it.
    let mut project: BTreeSet<PathBuf> = paths(&committed)
        .into_iter()
        .filter(|path| !path.starts_with(layout.store()))
        .collect();
    project.extend(committed_files.iter().cloned());
    crate::sync::exclude::Settled {
        machine: paths(&local),
        files: locked_files(layout, &local)
            .into_iter()
            .filter(|file| !committed_files.contains(file))
            .collect(),
        project,
    }
}

/// Record that `source`'s `kind` `name` on `target` is left out for the project's sake, and warn
/// once: the record is news (`RecordRef::Yielded`) when it is new or its reason changed. What
/// one source leaves out for one reason is one line.
pub(crate) fn leave_out(
    records: &mut Vec<crate::lock::YieldedRecord>,
    diag: &mut DiagnosticCollector,
    code: &'static str,
    (kind, target, name): ClaimKey,
    source: &str,
    what: String,
    why: String,
) {
    diag.hold_each(
        RecordRef::Yielded {
            kind,
            target: target.clone(),
            name: name.clone(),
            source: source.to_string(),
        },
        crate::diagnostic::ItemGroup {
            code,
            subject: &format!("{}{CAUSE}{why}", crate::layer::source_label(source)),
            describe: describe_left_out,
        },
        what,
    );
    records.push(crate::lock::YieldedRecord {
        kind,
        target,
        name,
        source: source.into(),
        why,
    });
}
