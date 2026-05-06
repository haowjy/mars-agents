#!/usr/bin/env bash

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/release.sh prepare [patch|minor|major|X.Y.Z] [--push] [--remote origin]
  scripts/release.sh resume [--push] [--remote origin]
  scripts/release.sh status
  scripts/release.sh abort

  # Shorthand (routes through prepare):
  scripts/release.sh patch [--push] [--remote origin]
  scripts/release.sh 1.2.3 [--push] [--remote origin]

Behavior:
  prepare:
    - Requires a clean working tree and branch checkout
    - Refuses when another mars release is active
    - Runs full pre-release checks exactly once
    - Updates version in Cargo.toml, pyproject.toml, and npm packages
    - Creates a release commit and records release state under <GIT_DIR>/mars-release/
    - With --push, pushes the branch only; tag creation moves to resume
  resume:
    - Validates persisted release state without rerunning full preflight
    - Creates an annotated git tag named v<version> at the release commit
    - Writes tag authorization state under <GIT_DIR>/mars-release/
    - With --push, pushes the tag and clears release state on success
  status:
    - Shows the active release state, or reports no release in progress
  abort:
    - Clears release state without reverting commits or deleting tags

Examples:
  scripts/release.sh prepare patch --push
  scripts/release.sh resume --push
  scripts/release.sh status
  scripts/release.sh abort
USAGE
}

die() {
  printf '%s\n' "$*" >&2
  exit 1
}

warn() {
  printf 'WARNING: %s\n' "$*" >&2
}

FAILURES=()

check() {
  local name="$1"
  local cmd="$2"
  printf '%s... ' "$name"
  if eval "$cmd" >/dev/null 2>&1; then
    printf 'pass\n'
    return 0
  else
    printf 'FAIL\n'
    FAILURES+=("check failed: $name")
    return 1
  fi
}

require_clean_tree() {
  if [[ -n "$(git -C "$ROOT_DIR" status --short)" ]]; then
    FAILURES+=("working tree is not clean; commit or stash changes first")
    return 1
  fi
  return 0
}

require_branch() {
  local branch
  branch="$(git -C "$ROOT_DIR" branch --show-current)"
  if [[ -z "$branch" ]]; then
    FAILURES+=("release script must run from a branch, not detached HEAD")
    return 1
  fi
  printf '%s\n' "$branch"
  return 0
}

read_cargo_version() {
  grep '^version' "$ROOT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/'
}

read_pypi_version() {
  grep '^version' "$ROOT_DIR/pyproject.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/'
}

read_npm_versions() {
  local pkg_dir="$ROOT_DIR/npm/@meridian-flow"
  local versions=()
  
  for pkg in "$pkg_dir"/mars-agents*/package.json; do
    if [[ -f "$pkg" ]]; then
      local v
      v="$(node -e "console.log(require('$pkg').version)")"
      versions+=("$v")
    fi
  done
  
  printf '%s\n' "${versions[@]}"
}

validate_version() {
  local version="$1"
  [[ "$version" =~ ^[0-9]+(\.[0-9]+){2}$ ]]
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
  local version="$1"
  
  sed -i "s/^version = \".*\"/version = \"$version\"/" "$ROOT_DIR/Cargo.toml"
  sed -i "s/^version = \".*\"/version = \"$version\"/" "$ROOT_DIR/pyproject.toml"
  
  local pkg_dir="$ROOT_DIR/npm/@meridian-flow"
  for pkg in "$pkg_dir"/mars-agents*/package.json; do
    if [[ -f "$pkg" ]]; then
      node -e "
        const pkg = require('$pkg');
        pkg.version = '$version';
        if (pkg.optionalDependencies) {
          for (const dep of Object.keys(pkg.optionalDependencies)) {
            pkg.optionalDependencies[dep] = '$version';
          }
        }
        require('fs').writeFileSync('$pkg', JSON.stringify(pkg, null, 2) + '\n');
      "
    fi
  done
  
  (cd "$ROOT_DIR" && cargo check --quiet 2>/dev/null)
}

release_dir() {
  local git_dir
  git_dir="$(git -C "$ROOT_DIR" rev-parse --absolute-git-dir)" || die "failed to resolve git dir"
  printf '%s\n' "$git_dir/mars-release"
}

active_json() {
  printf '%s\n' "$(release_dir)/active.json"
}

auth_json() {
  printf '%s\n' "$(release_dir)/auth.json"
}

json_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

json_field() {
  local file="$1"
  local field="$2"
  grep -m 1 "^[[:space:]]*\"$field\"[[:space:]]*:" "$file" \
    | sed 's/^[[:space:]]*"[^"]*"[[:space:]]*:[[:space:]]*"\(.*\)"[[:space:]]*,\{0,1\}[[:space:]]*$/\1/'
}

