# `mars add` Design Note

## Problem

`mars add` currently models one source at a time:

```bash
mars add <source>
```

That is clean for config authoring, but it misses two common workflows:

1. Bootstrap several whole sources at once.

```bash
mars add haowjy/meridian-base haowjy/meridian-dev-workflow
```

2. Add one mixed source but only one category of items from it.

```bash
mars add anthropic/skills --only-skills
```

The command should stay source-oriented, map cleanly to `mars.toml`, and avoid becoming an inline DSL.

## Domain Model

`mars` is not quite like npm or pip.

- In npm or pip, each argument is already a package.
- In `mars`, each argument is a source.
- A source may expose many installable units: agents, skills, or both.
- Some sources are cohesive bundles; some are loose collections.

That means `mars add` needs to support both:

- whole-source registration
- source-local filtering

The CLI should reflect the same model as `mars.toml`: one source entry with optional filter state.

## Proposed UX

### Primary forms

```bash
mars add <source>
mars add <source> --agents reviewer,planner
mars add <source> --skills frontend-design,git-worktree
mars add <source> --only-skills
mars add <source> --only-agents
mars add <source> --exclude legacy-agent,old-skill
```

### Convenience shorthand

```bash
mars add <source1> <source2> ...
```

This shorthand is only for whole-source adds.

## Rules

### Default mode

```bash
mars add <source>
```

No filter means install everything from the source.

### Include mode

```bash
mars add <source> --agents a,b
mars add <source> --skills x,y
mars add <source> --agents a,b --skills x,y
```

If `--agents` or `--skills` is present, `mars` enters include mode.

- Only the named agents and/or named skills are installed.
- If named agents reference skills in frontmatter, their transitive skill dependencies are also installed.

### Category-only mode

```bash
mars add <source> --only-skills
mars add <source> --only-agents
```

- `--only-skills`: install all discovered skills, no agents.
- `--only-agents`: install all discovered agents, plus required supporting skills referenced by those agents, but no unrelated standalone skills.

### Exclude mode

```bash
mars add <source> --exclude a,b
```

Install everything except the named items.

### Multi-source shorthand restriction

If any source-local filter flag is used, exactly one source must be provided.

Allowed:

```bash
mars add source1 source2
mars add source --only-skills
mars add source --agents reviewer
```

Rejected:

```bash
mars add source1 source2 --skills git
mars add source1 source2 --only-skills
```

## Validation Rules

Reject these combinations:

- `--only-skills` with `--only-agents`
- `--only-skills` with `--agents`
- `--only-agents` with `--skills`
- `--exclude` with any include-mode or category-only flags
- any filter flag with more than one source

Suggested error messages:

- `filters may only be used when adding exactly one source`
- `--only-skills cannot be combined with --agents`
- `--only-agents cannot be combined with --skills`
- `cannot combine --exclude with include filters`

## `mars.toml` Representation

The CLI should serialize cleanly to one dependency entry. (This design assumes the unified `[dependencies]` schema from the root config redesign is already in place — there is no `[sources]` section.)

Current config already supports:

- `agents = [...]`
- `skills = [...]`
- `exclude = [...]`

Add two booleans:

```toml
[dependencies.base]
url = "https://github.com/haowjy/meridian-base"
only_skills = true

[dependencies.dev]
url = "https://github.com/haowjy/meridian-dev-workflow"
```

And:

```toml
[dependencies.ops]
url = "https://github.com/acme/ops-agents"
only_agents = true
```

Validation:

- `only_skills` and `only_agents` are mutually exclusive
- neither may appear with `agents`, `skills`, or `exclude`

This is better than using sentinel values such as `"*"` because it is explicit in both CLI and TOML.

## Internal Model

Current filter state should expand from:

```rust
enum FilterMode {
    All,
    Include { agents: Vec<ItemName>, skills: Vec<ItemName> },
    Exclude(Vec<ItemName>),
}
```

to:

```rust
enum FilterMode {
    All,
    Include { agents: Vec<ItemName>, skills: Vec<ItemName> },
    Exclude(Vec<ItemName>),
    OnlySkills,
    OnlyAgents,
}
```

