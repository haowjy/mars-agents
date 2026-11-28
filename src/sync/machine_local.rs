//! The one decision for what a sync writes on behalf of this machine only: the content of a
//! layer other than the project's (`crate::layer`: the user level's `_user-*`, a host's
//! `_provided-*`), and whatever serves a target only `mars.local.toml` names. None of it may reach
//! a commit, and none of it may take a path the project holds. For every such output (an item's
//! folder or file, an entry in a shared config file, a file made only to hold such entries) the
//! sync decides, before it plans or writes anything, exactly one of:
//!
//! - **yield**: the project claims the name or the path. Its own `.mars-src` items, its
//!   dependencies (renamed or not), and any path at the output's location that git tracks or Mars
//!   did not install, are rank-1 claims in the claim rule (`layer::yielded`, applied in
//!   `sync::target`), so the item is not installed and `--force` never adopts over the path.
//!   [`ProjectClaims`] is what the claim rule reads.
//! - **withhold**: the entry would go into a shared file that can be committed: git tracks it,
//!   or, when git does not ignore it, the project's own entries go in it too or it is someone
//!   else's. One entry of a file cannot be kept out of a commit, so neither it nor its item is
//!   written. [`decide`].
//! - **install and exclude**: everything it creates that git does not already ignore is listed
//!   in this work tree's block of git's `info/exclude` (`sync::exclude`) before it is written,
//!   the layers' staged copies and the machine lock included. [`decide`].
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
    /// Whether the root is in a git work tree; without one nothing can be committed.
    git: bool,
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
        let tracked = git_fields(&root, &["ls-files", "-z"], &specs, Specs::Args)
            .into_iter()
            .collect();
        Self {
            listed: crate::sync::exclude::listed(layout),
            root,
            git: true,
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

/// How [`git_fields`] hands git its paths.
#[derive(Clone, Copy)]
enum Specs {
    /// After `--`, as `ls-files` takes them.
    Args,
    /// NUL-separated on standard input, as `check-ignore --stdin -z` takes them.
    Stdin,
}

/// The NUL-separated fields `git <args>` prints run in `root` for `specs`; none when git fails or
/// is missing, which counts as nothing tracked or ignored.
fn git_fields(root: &Path, args: &[&str], specs: &BTreeSet<String>, how: Specs) -> Vec<String> {
    use std::io::Write;
    if specs.is_empty() {
        return Vec::new();
    }
    let mut command = std::process::Command::new("git");
    command
        .args(args)
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    if let Specs::Args = how {
        command.arg("--").args(specs);
    }
    let Ok(mut child) = command.spawn() else {
        return Vec::new();
    };
    let input: Vec<u8> = match how {
        Specs::Args => Vec::new(),
        Specs::Stdin => specs
            .iter()
            .flat_map(|spec| spec.bytes().chain(std::iter::once(0)))
            .collect(),
    };
    // `check-ignore --stdin` prints as it reads: write from another thread, so a full output
    // pipe cannot stall the write.
    let writer = child.stdin.take().map(|mut stdin| {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        })
    });
    let output = child.wait_with_output();
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let Ok(output) = output else {
        return Vec::new();
    };
    // `check-ignore` exits 1 when nothing is ignored, with nothing printed.
    let mut fields: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .map(str::to_string)
        .collect();
    fields.pop();
    fields
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