write_active_json() {
  local file="$1"
  local version="$2"
  local tag="$3"
  local branch="$4"
  local release_commit="$5"
  local prepared_at="$6"
  local remote="$7"

  mkdir -p "$(dirname "$file")" || die "failed to create release state directory"
  printf '{\n  "version": "%s",\n  "tag": "%s",\n  "branch": "%s",\n  "release_commit": "%s",\n  "prepared_at": "%s",\n  "remote": "%s"\n}\n' \
    "$(json_escape "$version")" \
    "$(json_escape "$tag")" \
    "$(json_escape "$branch")" \
    "$(json_escape "$release_commit")" \
    "$(json_escape "$prepared_at")" \
    "$(json_escape "$remote")" > "$file" || die "failed to write active release state"
}

write_auth_json() {
  local file="$1"
  local tag="$2"
  local target_sha="$3"
  local remote="$4"

  mkdir -p "$(dirname "$file")" || die "failed to create release state directory"
  printf '{\n  "tag": "%s",\n  "target_sha": "%s",\n  "remote": "%s",\n  "action": "create"\n}\n' \
    "$(json_escape "$tag")" \
    "$(json_escape "$target_sha")" \
    "$(json_escape "$remote")" > "$file" || die "failed to write tag auth state"
}

parse_release_options() {
  push_remote=""
  remote="origin"

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
}

print_failures_and_exit() {
  printf '\n=== PRE-RELEASE CHECKS FAILED ===\n\n'
  for f in "${FAILURES[@]}"; do
    printf '  - %s\n' "$f"
  done
  printf '\nFix the issues above and try again.\n'
  exit 1
}

