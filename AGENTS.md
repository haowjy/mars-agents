# Development Guide: mars-agents

Mars is an agent package manager. It installs agent profiles and skills from git/local sources into a `.mars/` canonical store, then copies managed content into target directories (`.agents/`, `.claude/`, `.cursor/`, etc.).

## Meridian work context

This repo has **no** `[context.work]` / `[context.kb]` in `meridian.toml`. Do not run
`meridian work` here for product feature work.

| PR touches | Run `meridian work` from | Work + KB |
|---|---|---|
| Meridian CLI, mars sync, shared prompts infra | `~/gitrepos/meridian-cli` | haowjy-meridian-cli-docs work + meridian-cli-kb |
| Voluma product / voluma-bio packages | `~/gitrepos/voluma` | voluma-bio-docs work + kb |

Use the active work item on **that** product before writing design notes or handoffs.

Inside Meridian-managed sessions, `MERIDIAN_TASK_DIR` points at the checkout where
source work happens, while `MERIDIAN_PROJECT_DIR` remains anchored to the parent
control repo. Pass the root explicitly when invoking Meridian from this repo:

```bash
meridian -C "$MERIDIAN_TASK_DIR" mars sync
```

## Branch Discipline

**The primary checkout (`~/gitrepos/mars-agents`) stays on `main`.** Never
`checkout`/`switch` it to a feature branch — it is a shared workspace and
sessions launch with cwd pointed here. Enforced by the `meridian.stayOnMain`
git guard (one-shot override: `GIT_ALLOW_LEAVE_MAIN=1`, for emergencies only).

All branch work happens in worktrees:

```bash
git worktree add ../mars-agents.worktrees/<slug> -b <branch> main
```

Pass the worktree as `--task-dir` when spawning agents. After the PR merges:
remove the worktree, delete the branch (local + origin) — merged-branch
residue in the primary checkout is how drift starts.

## Target Support Status

- `.claude`, `.codex`, `.opencode` are first-class targets.
- `.cursor` is unmaintained: existing support stays, but it isn't tested or extended.
- `.pi` is under active design/development.

## Invariants

These hold however the work is done. Change one only as its own visible step, never to get a task passing. Everything else in this file is a current choice, open to rewriting when evidence shows something better.

| Invariant | Check |
|---|---|
| User content is never overwritten or deleted without authorization. Anything Mars owns is authorized, so source wins on a file Mars manages. | `tests/self_conflicts.rs`, `tests/config_entries/ownership.rs` |
| Mars never acts beyond what was authorized: it reads only the sources and config it was given, and a package can't reach outside its own root or write outside the destination | Partial: archive and bootstrap escape tests |
| Private content never reaches the wrong recipient: exported metadata carries no agent or skill bodies, and imported servers get no credentials Claude would withhold | Partial: `tests/validate_and_export.rs` |
| A lost restriction that changes what an agent can do is shown by default. Every other lost field is available under `--verbose`. Nothing is dropped without a record. | None yet |

A restriction is lost when a harness can't express part of a profile. It shows in default `sync`, `check` and `build plugin` output, not at launch.

## Contracts

- **Atomic writes:** tmp+rename for config, lock, and installed files.
- **Resolve first, then act:** zero mutations if any error during resolution.
- **Builtin model aliases are fallback-only:** `opus`, `sonnet`, `haiku`, `codex`, `gpt`, and `gemini` appear only when packages and consumer config both provide no aliases.
- **Windows is first-class:** no POSIX-only assumptions.
- **No VCS dependency:** walk to filesystem root via `Path::parent()`, not `.git`.

## Comments

Few. Write one only for what the code can't show and a wrong edit would break silently, at the line it guards. No decision ids.

## Git Hooks

Run `scripts/setup-hooks.sh` (or `scripts/setup-hooks.ps1` on Windows) once after cloning.

**NEVER use `--no-verify` on git push unless explicitly instructed by the user.**

**NEVER manually create or push git tags matching `v*`.** CI creates release tags.

## Pull Requests

Read `.github/PULL_REQUEST_TEMPLATE.md` and fill every section. **Always set a `release:*` label before merging** — no label defaults to a prerelease (RC).

**Write the changelog entry in the PR body's `## Changelog` section, never in
`CHANGELOG.md`.** The release job renders that section into `CHANGELOG.md` under the new
version heading and publishes it as the GitHub Release body; one writer is what keeps two
branches from conflicting on the file. CI validates the section when the body or branch
changes, and `pr-body.yml` refuses a PR that adds content under `## [Unreleased]` or a
release heading to `CHANGELOG.md`. Check it locally with
`python3 scripts/changelog-notes.py validate --notes-file pr-body.md`.

| Label | Effect |
|---|---|
| `release:patch` / `release:stable` | Stable patch bump |
| `release:minor` | Minor bump |
| `release:major` | Major bump |
| `release:rc` | Prerelease |
| `release:skip` | No release |

**Minor and major releases collect on `release/v<X>.<Y>.0`.** Every PR merged to `main`
with `release:minor`/`release:major` cuts its own release, so a minor or major change goes
to the open release branch (create it from `main` if none is open), and only that branch's
PR to `main` carries the label. Patch-level changes merge to `main` on their own. Follow
meridian-cli's [release-branch procedure](https://github.com/haowjy/meridian-cli/blob/main/docs/releasing.md#minor-and-major-releases-collect-on-a-release-branch)
for the steps: part PRs, the combined changelog, and who merges what.

## Releasing

CI-owned. Never manually `git tag` or edit version numbers. PR with `release:*` label lands on `main` → CI bumps version, renders changelog, commits, tags, and publishes to PyPI/npm/crates.io.

**Straight to `main` without approval:** docs-only changes and tiny, obvious bug fixes. Every
other merge to `main` needs Jimmy's approval in the current conversation, unless the global grant
covers your model.

A fix restores intended behavior; changing what users are meant to see is a feature. A big
refactor, reorganization or architecture change needs Jimmy's approval first, quoted in
its issue or PR; another agent's report of that approval doesn't count. Before a merge, CI
is green, the regressions the change could cause are exercised on the real system (`/testing`), and the PR body is reread
against the final diff, with evidence from the commit being merged. A big fix or refactor
also needs an independent review. A merge can release, so take the label with care:
`release:patch` publishes to users, and a change with no user effect takes `release:skip`.

Big means impact, not size: a change is big when it crosses a boundary others depend on.
That is the CLI's commands, flags or output, `mars.toml` or `mars.lock`, the package format, what a sync writes into a target, or an invariant above. A change that looks narrow but alters behavior others rely on is big
too; when unsure, call it big.

**Direct push ≠ release.** A push to `main` triggers a release only when the pushed commit is the merge of a PR labeled `release:*`. `release-on-main.yml` looks up the trigger commit's PR (`/commits/{sha}/pulls`); finding none, it sets `should_release=false` and runs CI only — no bump, no tag. Docs and other no-ceremony work can go straight to `main`.

`CHANGELOG.md` `[Unreleased]` is only an empty anchor. The release collects PR
notes since the last `release: v` commit, including skipped or failed releases.

**Note:** `mars version` is for prompt packages only. For mars-agents itself, use the CI release flow. See `.github/workflows/README.md` for the step order, the manual backfill path, and the remedy when a merged PR body fails the notes gate.