That preserves the current model instead of overloading include lists with special meanings.

Note: After the root config redesign, `FilterMode` lives on the unified `DependencyEntry` struct (which replaced the old `SourceEntry`). The filter fields (`agents`, `skills`, `exclude`, `only_skills`, `only_agents`) are part of this unified struct alongside `url`, `path`, `version`, and `rename`.

---

## Hardened Semantics

The sections below resolve gaps, edge cases, and ambiguities not covered by the original proposal.

### 1. Idempotency: Re-adding an Existing Source

**Current behavior:** `mars add` uses `ConfigMutation::UpsertDependency` (formerly `UpsertSource`). If the dependency name already exists, it merges — updating `url`/`path`/`version` unconditionally, and overwriting each filter field (`agents`, `skills`, `exclude`) only when the new entry explicitly sets it (i.e., the CLI flag was provided). Rename rules are never touched by add.

**Problem:** This merge-on-presence semantic creates a subtle trap with the new filter modes. Consider:

```bash
mars add source --agents reviewer      # sets Include{agents: [reviewer]}
mars add source --only-skills           # intends to switch to OnlySkills
```

Under the current merge logic, the second command would set `only_skills = true` but would **not** clear the previously-set `agents` field, leaving the TOML in an invalid state (both `agents` and `only_skills` present).

**Resolution: Filter replacement, not filter merging.** When any filter-related flag is provided on a re-add, the entire filter config is replaced atomically — all prior filter fields (`agents`, `skills`, `exclude`, `only_skills`, `only_agents`) are cleared, and the new filter state is written fresh. This prevents mixed-mode TOML and matches user intent: a re-add with `--only-skills` means "I want only skills now," not "I want only skills in addition to the previously configured agents."

When **no** filter flags are provided on a re-add (e.g., `mars add source@v2` to bump version), existing filters are preserved. This allows version bumps without restating filter config.

**Implementation:** In `apply_mutation`, check whether the incoming `DependencyEntry` has any filter state set. If yes, replace `existing.filter` wholesale. If no filter fields are set, preserve the existing filter.

```rust
let has_any_filter = entry.filter.agents.is_some()
    || entry.filter.skills.is_some()
    || entry.filter.exclude.is_some()
    || entry.filter.only_skills
    || entry.filter.only_agents;

if has_any_filter {
    existing.filter = entry.filter.clone(); // atomic replacement
}
// else: preserve existing filters (version bump, URL change, etc.)
```

**User messaging:** When a re-add changes filters, print both old and new filter state so the user sees what changed:

```
dependency `ops` already exists — updated
  filters changed: agents=[reviewer] → only_skills=true
```

### 2. Filter Interaction with Existing Config

Covered by the atomic filter replacement rule above. Specific scenarios:

| Existing config | New command | Result |
|---|---|---|
| `agents = ["reviewer"]` | `mars add source --only-skills` | `only_skills = true` (agents cleared) |
| `only_skills = true` | `mars add source --agents coder` | `agents = ["coder"]` (only_skills cleared) |
| `agents = ["reviewer"]` | `mars add source@v2` (no filter flags) | `agents = ["reviewer"]` preserved, version updated |
| `exclude = ["legacy"]` | `mars add source` (no filter flags) | `exclude = ["legacy"]` preserved |
| `exclude = ["legacy"]` | `mars add source --exclude legacy,old` | `exclude = ["legacy", "old"]` (full replacement) |
| `agents = ["a"], skills = ["b"]` | `mars add source --agents a,c` | `agents = ["a", "c"]` (skills cleared — atomic replacement) |

The last row is the most surprising case. Include mode means `--agents` and `--skills` together; providing only `--agents` on re-add means "I want only these agents (plus transitive skill deps)." If the user also wants named skills, they must re-specify them. This is correct — partial filter merging creates states the user didn't intend.

### 3. Transitive Skill Dependencies vs. Explicit Excludes

**Scenario:** Source exposes agents `[coder, reviewer]` and skills `[planning, review, deprecated]`. Coder's frontmatter declares `skills: [planning]`.

