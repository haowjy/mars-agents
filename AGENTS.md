# mars-agents

Mars installs agent profiles and skills from git sources into each harness's own
directories (`.claude/`, `.codex/`, `.opencode/`, `.cursor/`, `.pi/`, `.agy/`), through a canonical
store in `.mars/`.

## Who It's For

- **The person installing packages.** Their project mixes hand-written agents, skills,
  hooks and MCP servers with installed ones, in the same directories.
- **The package author.** Their profile should do what it says in every harness, or they
  need to know what didn't carry over.
- **Meridian and the harnesses,** which read `.mars/` and the target files. The package
  format and what a sync writes are contracts they build on.

## Engineering

This codebase is yours: how it's built is your call, and you'll keep building on it.
Product decisions are Jimmy's.

**Choose the simpler system, not the smaller change.** When a case doesn't fit, change the
design so it does instead of adding a branch, flag or special case. Judge a choice by the
code it leaves: a bigger change, or more deletion, is the right one when less is left to
understand. Fix structural debt in the same PR, whatever its size; a found fix too large for
the PR gets its own PR now.

**Fix the root cause.** Gather the problems a probe, a review or Jimmy finds, trace each to
its cause, and fix the cause: no guard, retry or special case where the problem showed up.
Several problems often share one cause. A rare edge case with no fix at its cause can be left
as it is.

## Errors and Warnings

A problem is built as a `Diagnostic` and nothing else; its level says what the user loses if
they ignore it, and the code that finds the problem states it.

- `Error`: Mars cannot use this input, and the command fails.
- `Warning`: the user loses something (a restriction not applied, a file not written, an item
  not installed).
- `Info`: nothing is lost, and it may be intended. Printed only when asked: `mars validate`,
  `mars check` and `--verbose` ask; a default `mars sync` and a launch bundle do not.

Two things make a problem a `Warning`, and either is enough. **Behaviour differs:** what is
installed does less than the package declares (a restriction a target cannot carry, a hook or
item Mars could not install, an MCP key it cannot set, an agent's missing skill). Whoever wrote
the item, a loss is a `Warning`: the person running agents is the one who gets the behaviour, so
a dependency's loss is not `Info`. **The user can act:** the project's own item that is wrong but
changes nothing yet. Only when behaviour is unchanged does the owner decide (`Owner` in
`diagnostic.rs`): schema advice about a dependency's item is `Info` to the installer, because only
its author can fix it, and a `Warning` to the author in `mars check` and `mars validate`.

A loss is news once: it is reported by the sync that makes the lock record it concerns new or
different, and an unchanged sync is silent. Each loss names its record (`lock::RecordRef`: an
item, an output, a config entry, or every item of a source) when it is found, and
`DiagnosticCollector::settle` decides all of them in `finalize`, by comparing the lock the sync
read with the one it wrote: there is no other release rule. `--verbose`, `mars check` and
`mars validate` list every loss each time. A hook that renders on no managed target writes
nothing; its record is the lock item `hook/<name>` with no outputs, which `LockFile::finish`
keeps because the sync resolved it. A sync that fails before it writes the lock has changed no
record, so the next sync reports the losses it held. A text meant for an installer names the installer's own fix (`mars upgrade
<source>`, or set the variable). One cause is one line: findings that arise once per item share a
`warn_each` group, and what follows from an item that was not installed is not reported again.

`Diagnostic::shown` and `output::print_diagnostics` are the whole policy: no reader prints, and
no reader decides whether its problem is shown. Reducing what users see means changing a site
from `Warning` to `Info`, or deleting it.

Mars has no code about a key it does not read: no per-key advice, no deprecation note for a key
that is gone. What happens to one is what its table does with any key it has no use for
(`crates/mars-schema/AGENTS.md`). The one exception is `mars_schema::retired`, a single table of
keys Mars moved off, each with a link to the docs that say how to fix it; a reader that meets one
emits `field.retired` as a Warning, because the user loses what it set. Nothing else names an old
key. The one other place that names a key from outside is a translator from another format
(`claude/`, `harness_overrides`, `set_aside`).

## Design Constraints

- Mars overwrites or deletes only what it installed and recorded; anything else it leaves
  alone and reports, because people keep their own files beside installed ones.
- Packages come from other people's repositories: Mars reads only the sources and config it
  was given, and a package can't read outside its root or write outside the destination.
- Exported metadata carries no agent or skill bodies, and an imported MCP server gets no
  credential Claude would withhold.
- A lost restriction that changes what an agent can do is shown by default, and every other
  field a harness can't express shows under `--verbose`.
- Resolve first, then act: if resolution fails anywhere, nothing on disk changes.
- Config, lock and installed files are written atomically (tmp+rename).
- Windows is first-class: no POSIX-only assumptions.
- No VCS dependency: find the project by walking `Path::parent()`, not by `.git`.
- The builtin model aliases (`opus`, `sonnet`, `haiku`, `codex`, `gpt`, `gemini`) appear
  only when neither packages nor the project's config provide any.
- `.claude`, `.codex` and `.opencode` are first-class targets. `.cursor` is deprecated:
  it keeps working but isn't tested or extended, a sync warns while it is a target
  (`deprecated-cursor-target`), and it goes in the minor after 0.37.0. `.pi` is still being designed. `.agy` (Agy) is a plugin directory that `.agents/plugins.json` points at, not a copy of `.mars/`: its files are written by key (`src/target/AGENTS.md`).
- Comments only for what the code can't show and a wrong edit would break silently; no
  decision IDs.
- A state machine's code and its diagram change in the same commit. Every machine, with the
  patterns they share, is indexed at [.context/state-machines/index.md](.context/state-machines/index.md).

How the code keeps the first four today, what checks them and where it falls short:
[.context/invariants.md](.context/invariants.md).

## Commands

Run `scripts/setup-hooks.sh` once after cloning. `scripts/preflight.sh fast` checks
formatting; `scripts/preflight.sh full` adds clippy (`-D warnings`), the tests, the
`mars-schema` tests and a release build.

Inside a Meridian session, pass the checkout explicitly: `meridian -C
"$MERIDIAN_TASK_DIR" mars sync`. This repo has no work or KB context of its own: run
`meridian work` from the product the change serves (`~/gitrepos/meridian-cli` for Mars
and prompt infrastructure).

## Shared Checkout

Other agents and Jimmy work here at the same time. Leave changes you didn't make, even
ones that look wrong, and report them. The primary checkout stays on `main`: change
branches only in a worktree (`../mars-agents.worktrees/<slug>`, passed to spawns as
`--task-dir`). Each area's `AGENTS.md` holds the rules that guard it; read it before
changing that area.

## Merging and Releasing

A fix restores intended behavior; changing what users are meant to see is a feature.
Docs-only changes and tiny, obvious bug fixes go straight to `main`. Everything else goes through a PR, and
merging one into `main` takes Jimmy's approval in this conversation, per PR, unless your
global instructions grant your model more. A change that crosses a boundary others
depend on (CLI commands, flags or output, `mars.toml` or `mars.lock`, the package format,
what a sync writes into a target) needs his approval before it starts.

CI tags and publishes releases: never tag, edit version numbers, or push with
`--no-verify`. Before opening a PR or merging, read
[docs/dev/releasing.md](docs/dev/releasing.md).
