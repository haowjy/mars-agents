# Smoke Testing

This page documents the manual smoke checks that are worth running after parser, discovery, fetch, or sync changes.

Use these when:
- changing source parsing in `src/source/parse.rs`
- changing package rooting or subpath handling
- changing discovery behavior in `src/discover/`
- changing sync behavior that affects installed output shape
- preparing a release that touches any of the above

These are not meant to replace unit or integration tests. They are short end-to-end checks for the highest-risk user flows.

## Baseline

Always start with the deterministic local checks:

```bash
cargo fmt --all
cargo test -q
```

## Local Path + `--subpath`

Verifies local source parsing, subpath rooting, discovery, install, and doctor.

```bash
tmpdir=$(mktemp -d)
proj="$tmpdir/project"
src="$tmpdir/source"

mkdir -p "$proj" "$src/plugins/foo/skills/planning" "$src/plugins/foo/agents"

cat > "$src/plugins/foo/skills/planning/SKILL.md" <<'EOF'
---
name: planning
description: local planning
---
# Planning
EOF

cat > "$src/plugins/foo/agents/coder.md" <<'EOF'
---
name: coder
description: local coder
skills:
  - planning
---
# Coder
EOF

mars init --root "$proj"
mars add "$src" --subpath plugins/foo --root "$proj"
mars doctor --root "$proj"
```

Expected result:
- `mars add` succeeds
- one agent and one skill are installed
- `mars doctor` exits cleanly

## GitHub Repo Add

Verifies the most common hosted-source path, including transitive discovery behavior.

```bash
tmpdir=$(mktemp -d)
mars init --root "$tmpdir"
mars add meridian-flow/meridian-dev-workflow --root "$tmpdir"
mars doctor --root "$tmpdir"
```

Expected result:
- `mars add` succeeds
- `mars doctor` exits cleanly

This is also the regression check for the historical `caveman` fallback-discovery issue.

## Generic `git://` URL

Verifies generic git transport handling without relying on GitHub or GitLab-specific parsing.

```bash
root=$(mktemp -d)
repo="$root/export/group/pkg"
bare="$root/export/group/pkg.git"
proj="$root/project"

mkdir -p "$repo/skills/planning" "$repo/agents" "$(dirname "$bare")"

cat > "$repo/skills/planning/SKILL.md" <<'EOF'
---
name: planning
description: daemon planning
---
# Planning
EOF

cat > "$repo/agents/coder.md" <<'EOF'
---
name: coder
description: daemon coder
skills:
  - planning
---
# Coder
EOF

(
  cd "$repo"
  git init -q
  git config user.name smoke
  git config user.email smoke@example.com
  git add .
  git commit -qm init
)

git clone -q --bare "$repo" "$bare"
git daemon --export-all --base-path="$root/export" --reuseaddr --listen=127.0.0.1 --port=19423 "$root/export" >/tmp/mars_gitd_19423.log 2>&1 &
pid=$!
trap 'kill $pid >/dev/null 2>&1 || true' EXIT
sleep 1

mars init --root "$proj"
mars add 'git://127.0.0.1:19423/group/pkg.git' --root "$proj"
mars doctor --root "$proj"
```

Expected result:
- `mars add` succeeds
- `mars doctor` exits cleanly

## GitLab-Style Host With Explicit Port

Verifies GitLab-like host detection plus explicit-port preservation during URL normalization.

```bash
root=$(mktemp -d)
repo="$root/export/group/pkg"
bare="$root/export/group/pkg.git"
proj="$root/project"

mkdir -p "$repo/skills/planning" "$repo/agents" "$(dirname "$bare")"

cat > "$repo/skills/planning/SKILL.md" <<'EOF'
---
name: planning
description: gitlab planning
---
# Planning
EOF

cat > "$repo/agents/reviewer.md" <<'EOF'
---
name: reviewer
description: gitlab reviewer
skills:
  - planning
---
# Reviewer
EOF

(
  cd "$repo"
  git init -q
  git config user.name smoke
  git config user.email smoke@example.com
  git add .
  git commit -qm init
)

git clone -q --bare "$repo" "$bare"
git daemon --export-all --base-path="$root/export" --reuseaddr --listen=0.0.0.0 --port=19424 "$root/export" >/tmp/mars_gitd_19424.log 2>&1 &
pid=$!
trap 'kill $pid >/dev/null 2>&1 || true' EXIT
sleep 1

mars init --root "$proj"
mars add 'git://gitlab.localtest.me:19424/group/pkg.git' --root "$proj"
mars doctor --root "$proj"
```

Expected result:
- `mars add` succeeds
- `mars doctor` exits cleanly

## Archive / Download Rejection

Verifies that unsupported source forms still fail clearly.

```bash
tmpdir=$(mktemp -d)
mars init --root "$tmpdir"
mars add 'https://github.com/owner/repo/archive/refs/heads/main.zip' --root "$tmpdir"
```

Expected result:
- command fails
- error explains that archive-download URLs are unsupported in v1

## When To Run Which Checks

Run this minimum set for parser-only changes:
- baseline
- archive / download rejection
- one hosted source relevant to the parser change

Run this minimum set for discovery changes:
- baseline
- local path + `--subpath`
- GitHub repo add
- `mars doctor`

Run this minimum set for transport or source-normalization changes:
- baseline
- generic `git://` URL
- GitLab-style host with explicit port
- one GitHub repo add

Run the full page before a release if the release includes parser, rooting, discovery, or sync behavior changes.
