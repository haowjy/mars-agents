## Changelog

<!-- CI validates this section; the release job renders it into CHANGELOG.md
     under the new version heading and uses it as the GitHub Release body. Do
     not edit CHANGELOG.md in this PR. Use `### Upgrade notes`, `### Breaking`,
     `### Added`, `### Changed`, `### Deprecated`, `### Removed`, `### Fixed`,
     or `### Security` blocks with `- ` bullets (continuation lines are fine),
     or replace the whole section with one line: `None. <reason nothing a user
     or developer notices changed>`. Write one line per upgrade note, ending in
     an absolute link to the guide section: `- ... See [what changed](https://github.com/haowjy/mars-agents/blob/main/docs/...).`
     The guide carries the detail; the changelog entry links to it. Absolute
     links resolve in both the PR body and the GitHub Release body. -->

### Fixed
- <what a user now sees that was broken before>

## Why

<!-- Problem or motivation. What breakage, gap, or need makes this PR worth merging? -->

## Goal

<!-- Intended outcome. What should be true after this merges? -->

## Summary

<!-- What changed? Paste the agent-generated summary, then adjust for clarity. -->

## Resulting Behavior

<!-- User-facing end state, in one or two sentences. What can someone do after this merges that they could not do before? -->

## End-to-End Walkthrough

<!-- Take a reviewer through the new or changed behavior from the user's side,
     when there is something to walk through. A UI change gets screenshots or a
     GIF with numbered steps and the before/after state. A CLI, config, library
     or docs change gets a numbered text walkthrough: copy-pasteable commands
     and their real output, or before/after files. Where behavior changed,
     show the before state on main and the after state on this branch. If
     nothing is runnable or visible, write "Not applicable" and say what
     check replaces it. -->

## Changes

<!-- Notable implementation details, behavior changes, risks, and follow-ups. -->

## Work Item

<!-- Meridian work item slug, for example: harness-aware-agent-inventory -->

## Verification

<!-- What was run or checked? Include tests, smoke checks, type/lint, or why verification is not applicable. -->

## Knowledge Updates

<!-- Were .context/, KB, docs, or other durable knowledge artifacts updated?
     If not, note why (for example: no new behavior, docs not applicable). -->

## Spawn Trace

<!-- Direct/top-level Meridian spawn IDs only: role and short purpose, for example:
     - p123 coder — implemented catalog filtering
     - p124 qa-lead — reviewed/updated tests
     - p125 kb-lead — updated durable knowledge
-->

## Release Label Guide

Set one `release:*` label on this PR:

- `release:patch` / `release:stable` — next stable **patch** release after merge
- `release:minor` — next stable **minor** release after merge
- `release:major` — next stable **major** release after merge
- `release:rc` — next **prerelease (RC)** after merge
- `release:skip` — no release for this merge

No `release:*` label defaults to a prerelease (RC). Unknown `release:*` labels also default to RC.

## Post-Merge Automation

After merge to `main`, CI (`.github/workflows/release-on-main.yml`) will:

1. Read the PR release label
2. Skip only when `release:skip` is present (no label defaults to RC)
3. Compute the next stable or RC version from existing `v*` tags
4. Collect the `## Changelog` sections of the merged PRs since the last release and render
   them into `CHANGELOG.md` under the new version heading
5. Update Cargo/PyPI/npm package versions
6. Commit `release: vX.Y.Z` (or `release: vX.Y.Z-rc.N`), create/push the matching tag
7. Run `.github/workflows/release.yml` from the tag, which publishes that same section as the
   GitHub Release body

## Cleanup

After merge, clean merged worktrees with:

```bash
scripts/prune-worktrees.sh
```
