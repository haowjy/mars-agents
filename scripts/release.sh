#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'EOF'
Usage:
  scripts/release.sh patch [--push] [--remote origin]
  scripts/release.sh minor [--push] [--remote origin]
  scripts/release.sh major [--push] [--remote origin]
  scripts/release.sh X.Y.Z [--push] [--remote origin]

Behavior:
  - Runs pre-release checks (fmt, clippy, tests, release build)
  - Updates version in Cargo.toml, pyproject.toml, and Cargo.lock
  - Creates a release commit
  - Creates an annotated git tag named v<version>
  - Optionally pushes the branch and tag

Examples:
  scripts/release.sh patch
  scripts/release.sh 0.1.0 --push
EOF
}

die() {
  printf '%s\n' "$*" >&2
  exit 1
}

require_clean_tree() {
  if [[ -n "$(git -C "$ROOT_DIR" status --short)" ]]; then
    die "working tree is not clean; commit or stash changes first"
  fi
}

require_branch() {
  local branch
  branch="$(git -C "$ROOT_DIR" branch --show-current)"
  if [[ -z "$branch" ]]; then
    die "release helper must run from a branch, not detached HEAD"
  fi
  printf '%s\n' "$branch"
}

read_current_version() {
  local version
  version="$(grep '^version' "$ROOT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')"
  [[ -n "$version" ]] || die "could not read version from Cargo.toml"
  printf '%s\n' "$version"
}

validate_version() {
  local version="$1"
  [[ "$version" =~ ^[0-9]+(\.[0-9]+){2}$ ]] || \
    die "version must look like X.Y.Z; got: $version"
}

next_version() {
  local bump="$1"
  local current="$2"
  IFS='.' read -r major minor patch <<<"$current"

  case "$bump" in
    patch) patch=$((patch + 1)) ;;
    minor) minor=$((minor + 1)); patch=0 ;;
    major) major=$((major + 1)); minor=0; patch=0 ;;
    *) die "unknown bump kind: $bump" ;;
  esac

  printf '%s\n' "$major.$minor.$patch"
}

write_version() {
  local current="$1"
  local version="$2"
  sed -i "s/^version = \"${current}\"/version = \"${version}\"/" "$ROOT_DIR/Cargo.toml"
  sed -i "s/^version = \"${current}\"/version = \"${version}\"/" "$ROOT_DIR/pyproject.toml"
  (cd "$ROOT_DIR" && cargo check --quiet 2>/dev/null)  # update Cargo.lock
}

main() {
  [[ $# -ge 1 ]] || {
    usage
    exit 1
  }

  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
  esac

  local target="$1"
  shift

  local push_remote=""
  local remote="origin"

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --push)
        push_remote="1"
        shift
        ;;
      --remote)
        [[ $# -ge 2 ]] || die "--remote requires a value"
        remote="$2"
        shift 2
        ;;
      *)
        die "unknown argument: $1"
        ;;
    esac
  done

  require_clean_tree
  local branch
  branch="$(require_branch)"

  # Pre-release checks
  printf 'Running pre-release checks...\n'
  printf '  fmt... '
  (cd "$ROOT_DIR" && cargo fmt --check >/dev/null 2>&1) || die "cargo fmt --check failed — run 'cargo fmt'"
  printf 'pass\n'
  printf '  clippy... '
  (cd "$ROOT_DIR" && cargo clippy --all-targets -- -D warnings >/dev/null 2>&1) || die "clippy has warnings"
  printf 'pass\n'
  printf '  tests... '
  (cd "$ROOT_DIR" && cargo test >/dev/null 2>&1) || die "tests failed"
  printf 'pass\n'
  printf '  release build... '
  (cd "$ROOT_DIR" && cargo build --release >/dev/null 2>&1) || die "release build failed"
  printf 'pass\n'

  local current_version
  current_version="$(read_current_version)"

  # Version consistency check
  local pypi_version
  pypi_version="$(grep '^version' "$ROOT_DIR/pyproject.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')"
  [[ "$current_version" = "$pypi_version" ]] || \
    die "Cargo.toml ($current_version) != pyproject.toml ($pypi_version)"

  local next_version_value
  case "$target" in
    patch|minor|major)
      next_version_value="$(next_version "$target" "$current_version")"
      ;;
    *)
      next_version_value="$target"
      ;;
  esac

  validate_version "$next_version_value"
  [[ "$next_version_value" != "$current_version" ]] || \
    die "next version matches current version ($current_version)"

  local tag="v$next_version_value"
  if git -C "$ROOT_DIR" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    die "tag already exists: $tag"
  fi

  write_version "$current_version" "$next_version_value"

  git -C "$ROOT_DIR" add Cargo.toml pyproject.toml Cargo.lock
  git -C "$ROOT_DIR" commit -m "release: v$next_version_value"
  git -C "$ROOT_DIR" tag -a "$tag" -m "Release $next_version_value"

  printf 'Released %s on branch %s\n' "$next_version_value" "$branch"
  printf 'Created commit and tag %s\n' "$tag"

  if [[ -n "$push_remote" ]]; then
    git -C "$ROOT_DIR" push "$remote" "$branch"
    git -C "$ROOT_DIR" push "$remote" "$tag"
    printf 'Pushed branch %s and tag %s to %s\n' "$branch" "$tag" "$remote"
  else
    printf 'Nothing pushed. Run:\n'
    printf '  git push %s %s\n' "$remote" "$branch"
    printf '  git push %s %s\n' "$remote" "$tag"
  fi
}

main "$@"