```bash
mars add source --agents coder --exclude planning
```

This command is **rejected at validation** — `--exclude` cannot combine with include-mode flags (`--agents`). This is already in the validation rules.

The real question is: what if you want agent `coder` but not its transitive dep `planning`?

**Resolution:** This is not supported in the first version. Include mode installs named agents + their transitive skill deps; there is no way to suppress a transitive dep. The rationale: an agent that declares a skill dependency presumably needs it. Installing the agent without its skills creates a broken runtime state (mars already emits `ValidationWarning::MissingSkill` for this).

If this becomes a real user need, the future path is a per-agent exclude syntax in TOML (e.g., agent-level overrides). Not this version.

**What about `OnlyAgents` mode?** `--only-agents` installs all agents plus their transitive skill deps but no unrelated standalone skills. If a user then manually edits `mars.toml` to add `exclude = ["planning"]` alongside `only_agents = true`, validation rejects it — `exclude` and `only_agents` are mutually exclusive. Consistent with the validation rules.

### 4. Naming Collisions

**Current behavior:** Mars already handles naming collisions via auto-rename. When two sources expose an item with the same destination path, both are suffixed with `__{owner}_{repo}` derived from the source URL or name. Agent frontmatter is rewritten to reference the renamed skills.

**What `mars add` does:** Nothing special — collision detection and auto-rename happen during `sync`, not during `add`. The add command registers the dependency; sync materializes it. If adding a new dependency introduces a collision with an existing one, sync detects it and auto-renames both items.

**User-visible impact:** After `mars add dep-b`, a previously-installed `planning` skill from `dep-a` may suddenly get renamed to `planning__dep-a_x`. This is surprising but correct — the alternative (silently shadowing) is worse.

**Recommendation:** When sync detects new collisions introduced by the current mutation, log them prominently:

```
⚠ naming collision: skill `planning` exists in both `dep-a` and `dep-b`
  auto-renamed to `planning__alice_agents` and `planning__bob_agents`
  agent frontmatter updated to reference new names
```

This is not a design change — it's a UX improvement for the existing collision system.

### 5. Version/Ref Interaction with Filters

**Filters are per-dependency, not per-version.** A dependency entry in `mars.toml` has one version constraint and one filter config. Changing the version does not change the filter, and vice versa.

```bash
mars add source@v1 --agents coder       # v1, Include{agents: [coder]}
mars add source@v2                       # v2, Include{agents: [coder]} (preserved)
mars add source@v2 --only-skills         # v2, OnlySkills (filter replaced)
```

**Edge case: version bump adds new agents/skills.** If `source@v1` has `[coder]` and `source@v2` adds `[reviewer]`, a filter of `--agents coder` still only installs `coder` after the version bump. This is correct — the filter is the user's intent, not a snapshot of what was available at add-time.

