#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/manually-release.sh [patch|minor|major|X.Y.Z] [--push]
                              [--no-collect] [--notes-file PATH]
  scripts/manually-release.sh tag-current [--push]

Manual release helper for emergency/backfill releases.

Creates a real release commit before tagging. The normal path remains the
label-gated GitHub workflow.

Changelog notes come from the merged PR bodies: the script walks the first-parent
commits since the last "release: v" commit and renders each merged PR's
`## Changelog` section into CHANGELOG.md under the new version heading, exactly
as the release workflow does. That needs an authenticated `gh`.

  --notes-file PATH  additional notes rendered after the collected ones
                     (same shape as a PR body); '-' reads stdin
  --no-collect       skip the walk and render --notes-file alone (offline, or a
                     direct-push-only range); the file must then carry every
                     merged PR's notes since the last release
USAGE
}

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

validate_version() {
  [[ "$1" =~ ^[0-9]+(\.[0-9]+){2}$ ]]
}

read_cargo_version() {
  grep '^version' "$ROOT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/'
}

read_pypi_version() {
  grep '^version' "$ROOT_DIR/pyproject.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/'
}

next_version() {
  local bump="$1"
  local current="$2"
  local major minor patch
  IFS='.' read -r major minor patch <<<"$current"

  case "$bump" in
    patch) patch=$((patch + 1)) ;;
    minor) minor=$((minor + 1)); patch=0 ;;
    major) major=$((major + 1)); minor=0; patch=0 ;;
    *) die "unknown bump kind: $bump" ;;
  esac

  printf '%s.%s.%s\n' "$major" "$minor" "$patch"
}

require_clean_tree() {
  [[ -z "$(git -C "$ROOT_DIR" status --short)" ]] || die "working tree is not clean"
}

render_notes() {
  local version="$1"
  local date="$2"
  shift 2
  python3 "$ROOT_DIR/scripts/changelog-notes.py" render \
    --version "$version" \
    --date "$date" \
    --changelog "$ROOT_DIR/CHANGELOG.md" \
    "$@"
}

