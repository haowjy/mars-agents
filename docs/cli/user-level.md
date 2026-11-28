# The user level

The user level holds the agents, skills, hooks and MCP servers you install for yourself rather
than for one project. Mars treats it as a root of its own, with a manifest, a lock and a store,
but it has no targets. A user-level sync writes only its store and its lock and never touches
`~/.claude`, `~/.codex` or any other harness folder. Every project's sync installs it into that
project, kept out of the project's commits ([In a project](#in-a-project)).

## Where its files are

The user level is one folder, `~/.mars`, on Linux, macOS and Windows. `MARS_HOME` moves it; a
relative `MARS_HOME` is taken from the working directory, and a relative `HOME` is ignored.

| File | What it is |
|---|---|
| `~/.mars/mars.toml` | The user level's manifest: a project `mars.toml` (`[dependencies]`, `[local-dependencies]`, `[settings]`, …) with no `targets` |
| `~/.mars/mars.lock` | Its lock, in the same format as a project's |
| `~/.mars/.mars-src/` | Your own items, written by hand, as in a project's `.mars-src/` |
| `~/.mars/config.toml` | The settings layer beneath every root, the user level included ([User config](../config/mars-toml.md#user-config)) |
| `~/.mars/curated.toml` | The user tier of `mars models list` curation ([Curated model rules](../config/mars-curated.md)) |
| `~/.mars/skills/`, `agents/`, … | The store: what a project keeps in `.mars/`, directly in the folder |

The source cache stays where it was (`MARS_CACHE_DIR`, else the platform cache folder).

**From Mars 0.40.** `MARS_CONFIG_DIR` is read as `MARS_HOME` when that is unset, with a
warning to rename it; when both are set, `MARS_HOME` wins and the warning says to unset the old
one. Mars 0.40 kept `config.toml` and `curated.toml` in the platform config folder
(`$XDG_CONFIG_HOME/mars` or `~/.config/mars` on Linux, `~/Library/Application Support/mars` on
macOS). While `~/.mars` has no copy of a file and the old folder does, Mars reads the old one and
warns to move it; when both exist, `~/.mars` wins and the warning says the old one is ignored.
This holds only for the default folder: with `MARS_HOME` or `MARS_CONFIG_DIR` set, Mars reads
the files from that folder alone. The next minor release reads neither old name.

No `init` step exists. With neither `mars.toml` nor `.mars-src/` the user level is empty. A
`.mars-src/` alone is enough: `mars --user sync` installs its items and writes `mars.lock`, and
`mars --user add` creates `mars.toml` when a dependency is added. A `targets` or `managed_root` key in the user `mars.toml` or `mars.local.toml`
is an error.

## Choosing the root

- `--user` uses the user level. It cannot be combined with `--root`.
- Inside `~/.mars` (or below it), every command uses the user level, as if `--user` were given;
  a command `--user` refuses is refused there too.
- Commands that only read installed items (`build launch-bundle`, `agents`, `skills`,
  `models` other than `models alias`, `list`, `why`) use the nearest project, and the user level when no project
  encloses the working directory.
- Commands that write (`sync`, `add`, `remove`, `upgrade`, `models alias`, …) use the nearest project. With
  none they fail and name `--user`: a write to the user level is always asked for. Under
  `--json` the error code is `project_not_found`.
- `--user` works with `sync`, `add`, `remove`, `upgrade`, `outdated`, `list`, `why`,
  `rename`, `override`, `adopt`, `repair`, `doctor`, `validate`, `agents`, `skills`,
  `models`, `resolve`, `export` and `build launch-bundle`. It is refused, with the reason, by
  `init`, `link`, `unlink` (the user level has no targets), `provide` (a host provides to
  projects) and `version`, `check` and `build plugin` (they author a package).

The user level never restricts routing: it has no `targets`, so every harness is allowed.

Every `--json` output that is an object carries the root it used:

```json
{"root": {"kind": "project", "path": "/home/me/repo"}}
{"root": {"kind": "user", "path": "/home/me/.mars"}}
```

`mars outdated --json` and `mars list --status --json` print a top-level array, which has no
place for the key; their shape is unchanged.

## In a project

Each `mars sync` in a project installs the user level beside the project's own packages, into
the project's targets:

- One dependency `_user-<name>` per entry of the user `[dependencies]`, at the exact version
  the user `mars.lock` records, read from the shared source cache. A project sync never
  resolves the user level again, so `mars upgrade` in a project leaves it alone: `mars --user
  upgrade` moves it, and each project installs the change on its next sync. A user
  dependency's own dependencies come in as `_user-<name>` too, so they never merge with the
  project's.
- `_user-self`: your `~/.mars/.mars-src/`.
- Hooks and MCP servers come in as well. The user store holds only agents and skills (it has
  no targets), so a project installs those from the source.

A user level that has never been synced, or whose lock or manifest Mars cannot read, adds
nothing, and the sync warns with what to run. So does a user dependency whose manifest entry
changed since the last `mars --user sync`. A user dependency whose pinned commit is not in the
cache is fetched at that commit; offline, the project sync fails and changes nothing.

**The project wins.** An agent, skill, hook or MCP server the project's packages or
`.mars-src` provide is not installed from the user level, even when the project's copies were
renamed on a collision (`reviewer__a`, `reviewer__b`): a launch of `reviewer` then fails and
lists the renamed names, instead of running your copy. Neither is one whose path in a target
(`.claude/skills/foo`) holds something git tracks or Mars did not install; `--force` does not
adopt over it. The sync warns once, when the record is new, one line per source and reason:
`2 items from upkg (user level) are not installed here: skill `foo`, agent `bar`. For each: the
project also provides it`. `mars list` shows them under NOT INSTALLED HERE (`yielded` in
`--json`), and `mars why <name>` gives the reason. Within the user level, a collision renames
every side, as in a project, except your own `.mars-src` item, which keeps its name.

**Records stay on this machine.** Everything from the user level is recorded in
`.mars/machine.lock`, never in `mars.lock`, including what it did not install and why. Remove an item from the user level and the next
project sync removes it from the project.

**Commits stay clean.** In a git work tree, the paths Mars writes for user-level items (and
for a host's content and for targets only `mars.local.toml` names) are listed in a block Mars
owns in the repository's `info/exclude`, so `git add -A` leaves them out. That includes a
config file (`.mcp.json`, `.codex/hooks.json`, …) that holds only user-level entries, and,
unless the project ignores `.mars/`, the user level's staged copy (`.mars/staging/_user-*`) and
`.mars/machine.lock`. A config file, staged copy or machine lock that your own ignore rules
already cover is not listed. Each work tree of a repository has its own block, keyed by its path; lines outside the
blocks are yours and stay.

A user-level hook or MCP server (or one serving a target only `mars.local.toml` names) is not
installed, with a warning once, when its entry would go in a config file that could be
committed: one entry of a file cannot be kept out of a commit. Each config file is judged by
what the sync puts in it, in this order:

1. A file git tracks: the entry is not installed.
2. A file git ignores: the entry is installed, whatever else the file holds.
3. A file that also gets the project's own entries: the entry is not installed.
4. A file that exists, that Mars has no record of writing and that the exclude block does not
   list: someone else's, so the entry is not installed. Add the file to `.gitignore` to install
   it.
5. Otherwise the file is Mars's: the entry is installed and the file is listed in the block.

Nothing of a withheld hook or MCP server is installed, its copy under `.mars/` and the target
included, and `mars why` names the reason. Without git, nothing can be committed, so every
entry is installed.

Git reads one `info/exclude` for all work trees of a repository, so a path one work tree's
block lists is hidden from `git add` in every other work tree too. When another work tree's
block lists a path this project holds, the sync warns once, naming the path and that work
tree; `git add -f` adds it. Without git there is nothing to write.

A directory outside every project installs nothing; lookups there read the user store. A
lookup serves only what the user `mars.lock` records: a file in `~/.mars` it does not record (one
an old home project left there, say) is not the user level's.

With no absolute `HOME` and no `MARS_HOME`, the user folder cannot be located: project commands
run with no user config and no user level (an `info` note under `--verbose`), and `--user` fails.
A lookup outside a project then has no user level to read: it runs in the working directory, finds
nothing installed there, and reports `{"kind": "project", "path": "<working directory>"}` as its
root.

## The home directory is never a project

The walk-up for `mars.toml` skips your home directory, including when it starts there, so
`cd ~ && mars sync` does not open `~/mars.toml`. `mars init` and the automatic init of
`mars add` and `mars link` refuse the home directory and `~/.mars` and name `--user`, and a
command run from home that finds no project names only `--user`.

### Moving a home project

Before the user level, a `~/mars.toml` was how people installed for themselves. Its store is
`~/.mars/`, the folder the user level now uses. Mars no longer opens it by walk-up.

While `~/mars.toml` or `~/mars.lock` exists, home holds that project and `~/.mars` holds its
files, so the user level is not written there: every write with `--user` (`sync`, `add`,
`remove`, `upgrade`, …) fails, and a project's sync installs no user level. Any command that
meets the home project (a walk-up that passes home, `--root ~`, a project sync, `--user doctor`,
a refused write) ends with one `home-project` warning listing the steps still left, after the
command has run; under `--json` it is in `diagnostics`. To move it:

1. Run `mars --root ~ unlink <target>` for each target `~/mars.lock` records, which removes what
   Mars installed there. This works from the lock alone if `~/mars.toml` is already gone.
2. Delete `~/mars.lock` and the old store's files in `~/.mars/` (all generated; the warning names
   them, and keeps any `config.toml`, `curated.toml`, `mars.toml` and `.mars-src/` you already
   moved in).
3. Move `~/mars.toml` to `~/.mars/mars.toml` and delete its `targets` line (or merge it into the
   `~/.mars/mars.toml` you have). A path dependency written relative to `~` must be rewritten
   relative to `~/.mars`, or as an absolute path.
4. Move `~/.mars-src/`, if you have one, to `~/.mars/.mars-src/`.
5. Run `mars --user sync`.

A file in `~/.mars` that the user `mars.lock` does not record is not the user level's: a user
sync does not delete it, and replaces it only with `mars --user sync --force` (without, it warns
that the path is taken, as in a project); lookups do not serve it, and `mars --user doctor` lists
it. The user level reads no `machine.lock` or `provided.lock` there.

For this minor release, `--root` naming the home directory exactly still opens it, but only to
read it and to `unlink` its targets: any other write (`sync`, `add`, `link`, `repair`, …) is
refused, since it could adopt the user level's files in `~/.mars` and later delete them as its
own. The next minor release refuses `--root ~` entirely.

## `--fallback-root`
`mars agents`, `mars skills` and `mars build launch-bundle` still accept `--fallback-root`, a
second project consulted for items the root lacks, and warn that it is deprecated. It goes in
the next minor release: install those items at the user level instead.
