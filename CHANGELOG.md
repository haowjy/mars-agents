# Changelog

Caveman style. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

## [0.1.3] - 2026-04-16

### Added
- `mars adopt` moves unmanaged target items into `.mars-src/`, then syncs.
- `.mars-src` is now project-local source for agents and skills.
- Non-package repos can mirror local items across `.agents`, `.claude`, and other targets.
- Smoke coverage and docs for adopt/local source flow.

### Changed
- Sync now reads `.mars-src` local items even without `[package]`.
- Legacy repo-root `agents/` and `skills/` stay supported only for package repos.
- `.mars-src` wins if both local roots define the same item.

### Fixed
- `mars list` now shows adopted/local `.mars-src` items after sync.

## [0.1.2] - 2026-04-16

### Added
- Subpath support. One repo can hold many packages.
- Parser understands more source forms: GitHub, GitLab, generic git, local path.
- Smoke testing guide added.
- Repo now uses `meridian-dev-workflow` through Mars.

### Changed
- Fallback discovery now does explicit paths first, then nearest non-empty layer.
- Same-layer fallback picks first deterministic match.
- `mars add` supports `--subpath`.
- Docs now explain subpath and supported source forms.

### Fixed
- `meridian-dev-workflow` install no longer breaks on mirrored `caveman` layout.
- GitLab-like URLs keep explicit ports.
- Parser clippy failure fixed for release checks.
