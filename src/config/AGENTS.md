# src/config/ — Config Loading & Routing Settings

User network defaults plus mars.toml + mars.local.toml schemas, load/save, and merge to
EffectiveConfig.

## Mental Model

```
mars.toml (Config) + mars.local.toml (LocalConfig)
    ↓ merge_with_root()
EffectiveConfig { dependencies, settings }
    ↓
pipeline operates on EffectiveConfig only
```

## Key Types

| Type | Role |
|---|---|
| `Config` | Full mars.toml: package, plugin, dependencies, local-dependencies, settings, models, agents |
| `PluginConfig` | Committed plugin output directory, identity, description, and selected harnesses |
| `PluginHarness` | The five harness names accepted by plugin builds; its default set is all five |
| `LocalConfig` | Gitignored dev overrides: source path swaps, local agent overlays |
| `EffectiveConfig` | Merged result — what the pipeline operates on |
| `EffectiveDependency` | Resolved source with override tracking (`is_overridden`, `original_git`), plus `dialect: Option<Dialect>` for inbound lift dialect |
| `SkillOverlay` | `[skills.<name>]` overlay — `description`, `model_invocable`, `user_invocable`, `tools` (`AgentOverlayTools`) |
| `AgentOverlay` | `[agents.<name>]` overlay — launch scalars, `network-allow` / `network-deny`, invocability, tools, and model policies |
| `NetworkPolicy` / `NetworkHost` | Shared typed parsers for every mode and host surface |

`load()` checks only the structural relationship that `[plugin]` needs `[package]`; plugin author,
dir, identity, marketplace, and requested-harness semantics belong to
`build::plugin::stage_plugin` so a bad publish configuration cannot block `sync` or `list`.
Unknown requested harness spellings deserialize to a typed `Unknown(String)` value that retains the
author's spelling for build errors and `mars check` warnings, and are rejected at the build
boundary. Dependency `load_manifest()` uses a narrow package view
(identity and engine floors) and ignores plugin-only metadata even when its shape is invalid for the
package being built.

`AgentOverlayTools` holds permission sets of `OverlayToolName`s (`mars_schema::tool_names`, a
leaf grammar module): a tool name in `[agents.x.tools]` / `[skills.x.tools]` parses with the
profile grammar as the config loads, and a malformed one fails the load. It keeps the author's
spelling, so `save` does not rewrite it. This is the one place `config` reaches into `compiler`.
Plain `tools.allowed` / `tools.disallowed` lists add to profile rules; a
`{ replace = [...] }` table clears that action's lower rules, including `'*': allow`. Plain lists
preserve wildcard rules; existing asks and the `'*': deny` allowlist marker survive both forms.
Profile list-form `tools: [...]` is deprecated and only grants its named tools; package profiles
cannot replace deny sets. The optional fields preserve absent versus authored-empty lists. During
the compatibility release, `project_config_warnings` warns once for each non-empty plain tool
list from the raw project/local config; empty lists and replacement tables are silent.

## Layering (`layering.rs`)

- `merged_settings()` applies `mars.local.toml [settings]` over already-loaded project settings.
- Network host permission sets compose across user, project, and local settings; other settings
  lists retain replacement semantics.
- Model aliases are **replace-by-key**. Agent and skill overlays also replace the whole keyed
  entry, except their four network/tool permission sets compose across project and local entries.
- Composition means a project tool list survives a matching local block that omits the list. A
  local block that adds only a network set still replaces project scalars; repeat any project
  scalar the local entry must retain.
- Unknown project settings and agent-overlay keys remain forward-compatible. Network-looking
  unknowns are retained and warned because silently dropping a deny would fail open.
  `mars.local.toml [settings]` remains strict and rejects unknown keys.
- Removed `[settings.model_visibility]` is an error only for the consumer's
  project `mars.toml` and `mars.local.toml`. `load_manifest` parses only
  exported package sections, ignoring an upstream package's consumer settings.

## Dependency Entry Validation

- `url` XOR `path` (not both, not neither)
- Include filters (`agents`/`skills`) XOR `exclude` (not both)
- `only_skills` XOR `only_agents` (not both)
- Category flags cannot combine with include/exclude lists

## Link Normalization (`targets.rs`)

`normalize_link(raw)` is the **single** link-normalization function. `LinkKind` determines routing impact:

| Kind | Examples | Effect on routing |
|---|---|---|
| `KnownHarness` | `claude`, `.claude`, `codex`, etc. | Adds permission to the configured `HarnessScope` |
| `GenericTarget` | `agents`, `.agents`, `.foo` | Materialization only — adds no harness permission |
| `PathLike` | `path/to/dir`, `C:\foo` | Materialization only — adds no harness permission |

`HarnessScope::Only` preserves empty target scope; generic/path-only targets permit
no harness. Only absent targets and managed_root yield `Unrestricted`.

## Routing Settings (`routing_settings.rs`)

`resolve(settings)` converts raw `Settings` into `ResolvedRoutingSettings`. Both models commands and build policy must consume this typed result — they must not re-parse harness names independently.

Invalid harness names produce diagnostics (not hard errors) so commands continue with degraded config.

## Model Policies

`mars_schema::model_policy::ModelPolicyRule` — match by `model`, `alias`, or `model-glob` with optional overrides and `no-fallback` flag. The schema crate defines and validates it for agent overlays, settings, and profile frontmatter.

## Save Roundtrip Validation

`save()` validates the serialized config roundtrips identically before writing. Prevents data loss from serialization bugs.

## Patterns

**Load and merge:**
```rust
let config = load(project_root)?;
let local = load_local(project_root)?;
let (effective, diagnostics) = merge_with_root(config, local, project_root)?;
```

**Link inspection:**
```rust
let links = config.settings.effective_links();
let scope = links.harness_scope();     // permission, preserving empty/unset
let targets = links.managed_targets();  // materialization paths
```

## See Also

- `.context/CONTEXT.md` — link normalization contracts, routing settings boundary
- `src/routing/AGENTS.md` — consumes `linked_harnesses` from config
- `src/sync/AGENTS.md` — uses EffectiveConfig as input