cmd_prepare() {
  [[ $# -ge 1 ]] || die "prepare requires a target version or bump kind"

  local target="$1"
  shift
  local push_remote remote
  parse_release_options "$@"

  printf 'Running pre-release checks...\n\n'

  FAILURES=()
  local branch=""
  branch="$(require_branch)"
  require_clean_tree

  local active_file
  active_file="$(active_json)"
  if [[ -f "$active_file" ]]; then
    FAILURES+=("release already active: $active_file")
  fi

  if [[ ${#FAILURES[@]} -gt 0 ]]; then
    print_failures_and_exit
  fi

  if ! "$ROOT_DIR/scripts/preflight.sh" full; then
    FAILURES+=("check failed: preflight full")
  fi

  local cargo_version
  cargo_version="$(read_cargo_version)"
  local pypi_version
  pypi_version="$(read_pypi_version)"
  
  check "version: cargo == pypi" "[[ '$cargo_version' = '$pypi_version' ]]"

  if [[ ${#FAILURES[@]} -gt 0 ]]; then
    print_failures_and_exit
  fi

  printf '\nAll pre-release checks passed.\n\n'

  local next_version_value
  case "$target" in
    patch|minor|major)
      next_version_value="$(next_version "$target" "$cargo_version")"
      ;;
    *)
      next_version_value="$target"
      ;;
  esac

  validate_version "$next_version_value" || die "invalid version: $next_version_value"
  [[ "$next_version_value" != "$cargo_version" ]] || die "next version matches current: $cargo_version"

  local tag="v$next_version_value"
  if git -C "$ROOT_DIR" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    die "tag already exists: $tag"
  fi

  printf 'Bumping version: %s -> %s\n' "$cargo_version" "$next_version_value"
  write_version "$next_version_value" || die "failed to write version $next_version_value"

  local version_files=("Cargo.toml" "pyproject.toml" "Cargo.lock")
  local pkg
  for pkg in "$ROOT_DIR/npm/@meridian-flow"/mars-agents*/package.json; do
    if [[ -f "$pkg" ]]; then
      version_files+=("${pkg#$ROOT_DIR/}")
    fi
  done
  
  git -C "$ROOT_DIR" add "${version_files[@]}" || die "failed to stage version files"
  git -C "$ROOT_DIR" commit -m "release: v$next_version_value" || die "failed to create release commit"

  local release_commit
  release_commit="$(git -C "$ROOT_DIR" rev-parse HEAD)" || die "failed to read release commit"
  local prepared_at
  prepared_at="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
  write_active_json "$active_file" "$next_version_value" "$tag" "$branch" "$release_commit" "$prepared_at" "$remote"

  printf '\nPrepared release %s on branch %s\n' "$next_version_value" "$branch"
  printf 'Created release commit %s\n' "$release_commit"
  printf 'Wrote release state: %s\n' "$active_file"

  if [[ -n "$push_remote" ]]; then
    git -C "$ROOT_DIR" push "$remote" "$branch" || die "failed to push branch $branch to $remote"
    printf 'Pushed branch %s to %s. Tag not pushed.\n' "$branch" "$remote"
  fi

  printf '\nNext step: when CI is green, run:\n'
  printf '  scripts/release.sh resume --push --remote %s\n' "$remote"
}

cmd_resume() {
  local push_remote remote
  parse_release_options "$@"

  local active_file auth_file
  active_file="$(active_json)"
  auth_file="$(auth_json)"
  [[ -f "$active_file" ]] || die "no active release: $active_file"

  local version tag branch release_commit prepared_remote
  version="$(json_field "$active_file" version)"
  tag="$(json_field "$active_file" tag)"
  branch="$(json_field "$active_file" branch)"
  release_commit="$(json_field "$active_file" release_commit)"
  prepared_remote="$(json_field "$active_file" remote)"

  [[ -n "$version" ]] || die "active release missing version"
  [[ -n "$tag" ]] || die "active release missing tag"
  [[ -n "$release_commit" ]] || die "active release missing release_commit"

  if [[ "$remote" = "origin" && -n "$prepared_remote" ]]; then
    remote="$prepared_remote"
  fi

  local cargo_version pypi_version
  cargo_version="$(read_cargo_version)"
  pypi_version="$(read_pypi_version)"
  [[ "$cargo_version" = "$version" ]] || die "Cargo.toml version $cargo_version does not match prepared version $version"
  [[ "$pypi_version" = "$version" ]] || die "pyproject.toml version $pypi_version does not match prepared version $version"

  git -C "$ROOT_DIR" merge-base --is-ancestor "$release_commit" HEAD \
    || die "current branch does not contain release commit $release_commit"

  local existing_tag_sha=""
  if git -C "$ROOT_DIR" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    existing_tag_sha="$(git -C "$ROOT_DIR" rev-list -n 1 "$tag")" || die "failed to inspect existing tag $tag"
    [[ "$existing_tag_sha" = "$release_commit" ]] \
      || die "tag $tag already exists at $existing_tag_sha, expected $release_commit"
    printf 'Tag %s already exists at expected commit; skipping tag creation.\n' "$tag"
  else
    git -C "$ROOT_DIR" tag -a "$tag" "$release_commit" -m "Release $version" \
      || die "failed to create tag $tag"
    printf 'Created tag %s at %s.\n' "$tag" "$release_commit"
  fi

  write_auth_json "$auth_file" "$tag" "$release_commit" "$remote"
  printf 'Wrote tag auth state: %s\n' "$auth_file"

  if [[ -n "$push_remote" ]]; then
    if git -C "$ROOT_DIR" push "$remote" "$tag"; then
      rm -f "$auth_file" "$active_file" || die "tag pushed, but failed to clear release state"
      printf 'Pushed tag %s to %s.\n' "$tag" "$remote"
      printf 'Cleared release state.\n'
    else
      printf 'Failed to push tag %s to %s; release state left for retry.\n' "$tag" "$remote" >&2
      exit 1
    fi
  else
    printf '\nNothing pushed. Run:\n'
    printf '  git push %s %s\n' "$remote" "$tag"
    printf 'Then clear release state with scripts/release.sh abort if no longer needed.\n'
  fi
}

cmd_status() {
  local active_file
  active_file="$(active_json)"

  if [[ ! -f "$active_file" ]]; then
    printf 'No release in progress\n'
    return 0
  fi

  printf 'Release in progress:\n'
  printf '  version: %s\n' "$(json_field "$active_file" version)"
  printf '  tag: %s\n' "$(json_field "$active_file" tag)"
  printf '  branch: %s\n' "$(json_field "$active_file" branch)"
  printf '  commit: %s\n' "$(json_field "$active_file" release_commit)"
  printf '  prepared_at: %s\n' "$(json_field "$active_file" prepared_at)"
  printf '  remote: %s\n' "$(json_field "$active_file" remote)"
  printf '  state: %s\n' "$active_file"
}

cmd_abort() {
  local active_file auth_file tag
  active_file="$(active_json)"
  auth_file="$(auth_json)"

  if [[ -f "$active_file" ]]; then
    tag="$(json_field "$active_file" tag)"
    rm -f "$active_file" "$auth_file" || die "failed to clear release state"
    printf 'Cleared release state. Release commit remains in history.\n'
  else
    rm -f "$auth_file" || die "failed to clear tag auth state"
    printf 'No release in progress. Cleared tag auth state if present.\n'
  fi

  if [[ -n "$tag" ]] && git -C "$ROOT_DIR" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    printf 'Notice: local tag %s still exists. Delete manually with: git tag -d %s\n' "$tag" "$tag"
  fi
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
    prepare)
      shift
      cmd_prepare "$@"
      ;;
    resume)
      shift
      cmd_resume "$@"
      ;;
    status)
      shift
      [[ $# -eq 0 ]] || die "status does not accept arguments"
      cmd_status
      ;;
    abort)
      shift
      [[ $# -eq 0 ]] || die "abort does not accept arguments"
      cmd_abort
      ;;
    patch|minor|major)
      cmd_prepare "$@"
      ;;
    *)
      if validate_version "$1"; then
        cmd_prepare "$@"
      else
        die "unknown subcommand or target: $1"
      fi
      ;;
  esac
}

main "$@"