**Edge case: version bump removes a filtered item.** If `source@v1` has skill `planning` and `source@v2` removes it, but the filter includes `planning`, sync proceeds without it (the item simply won't be discovered). Mars already handles this gracefully — Include mode filters against what's discovered, so missing items are silently absent. However, if an agent depends on the now-missing skill, `ValidationWarning::MissingSkill` fires. This is the right behavior.

### 6. Validation Timing

**Filter names are NOT validated at add-time.** They are validated at sync-time against discovered items.

**Rationale:**
- Add-time validation would require fetching the source tree, which is expensive and makes `mars add` slow for a config-authoring operation.
- The source might not be fetchable at add-time (network down, private repo not yet configured).
- Mars already validates during sync — include-mode items that don't match any discovered item are simply absent, and missing skill references produce warnings.

**However, structural validation IS done at add-time:**
- Flag combinations are validated immediately (e.g., `--exclude` with `--agents` → error).
- TOML structural integrity is validated when writing config.
- Empty filter lists are allowed (`--agents` with no value → clap error, since `value_delimiter` requires at least one value).

**Recommendation: Add a sync-time warning for unmatched filter names.** If a user writes `--agents nonexistent`, sync should warn:

```
⚠ dependency `ops`: filter references agent `nonexistent` which was not found
```

This is a warning, not an error — the source might add that agent in a future version, and erroring would break `--frozen` workflows where the config is committed ahead of the source update.

### 7. UX Edge Cases

**`--only-agents` on a source with zero agents:**
- Sync discovers zero agents, installs nothing. No error — this is a valid (if useless) configuration.
- Print an informational message: `dependency 'skills-only-repo' has no agents (--only-agents selected)`
- Rationale: erroring would break automation that applies `--only-agents` uniformly. A warning gives the user signal without failing the pipeline.

**`--only-skills` on a source with zero skills:**
- Same treatment: informational message, not error.

**`--agents coder` on a source that has no agent named `coder`:**
- Sync-time warning (see §6). No error.

**Empty `--exclude` list:**
- Clap requires at least one value when `value_delimiter` is set. If somehow an empty list reaches the config, it's equivalent to `All` mode. Normalize: if `exclude` is `Some([])`, treat as `FilterMode::All`.

**`mars add source` when dependency already exists with identical config:**
- Sync runs but computes no diff. Output: `dependency 'base' already exists — no changes`. Exit 0.

**`mars add source --agents a,b` when the dependency already has `agents = ["a", "b"]` exactly:**
- Atomic filter replacement writes the same value. Sync runs, no diff. Idempotent.

### 8. `mars remove` Symmetry

**`mars remove` remains whole-source only.** It does not need filter removal support.

**Rationale:**
- Removing individual items from a source is better modeled as a filter change: `mars add source --exclude item` or `mars add source --agents remaining-ones`.
- A hypothetical `mars remove source --agents coder` conflates two operations: "remove the dependency entirely" vs. "modify the dependency's filter." The verb "remove" implies deletion; filter changes are updates.
- The current `mars remove` already removes all items from the dependency via lock file comparison during sync. Adding filter-awareness to remove would complicate the mutation model for minimal UX gain.

**If a user wants to narrow an existing source:**
```bash
mars add source --agents reviewer   # re-add with narrower filter (replaces old filter)
```

This is explicit and uses the existing re-add/upsert path.

### 9. `FilterConfig` Schema Changes

The current `FilterConfig` struct (on the unified `DependencyEntry` from the root config redesign) needs two new boolean fields:

```rust
pub struct FilterConfig {
    pub agents: Option<Vec<ItemName>>,
    pub skills: Option<Vec<ItemName>>,
    pub exclude: Option<Vec<ItemName>>,
    pub rename: Option<RenameMap>,
    #[serde(default)]
    pub only_skills: bool,
    #[serde(default)]
    pub only_agents: bool,
}
```

**TOML validation** (in `merge()` or a dedicated `validate_config()` step):

```rust
fn validate_filter(filter: &FilterConfig) -> Result<(), ConfigError> {
    let has_include = filter.agents.is_some() || filter.skills.is_some();
    let has_exclude = filter.exclude.is_some();
    let has_category = filter.only_skills || filter.only_agents;

    if filter.only_skills && filter.only_agents {
        return Err("only_skills and only_agents are mutually exclusive");
    }
    if has_category && has_include {
        return Err("only_skills/only_agents cannot combine with agents/skills lists");
    }
    if has_category && has_exclude {
        return Err("only_skills/only_agents cannot combine with exclude");
    }
    if has_include && has_exclude {
        return Err("agents/skills lists cannot combine with exclude");
    }
    Ok(())
}
```

This validation runs both at config load time (catches hand-edited TOML errors) and at CLI parse time (catches flag combination errors before writing config).

### 10. `apply_filter` Changes for New Variants

The `apply_filter` function in `sync/target.rs` needs two new match arms:

```rust
FilterMode::OnlySkills => {
    Ok(discovered.iter()
        .filter(|item| item.kind == ItemKind::Skill)
        .cloned()
        .collect())
}

FilterMode::OnlyAgents => {
    // Step 1: Include all agents
    let agents: Vec<_> = discovered.iter()
        .filter(|item| item.kind == ItemKind::Agent)
        .cloned()
        .collect();

    // Step 2: Resolve transitive skill deps from agent frontmatter
    // Reuse existing helper from validate module (same pattern as Include mode)
    let mut skill_deps: HashSet<ItemName> = HashSet::new();
    for agent in &agents {
        let agent_path = tree_path.join(&agent.source_path);
        let deps = validate::parse_agent_skills(&agent_path).unwrap_or_default();
        for skill in deps {
            skill_deps.insert(ItemName::from(skill));
        }
    }

    // Step 3: Include agents + their transitive skill deps only
    let skills: Vec<_> = discovered.iter()
        .filter(|item| item.kind == ItemKind::Skill && skill_deps.contains(&item.name))
        .cloned()
        .collect();

    let mut result = agents;
    result.extend(skills);
    Ok(result)
}
```

Note: `OnlyAgents` shares the transitive-dep resolution logic with `Include` mode's agent handling. Factor this into a shared helper to avoid duplication.

### 11. Multi-source Add Implementation

The `AddArgs` struct changes from a single `source: String` to `sources: Vec<String>`:

```rust
#[derive(Debug, clap::Args)]
pub struct AddArgs {
    /// Source specifiers (one or more).
    #[arg(required = true)]
    pub sources: Vec<String>,

    #[arg(long, value_delimiter = ',')]
    pub agents: Vec<String>,

    #[arg(long, value_delimiter = ',')]
    pub skills: Vec<String>,

    #[arg(long, value_delimiter = ',')]
    pub exclude: Vec<String>,

    #[arg(long)]
    pub only_skills: bool,

    #[arg(long)]
    pub only_agents: bool,
}
```

**Validation** (before any mutation):

```rust
let has_filters = !args.agents.is_empty()
    || !args.skills.is_empty()
    || !args.exclude.is_empty()
    || args.only_skills
    || args.only_agents;

if has_filters && args.sources.len() > 1 {
    return Err("filters may only be used when adding exactly one source");
}
```

**Multi-dependency execution:** Each dependency is added as a separate `UpsertDependency` mutation. All mutations are applied to the config before a single sync runs. This is more efficient than running N syncs and avoids intermediate states where half the dependencies are synced.

```rust
// Apply all mutations to config, then sync once
let mutations: Vec<ConfigMutation> = args.sources.iter()
    .map(|s| parse_and_build_mutation(s, &filter_config))
    .collect::<Result<Vec<_>, _>>()?;

// New: SyncRequest accepts Vec<ConfigMutation>
// Or: apply mutations sequentially to config in-memory, then sync
```

**Alternative (simpler, recommended for v1):** Loop over dependencies, apply each `UpsertDependency` mutation sequentially, run one sync at the end. The config mutations are cheap; sync is the expensive part.

---

## Why This Design

### Why not overload `--skills`

`--skills` already means "include these named skills."

Changing it to sometimes mean "all skills" when no value is supplied would be ambiguous and harder to document.

### Why not add `--all-skills`

`--all-skills` would be less clear than `--only-skills`.

The real user intent is category restriction, not "turn on filter mode and include all skills."

### Why not use inline source mini-language syntax

Avoid forms like:

```bash
mars add source::agent1,agent2;skill1,skill2
```

Problems:

- hard to discover in `--help`
- awkward in shells because of `;`
- mixes source parsing with filter parsing
- becomes messy with versions, URLs, and local paths

### Why not use ordered `--source` groups yet

Avoid forms like:

```bash
mars add --source source1 --skills git --source source2 --agents reviewer
```

This is viable, but it is more advanced:

- order-dependent
- harder to explain precisely
- more complex to parse
- more complex to serialize back to config

It solves a real power-user problem, but it is not the smallest good design for now.

## Comparison to Other CLIs

This design intentionally borrows the right lessons from other successful CLIs without copying the wrong assumptions.

## What Is Borrowed vs Novel

### Borrowed semantics

The proposal does not invent a new command language from scratch.

It deliberately borrows familiar patterns:

- multi-positional add for whole-source convenience
- explicit include-list flags
- explicit category-only mode flags
- source-local validation instead of cross-source flag ambiguity

Those ideas are all common in mature CLIs.

### Novel part

What is different is the domain model, not the syntax.

`mars` is operating on:

- sources, not direct packages
- sources that may contain many installable units
- managed projection into `.agents/`
- lockfile-backed ownership and sync behavior

So the novelty is not "inventing a new CLI grammar." The novelty is applying familiar package-manager and config-CLI patterns to a source-of-many-agent-assets model.

That is why the design prefers small, explicit additions over a bespoke mini-language.

### npm install

`npm install` accepts multiple package specs in one command:

```text
npm install [<package-spec> ...]
```

Source:
<https://docs.npmjs.com/cli/v11/commands/npm-install/>

Why it differs from `mars`:

- each argument is already a package
- most flags apply to the entire invocation

That makes plain multi-add natural for npm in a way it is not for source-local `mars` filters.

### pip install

`pip install` also accepts multiple requirements and applies shared options across them:

```text
python -m pip install SomePackage1 SomePackage2 --no-binary :all:
python -m pip install SomePackage1 SomePackage2 --no-binary SomePackage1
```

Source:
<https://pip.pypa.io/en/stable/cli/pip_install/>

Why it differs from `mars`:

- pip installs packages directly, not source catalogs that may contain many smaller packages
- even when pip scopes an option to one package, it still operates in a package-oriented model

### cargo add

`cargo add` supports multiple crates:

```text
cargo add [options] crate…
```

And when features need to be scoped to a specific crate, Cargo uses a self-contained qualifier:

```text
package-name/feature-name
```

Source:
<https://doc.rust-lang.org/cargo/commands/cargo-add.html>

Why it matters:

- Cargo shows a real pattern for multi-target add plus scoped options.
- If `mars` ever needs per-source filtering in one command, a self-contained scoped form is more defensible than order-dependent flags.

### docker buildx bake

Docker Bake supports repeatable target-specific overrides:

```text
--set targetpattern.key=value
```

Source:
<https://docs.docker.com/reference/cli/docker/buildx/bake/>

Why it matters:

- target identity is embedded in the flag value
- the CLI does not depend on "the previous flag changed context"

This is a strong pattern for advanced configuration CLIs, but probably too heavy for the default `mars add` UX.

### kubectl create secret generic

Kubernetes uses repeatable self-contained flags:

```text
--from-literal=key=value
--from-file=name=path
```

Source:
<https://kubernetes.io/docs/reference/kubectl/generated/kubectl_create/kubectl_create_secret_generic/>

Why it matters:

- each repeated flag carries its own small record
- this scales better than context-sensitive flag ordering

Again, this is useful precedent if `mars` later needs a power-user repeated source-spec mode.

## Recommendation

Ship the smallest coherent upgrade:

1. Add multi-source shorthand for whole-source adds.
2. Add `--only-skills` and `--only-agents`.
3. Keep filters source-local and require exactly one source when filters are present.
4. Do not add inline mini-language syntax.
5. Do not add ordered `--source` grouping yet.

This preserves a clean source-oriented model, produces good `mars.toml`, and covers the most common workflows without turning `mars add` into a DSL.

## Implementation Notes

### Migration: Existing `apply_mutation` Must Change

The current merge logic in `apply_mutation` updates filter fields individually. This must change to atomic filter replacement when any filter flag is present. The change is backwards-compatible — existing `mars.toml` files with only `agents`/`skills`/`exclude` fields continue to work because `only_skills` and `only_agents` default to `false`.

### Multi-mutation Sync

For multi-dependency add, the sync pipeline already supports a single `ConfigMutation` per request. Either:
- (a) Extend `ConfigMutation` to accept a batch, or
- (b) Apply mutations to the in-memory config sequentially before entering the sync pipeline.

Option (b) is simpler and sufficient — config mutations are cheap, and running sync once at the end is the right optimization target.

### `FilterConfig.default()` Semantics

`FilterConfig::default()` currently returns all-`None` fields. Adding `only_skills: false` and `only_agents: false` preserves this — `#[serde(default)]` handles TOML files that don't mention these fields.
