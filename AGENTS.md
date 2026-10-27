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

## Critical Invariants

- **Mars never deletes files it didn't create.** Per-target lock ownership gates all removals.
- **Atomic writes** — tmp+rename for config, lock, and installed files.
- **Resolve first, then act** — zero mutations if any error during resolution.
- **Builtin model aliases are fallback-only** — `opus`, `sonnet`, `haiku`,
  `codex`, `gpt`, and `gemini` appear only when packages and consumer config
  both provide no aliases.
- **Windows is first-class** — no POSIX-only assumptions.
- **No VCS dependency** — walk to filesystem root via `Path::parent()`, not `.git`.

## Architecture

See `src/AGENTS.md` for the dependency graph and module index. Each `src/*/AGENTS.md`
documents its own mental model, key rules, and anti-patterns.

## Dev Workflow

```bash
cargo build
cargo test
cargo clippy
```

Integration tests under `tests/`.

## Git Hooks

Run `scripts/setup-hooks.sh` (or `scripts/setup-hooks.ps1` on Windows) once after cloning.

**NEVER use `--no-verify` on git push unless explicitly instructed by the user.**

**NEVER manually create or push git tags matching `v*`.** CI creates release tags.

## Pull Requests

Read `.github/PULL_REQUEST_TEMPLATE.md` and fill every section. **Always set a `release:*` label before merging** — no label = no release.

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

**A breaking change split across several PRs ships as one release.** Every PR merged to
`main` with `release:minor`/`release:major` cuts its own release, so when one piece of work
breaks behaviour in several PRs, plan this when you split it:
1. Create an integration branch from `main` (e.g. `release/<topic>`) and open its PR to
   `main` early, as a draft. It carries the release label, the combined changelog and
   upgrade notes, and the whole-change review.
2. Point each part's PR at the integration branch. Merging there releases nothing, so those
   PRs need no label. Copy their changelog sections into the integration PR's `## Changelog`:
   only commits on `main`'s first-parent history reach the renderer, so a part PR merged into
   the integration branch never renders on its own.
3. When the last part lands and the full gate passes, merge the integration PR once.

Work that must release on its own (e.g. a Mars release a Meridian change waits on) gets its
own release; decide that when planning, not at merge time.

## Releasing

CI-owned. Never manually `git tag` or edit version numbers. PR with `release:*` label lands on `main` → CI bumps version, renders changelog, commits, tags, and publishes to PyPI/npm/crates.io.

**Direct push ≠ release.** A push to `main` triggers a release only when the pushed commit is the merge of a PR labeled `release:*`. `release-on-main.yml` looks up the trigger commit's PR (`/commits/{sha}/pulls`); finding none, it sets `should_release=false` and runs CI only — no bump, no tag. Docs and other no-ceremony work can go straight to `main`.

**`CHANGELOG.md` `[Unreleased]` is only an empty anchor.** Never write entries there: the release walks the merged PRs' `## Changelog` sections since the last `release: v` commit and renders them under the new version heading. A `release:skip` merge, a failed or cancelled run, and two merges in one push keep their notes — the next release, automatic or manual, collects them.

**Note:** `mars version` is for prompt packages only. For mars-agents itself, use the CI release flow. See `.github/workflows/README.md` for the step order, the manual backfill path, and the remedy when a merged PR body fails the notes gate.
