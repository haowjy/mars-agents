#!/usr/bin/env bash
set -euo pipefail
WORKTREE="$(cd "$(dirname "$0")/.." && pwd)"
cd "$WORKTREE"
cargo build -q
MARS="$WORKTREE/target/debug/mars"
SCRATCH=$(mktemp -d)
echo "SCRATCH=$SCRATCH"
PASS=0
FAIL=0
pass() { echo "PASS: $1"; PASS=$((PASS+1)); }
fail() { echo "FAIL: $1"; FAIL=$((FAIL+1)); }

# --- SMOKE 1: .opencode + never preserves hand-written agents ---
mkdir -p "$SCRATCH/src/agents"
echo '# Design lead from Mars' > "$SCRATCH/src/agents/design-lead.md"
PROJ1="$SCRATCH/smoke1"
mkdir -p "$PROJ1"
cat > "$PROJ1/mars.toml" << EOF
[settings]
targets = [".opencode"]
agent_emission = "never"

[dependencies.src]
path = "../src"
EOF
cd "$PROJ1"
"$MARS" sync --root . >/dev/null 2>&1
test -d .mars && pass "SMOKE1: initial sync creates .mars" || fail "SMOKE1: no .mars"
mkdir -p .opencode/agents
echo '# custom' > .opencode/agents/opencode-only-test.md
echo '# hand-written' > .opencode/agents/design-lead.md
"$MARS" sync --root . >/dev/null 2>&1
if [ "$(cat .opencode/agents/design-lead.md)" = "# hand-written" ]; then
  pass "SMOKE1: design-lead preserved after second sync"
else
  fail "SMOKE1: design-lead changed: $(cat .opencode/agents/design-lead.md)"
fi
test -f .opencode/agents/opencode-only-test.md && pass "SMOKE1: opencode-only-test preserved" || fail "SMOKE1: opencode-only-test missing"

# --- SMOKE 2: link .agents collision (integration-style: no never) ---
mkdir -p "$SCRATCH/src2/agents"
echo '# Coder from Mars' > "$SCRATCH/src2/agents/coder.md"
PROJ2="$SCRATCH/smoke2"
mkdir -p "$PROJ2"
cat > "$PROJ2/mars.toml" << EOF
[dependencies.base]
path = "../src2"
EOF
cd "$PROJ2"
"$MARS" sync --root . >/dev/null 2>&1
mkdir -p .agents/agents
echo '# hand-written' > .agents/agents/coder.md
set +e
"$MARS" link .agents --root . >/dev/null 2>&1
LINK_EXIT=$?
set -e
if [ "$LINK_EXIT" -ne 0 ]; then
  pass "SMOKE2a: link .agents without --force exits non-zero ($LINK_EXIT)"
else
  fail "SMOKE2a: link .agents without --force should fail (got 0)"
fi
if [ "$(cat .agents/agents/coder.md)" = "# hand-written" ]; then
  pass "SMOKE2a: hand-written coder preserved after failed link"
else
  fail "SMOKE2a: coder overwritten before force"
fi
"$MARS" link .agents --force --root . >/dev/null 2>&1
if [ "$(cat .agents/agents/coder.md)" = "# Coder from Mars" ]; then
  pass "SMOKE2b: link --force adopts collision (content from Mars)"
else
  fail "SMOKE2b: coder content after force: $(cat .agents/agents/coder.md)"
fi
if grep -q 'target_root = ".agents"' mars.lock 2>/dev/null; then
  pass "SMOKE2b: lock records target_root = .agents"
else
  fail "SMOKE2b: no target_root = .agents in mars.lock"
fi

# --- SMOKE 3: sync --force overwrites divergent .agents file ---
PROJ3="$SCRATCH/smoke3"
mkdir -p "$PROJ3"
cat > "$PROJ3/mars.toml" << EOF
[settings]
targets = [".agents"]

[dependencies.base]
path = "../src2"
EOF
cd "$PROJ3"
"$MARS" sync --root . >/dev/null 2>&1
echo '# Hand-edited' > .agents/agents/coder.md
"$MARS" sync --root . >/dev/null 2>&1
if [ "$(cat .agents/agents/coder.md)" = "# Hand-edited" ]; then
  pass "SMOKE3a: sync without --force preserves divergent .agents file"
else
  fail "SMOKE3a: divergent file overwritten without force"
fi
"$MARS" sync --force --root . >/dev/null 2>&1
if [ "$(cat .agents/agents/coder.md)" = "# Coder from Mars" ]; then
  pass "SMOKE3b: sync --force restores canonical .agents content"
else
  fail "SMOKE3b: content after force: $(cat .agents/agents/coder.md)"
fi

echo ""
echo "=== SUMMARY: $PASS passed, $FAIL failed (scratch: $SCRATCH) ==="
exit "$FAIL"
