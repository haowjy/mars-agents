# mars-agents

Mars installs agent profiles and skills from git sources into each harness's own
directories (`.claude/`, `.codex/`, `.opencode/`, `.cursor/`, `.pi/`), through a canonical
store in `.mars/`.

## Who It's For

- **The person installing packages.** Their project mixes hand-written agents, skills,
  hooks and MCP servers with installed ones, in the same directories.
- **The package author.** Their profile should do what it says in every harness, or they
  need to know what didn't carry over.
- **Meridian and the harnesses,** which read `.mars/` and the target files. The package
  format and what a sync writes are contracts they build on.

## Invariants

These hold however the code changes, but they can change too. When evidence shows one
no longer serves the users, propose the change to Jimmy as its own step, never as a
side effect of getting a task to pass. How the code keeps each one, what checks it and
where it falls short: [.context/invariants.md](.context/invariants.md).

- **Your files stay yours.** Mars overwrites or deletes only what it installed and
  recorded; anything else it leaves alone and reports, because people keep their own
  files beside installed ones.
- **Only what it was given.** Mars reads only the sources and config it was given, and a
  package can't read outside its root or write outside the destination: packages come
  from other people's repositories.
- **Private content stays private.** Exported metadata carries no agent or skill bodies,
  and an imported MCP server gets no credential Claude would withhold.
- **Nothing lost silently.** A lost restriction that changes what an agent can do is
  shown by default, and every other field a harness can't express shows under
  `--verbose`: the author and the user must know what an agent can actually do.

## Errors and Warnings

A problem is built as a `Diagnostic` and nothing else; its level says what the user loses if
they ignore it, and the code that finds the problem states it.

- `Error`: Mars cannot use this input, and the command fails.
- `Warning`: the user loses something (a restriction not applied, a file not written, an item
  not installed).
- `Info`: nothing is lost, and it may be intended. Printed only when asked: `mars validate`,
  `mars check` and `--verbose` ask; a default `mars sync` and a launch bundle do not.

`Diagnostic::shown` and `output::print_diagnostics` are the whole policy: no reader prints, and
no reader decides whether its problem is shown. Reducing what users see means changing a site
from `Warning` to `Info`, or deleting it.

Mars has no code about a key it does not read: no list of removed keys, no per-key advice, no
deprecation note for a key that is gone. What happens to one is what its table does with any
key it has no use for (`crates/mars-schema/AGENTS.md`). The one place that names an old key is
a translator from another format, such as the pre-v1 upgrade in `upgrade.rs`.

## Design Constraints

- Resolve first, then act: if resolution fails anywhere, nothing on disk changes.
- Config, lock and installed files are written atomically (tmp+rename).
- Windows is first-class: no POSIX-only assumptions.
- No VCS dependency: find the project by walking `Path::parent()`, not by `.git`.
- The builtin model aliases (`opus`, `sonnet`, `haiku`, `codex`, `gpt`, `gemini`) appear
  only when neither packages nor the project's config provide any.
- `.claude`, `.codex` and `.opencode` are first-class targets. `.cursor` is unmaintained:
  it keeps working but isn't tested or extended. `.pi` is still being designed.
- Comments only for what the code can't show and a wrong edit would break silently; no
  decision IDs.

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
what a sync writes into a target, an invariant) needs his approval before it starts.

CI tags and publishes releases: never tag, edit version numbers, or push with
`--no-verify`. Before opening a PR or merging, read
[docs/dev/releasing.md](docs/dev/releasing.md).
