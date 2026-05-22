# Smoke: target-scoped linked-target ownership (#60)

Run from a scratch project using the worktree binary (no install):

```bash
cd /path/to/mars-agents.worktrees/target-orphan-cleanup
MARS=$(pwd)/target/debug/mars
cargo build -q
```

## Sync preserves hand-written `.cursor` agents

```bash
tmpdir=$(mktemp -d)
cd "$tmpdir"
echo '[settings]
targets = [".cursor"]
agent_emission = "never"

[dependencies.base]
path = "/path/to/local/source"
' > mars.toml

$MARS sync
mkdir -p .cursor/agents
echo '# custom' > .cursor/agents/cursor-only-test.md
echo '# hand-written' > .cursor/agents/design-lead.md
$MARS sync
cat .cursor/agents/design-lead.md   # expect: # hand-written
```

## Link fails without `--force` on collision

Uses default agent emission (not `never`) so link materializes agents into the target.

```bash
# mars.toml: [dependencies.base] path = ... only (no agent_emission = "never")
$MARS sync
mkdir -p .agents/agents
echo '# hand-written' > .agents/agents/coder.md
$MARS link .agents            # expect: non-zero exit
$MARS link .agents --force    # expect: success; mars.lock has target_root = ".agents" for agents/coder.md
```

## Sync `--force` overwrites divergent managed file

```bash
# mars.toml: targets = [".agents"], [dependencies.base] path = ...
$MARS sync
echo '# Hand-edited' > .agents/agents/coder.md
$MARS sync                    # expect: preserves Hand-edited
$MARS sync --force            # expect: restores Mars canonical content
```

Or run all checks: `scripts/smoke-target-scoped.sh` from the worktree root.