write_versions() {
  local version="$1"

  sed -i -E "0,/^version = \".*\"/s//version = \"${version}\"/" "$ROOT_DIR/Cargo.toml"
  sed -i -E "0,/^version = \".*\"/s//version = \"${version}\"/" "$ROOT_DIR/crates/mars-schema/Cargo.toml"
  sed -i -E "0,/^version = \".*\"/s//version = \"${version}\"/" "$ROOT_DIR/crates/mars-schema-wasm/Cargo.toml"
  sed -i -E "s/(mars-schema = \{ version = )\"[^\"]+\"/\1\"${version}\"/" "$ROOT_DIR/Cargo.toml" "$ROOT_DIR/crates/mars-schema-wasm/Cargo.toml"
  sed -i -E "0,/^version = \".*\"/s//version = \"${version}\"/" "$ROOT_DIR/pyproject.toml"

  local pkg
  for pkg in "$ROOT_DIR"/npm/@meridian-flow/*/package.json; do
    [[ -f "$pkg" ]] || continue
    node -e "
      const fs = require('fs');
      const path = '$pkg';
      const pkg = JSON.parse(fs.readFileSync(path, 'utf8'));
      pkg.version = '$version';
      for (const dep of Object.keys(pkg.optionalDependencies || {})) {
        pkg.optionalDependencies[dep] = '$version';
      }
      fs.writeFileSync(path, JSON.stringify(pkg, null, 2) + '\n');
      // A package's lockfile records its own version twice.
      const lockPath = '${pkg%package.json}package-lock.json';
      if (fs.existsSync(lockPath)) {
        const lock = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
        lock.version = '$version';
        lock.packages[''].version = '$version';
        fs.writeFileSync(lockPath, JSON.stringify(lock, null, 2) + '\n');
      }
    "
  done

  (cd "$ROOT_DIR" && cargo check --quiet)
}

validate_versions_match() {
  local cargo_version pypi_version
  cargo_version="$(read_cargo_version)"
  pypi_version="$(read_pypi_version)"
  [[ "$cargo_version" == "$pypi_version" ]] || die "Cargo.toml version $cargo_version != pyproject.toml version $pypi_version"
}

validate_current_release() {
  validate_versions_match
  local version tag subject
  version="$(read_cargo_version)"
  validate_version "$version" || die "invalid Cargo.toml version: $version"
  tag="v$version"
  subject="$(git -C "$ROOT_DIR" log -1 --format=%s)"

  [[ "$subject" == "release: v$version" ]] || die "HEAD subject must be 'release: v$version', got '$subject'"
  grep -q "^## \\[$version\\] -" "$ROOT_DIR/CHANGELOG.md" || die "CHANGELOG.md missing [$version] release section"
  printf '%s\n' "$tag"
}

create_tag() {
  local tag="$1"
  local head_sha existing_sha
  head_sha="$(git -C "$ROOT_DIR" rev-parse HEAD)"

  if git -C "$ROOT_DIR" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    existing_sha="$(git -C "$ROOT_DIR" rev-list -n 1 "$tag")"
    [[ "$existing_sha" == "$head_sha" ]] || die "$tag already points at $existing_sha, expected $head_sha"
    printf 'Tag %s already exists at HEAD.\n' "$tag"
  else
    git -C "$ROOT_DIR" tag -a "$tag" -m "Release ${tag#v}"
    printf 'Created tag %s.\n' "$tag"
  fi
}

push_release() {
  local tag="$1"
  git -C "$ROOT_DIR" push origin HEAD:main
  git -C "$ROOT_DIR" push --no-verify origin "$tag"
}

main() {
  [[ $# -ge 1 ]] || { usage >&2; exit 1; }

  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    -*)
      die "the release command must come first: scripts/manually-release.sh patch --notes-file PATH"
      ;;
  esac

  local command="$1"
  shift
  local push=0
  local collect=1
  local notes_file=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --push) push=1; shift ;;
      --no-collect) collect=0; shift ;;
      --notes-file)
        [[ $# -ge 2 ]] || die "--notes-file requires a path ('-' reads stdin)"
        notes_file="$2"
        shift 2
        ;;
      -h|--help) usage; exit 0 ;;
      *) die "unknown argument: $1" ;;
    esac
  done

  cd "$ROOT_DIR"

  if [[ "$command" == "tag-current" ]]; then
    [[ -z "$notes_file" ]] || die "tag-current does not take --notes-file"
    [[ "$collect" -eq 1 ]] || die "tag-current does not take --no-collect"
    require_clean_tree
    tag="$(validate_current_release)"
    create_tag "$tag"
    [[ "$push" -eq 0 ]] || push_release "$tag"
    exit 0
  fi

  work_dir="$(mktemp -d)"
  trap 'rm -rf "$work_dir"' EXIT

  # Resolve --notes-file before changing directory, so a relative path means
  # the caller's working directory, not the repository root.
  if [[ -n "$notes_file" ]]; then
    if [[ "$notes_file" == "-" ]]; then
      cat >"$work_dir/notes.md"
      notes_file="$work_dir/notes.md"
    else
      [[ -f "$notes_file" ]] || die "notes file not found: $notes_file"
      notes_file="$(cd "$(dirname "$notes_file")" && pwd)/$(basename "$notes_file")"
    fi
  fi

  require_clean_tree
  validate_versions_match

  local current version today tag
  current="$(read_cargo_version)"
  case "$command" in
    patch|minor|major) version="$(next_version "$command" "$current")" ;;
    *) validate_version "$command" || die "unknown release target: $command"; version="$command" ;;
  esac

  [[ "$version" != "$current" ]] || die "target version matches current version: $version"
  tag="v$version"
  if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    die "tag already exists: $tag"
  fi

  local notes_args=()
  if [[ "$collect" -eq 1 ]]; then
    if ! scripts/collect-release-notes.sh "$work_dir/prs.json"; then
      die "collecting merged PR notes failed (see the error above); pass --no-collect to release from --notes-file alone"
    fi
    notes_args+=(--notes-json "$work_dir/prs.json")
  fi
  if [[ -n "$notes_file" ]]; then
    notes_args+=(--notes-file "$notes_file")
  fi
  [[ ${#notes_args[@]} -gt 0 ]] || die "no notes: pass --notes-file PATH, or drop --no-collect so merged PRs are collected"

  today="$(date -u +%Y-%m-%d)"
  render_notes "$version" "$today" "${notes_args[@]}" --dry-run >/dev/null
  write_versions "$version"
  scripts/preflight.sh full
  render_notes "$version" "$today" "${notes_args[@]}"

  git add Cargo.toml crates/mars-schema/Cargo.toml crates/mars-schema-wasm/Cargo.toml pyproject.toml Cargo.lock CHANGELOG.md npm/@meridian-flow/*/package.json npm/@meridian-flow/*/package-lock.json
  git commit -m "release: v$version"
  create_tag "$tag"
  [[ "$push" -eq 0 ]] || push_release "$tag"
}

main "$@"
