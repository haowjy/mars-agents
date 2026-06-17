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

## Target Support Status

- `.claude`, `.codex`, `.opencode`, `.cursor` are first-class targets.
- `.pi` is under active design/development.

## Critical Invariants

- **Mars never deletes files it didn't create.** Per-target lock ownership gates all removals.
- **Atomic writes** — tmp+rename for config, lock, and installed files.
- **Resolve first, then act** — zero mutations if any error during resolution.
- **No builtin model aliases in the binary** — all come from packages or consumer config.
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

| Label | Effect |
|---|---|
| `release:patch` / `release:stable` | Stable patch bump |
| `release:minor` | Minor bump |
| `release:major` | Major bump |
| `release:rc` | Prerelease |
| `release:skip` | No release |

## Releasing

CI-owned. Never manually `git tag` or edit version numbers. PR with `release:*` label lands on `main` → CI bumps version, promotes changelog, commits, tags, and publishes to PyPI/npm/crates.io.

**Direct push ≠ release.** A push to `main` triggers a release only when the pushed commit is the merge of a PR labeled `release:*`. `release-on-main.yml` looks up the trigger commit's PR (`/commits/{sha}/pulls`); finding none, it sets `should_release=false` and runs CI only — no bump, no tag. Docs and other no-ceremony work can go straight to `main`.

**Update CHANGELOG.md `[Unreleased]` as you work** — CI promotes it during release.

**Note:** `mars version` is for prompt packages only. For mars-agents itself, use the CI release flow.