/// An entry the plan puts in a shared file.
struct Entry {
    machine_local: bool,
    /// The entry's directory, source, name and noun, for withholding it; none for an entry no
    /// item owns (a target's fixed entries, the records a failed scan keeps).
    of: Option<(PackageDir, String, String, &'static str)>,
}

/// Decide, before `target` is planned, which machine-local hook and MCP entries are withheld,
/// and return every path to list in the exclude block. A withheld entry's item is dropped from `target` with it, recorded, and
/// warned once. `project_targets` are the targets `mars.toml` names: what serves any other
/// target is machine-local whatever its source. `sources` are the sync's resolved sources, whose
/// staged copies are in the store.
///
/// A shared file is judged by what this sync puts in it and by git, in this order: one git
/// tracks withholds; one git ignores takes every entry and needs no exclude line; one that also
/// gets the project's entries withholds; one that exists, that this work tree's block does not
/// list and that the old lock records nothing in is someone else's and withholds. Otherwise the
/// file is Mars's, gets the entries, and is excluded.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide<'s>(
    layout: &Layout,
    facts: &Facts,
    target: &mut TargetState,
    rendered: &RenderedHooks,
    managed_targets: &[String],
    project_targets: &BTreeSet<String>,
    sources: impl Iterator<Item = &'s str>,
    old_lock: &LockFile,
    diag: &mut DiagnosticCollector,
) -> BTreeSet<PathBuf> {
    let machine_local = |source: &str, target_root: &str| {
        crate::layer::is_machine_local(source) || !project_targets.contains(target_root)
    };

    // Each shared file of each target, with the entries the plan puts in it, and the files a
    // machine-local keyed or file hook writes beside its entry.
    let mut files: BTreeMap<PathBuf, Vec<Entry>> = BTreeMap::new();
    let mut hook_files: Vec<(String, Vec<PathBuf>)> = Vec::new();
    for target_root in managed_targets {
        let by_prefix = config_files(layout, target_root);
        let mut add = |prefix: &str, entry: Entry| {
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
                    of: Some((
                        dir.clone(),
                        item.source_name.clone(),
                        item.name.clone(),
                        "MCP server",
                    )),
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
            let (prefix, written) = match rendered.get(dir) {
                HookOutput::Merge(_) => (Some(HOOK_KEY_PREFIX), Vec::new()),
                HookOutput::Keyed(output) => {
                    (Some(HOOK_ENTRY_KIND.key_prefix), output.files.clone())
                }
                HookOutput::File(output) => (None, output.files.clone()),
            };
            if local && !written.is_empty() {
                hook_files.push((
                    dir.lock_key(),
                    written
                        .iter()
                        .map(|file| layout.in_target(target_root, &file.path))
                        .collect(),
                ));
            }
            if let Some(prefix) = prefix {
                add(
                    prefix,
                    Entry {
                        machine_local: local,
                        of: Some((
                            dir.clone(),
                            item.source_name.clone(),
                            item.def.name.clone(),
                            "hook",
                        )),
                    },
                );
            }
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
    files.retain(|_, entries| entries.iter().any(|entry| entry.machine_local));

    // A layer's staged copy, and the lock of what this machine installs, are in the store.
    let mut store: Vec<PathBuf> = sources
        .filter(|source| crate::layer::is_machine_local(source))
        .map(|source| layout.store().join("staging").join(source))
        .collect();
    store.extend(layout.machine_lock());

    let candidates: Vec<&PathBuf> = files
        .keys()
        .chain(&store)
        .filter(|path| !facts.tracked(path))
        .collect();
    let ignored = ignored(layout, facts, &candidates);
    // Any record of the old lock makes a file Mars's: it wrote there, and what it writes now
    // replaces what it wrote.
    let recorded = locked_files(layout, old_lock);

    let mut exclude = BTreeSet::new();
    let mut withheld: Vec<PackageDir> = Vec::new();
    let mut records = Vec::new();
    for (path, entries) in &files {
        let file = facts
            .relative(path)
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let why = if facts.tracked(path) {
            format!("`{file}` is committed, and an entry in it would be too")
        } else if ignored.contains(path) {
            continue;
        } else if entries.iter().any(|entry| !entry.machine_local) {
            format!("`{file}` holds the project's entries too, so it can be committed with them")
        } else if path.symlink_metadata().is_ok()
            && !facts.listed.contains(path)
            && !recorded.contains(path)
        {
            format!(
                "`{file}` is not Mars's and git does not ignore it, so it can be committed with the entry; add it to `.gitignore` to install it"
            )
        } else {
            exclude.insert(path.clone());
            continue;
        };
        for (dir, source, name, noun) in entries
            .iter()
            .filter(|entry| entry.machine_local)
            .filter_map(|entry| entry.of.as_ref())
        {
            if withheld
                .iter()
                .any(|held| held.lock_key() == dir.lock_key())
            {
                continue;
            }
            withheld.push(dir.clone());
            leave_out(
                &mut records,
                diag,
                "machine-local-entry-withheld",
                (dir.kind(), dir.target().to_string(), name.clone()),
                source,
                format!("{noun} `{name}`"),
                why.clone(),
            );
        }
    }
    target.withhold(&withheld);
    target.record_yielded(records);

    for item in target.items.values() {
        for output in outputs(
            layout,
            item.id.kind,
            &item.dest_path,
            &item.source_name,
            managed_targets,
        ) {
            if machine_local(&item.source_name, &output.target) {
                exclude.insert(output.path);
            }
        }
    }
    for (key, paths) in hook_files {
        if !withheld.iter().any(|dir| dir.lock_key() == key) {
            exclude.extend(paths);
        }
    }
    // The machine lock holds whatever this sync installs or leaves out for this machine only.
    if store.len() == 1
        && exclude.is_empty()
        && withheld.is_empty()
        && target.yielded_items().is_empty()
    {
        store.pop();
    }
    exclude.extend(store.into_iter().filter(|path| !ignored.contains(path)));
    exclude
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

/// Which of `paths` the user has git ignore: all of them outside a work tree, where none can be
/// committed. A match in a Mars block of `info/exclude` is Mars's listing, not the user's choice.
fn ignored(layout: &Layout, facts: &Facts, paths: &[&PathBuf]) -> BTreeSet<PathBuf> {
    if !facts.git {
        return paths.iter().map(|path| (*path).clone()).collect();
    }
    let specs: BTreeSet<String> = paths
        .iter()
        .filter_map(|path| facts.relative(path))
        .collect();
    let blocks = crate::sync::exclude::block_lines(layout);
    // `-v` prints source, line number, pattern and path for each ignored path.
    let fields = git_fields(
        &facts.root,
        &["check-ignore", "-z", "-v", "--stdin", "--no-index"],
        &specs,
        Specs::Stdin,
    );
    let (matches, _) = fields.as_chunks::<4>();
    matches
        .iter()
        .filter(|[source, line, _, _]| {
            let Some((exclude, lines)) = &blocks else {
                return true;
            };
            let in_block = line.parse().is_ok_and(|line| lines.contains(&line));
            !(in_block
                && dunce::canonicalize(facts.root.join(source))
                    .is_ok_and(|source| source == *exclude))
        })
        .map(|[_, _, _, path]| facts.root.join(path))
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
    // Whether the project commits its store is its own choice, not a path to check.
    let mut project: BTreeSet<PathBuf> = paths(&committed)
        .into_iter()
        .filter(|path| !path.starts_with(layout.store()))
        .collect();
    project.extend(committed_files.iter().cloned());
    let mut files: BTreeSet<PathBuf> = locked_files(layout, &local)
        .into_iter()
        .filter(|file| !committed_files.contains(file))
        .collect();
    // A layer's staged copy and the machine lock: listed while listed, since whether git ignores
    // them was decided before the sync wrote (`decide`).
    files.extend(
        local
            .dependencies
            .keys()
            .filter(|source| crate::layer::is_machine_local(source))
            .map(|source| layout.store().join("staging").join(source.as_str())),
    );
    files.extend(layout.machine_lock().filter(|path| path.exists()));
    crate::sync::exclude::Settled {
        machine: paths(&local),
        files,
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
