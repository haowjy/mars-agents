# State machines

Every state machine in Mars: anything with named states and rules for moving between them, whether
an enum, a typed phase, a marker file or a lifecycle spread over two fields. Each
`state-machines.md` sits in the `.context/` beside the code that owns the transitions, draws the
machine as the code has it and says what a wrong edit breaks. Change the code and the diagram in the
same commit. Product words are in the [invariants](../invariants.md) and each area's `AGENTS.md`.

## Patterns

Six shapes recur. Each page draws the shape once, gives its rules and lists the machines that have
it:

- [An atomic replace](atomic-replace.md): a file or tree swapped into place by rename, and what a
  kill between the renames leaves.
- [A marker a crash leaves](crash-marker.md): a journal or marker written first so the next run can
  finish, undo or refuse, and the machines that have none.
- [A lease or lock](lease-or-lock.md): work one process may do at a time, as an OS lock the kernel
  releases or a claim file the clock expires.
- [A lazy cache entry](lazy-cache-entry.md): fetched the first time it is needed, kept, and never
  replaced by a failed refresh.
- [An ordered fallback walk](fallback-walk.md): candidates tried in a documented order, with a rule
  for rejection and for running out.
- [An ownership record](ownership-record.md): the record that proves Mars wrote a file or entry,
  before it overwrites or deletes it.

## Schema crate

| Machine | State of | Page |
|---|---|---|
| Root classification | One package root: `Schema1`, `Foreign`, `Marketplace`, `Invalid`, `Empty` | [mars-schema](../../crates/mars-schema/.context/state-machines.md) |
| Prepared definition | One file, from selected through translation to a strict reading | [mars-schema](../../crates/mars-schema/.context/state-machines.md) |
| Where a rejected key goes | One frontmatter key the strict reader refuses | [mars-schema](../../crates/mars-schema/.context/state-machines.md) |
| Item selection | One Agent, skill, document or hook across the dependency graph | [mars-schema](../../crates/mars-schema/.context/state-machines.md) |
| Skill ref resolution | One `skills:` ref of one Agent | [mars-schema](../../crates/mars-schema/.context/state-machines.md) |
| Version choice | One package's ref: a pin, the lock or the newest tag | [mars-schema](../../crates/mars-schema/.context/state-machines.md) |
| Generated-tree marker | One directory: input, or Mars's own output | [mars-schema](../../crates/mars-schema/.context/state-machines.md) |
| Hook on one target | One event of one hook: rendered, skipped, overridden, off | [mars-schema](../../crates/mars-schema/.context/state-machines.md) |
| Tool rule on one harness | One `tools:` rule: a native name, a gap, absent or beaten | [mars-schema](../../crates/mars-schema/.context/state-machines.md) |
| Dependency filter | One dependency entry's six filter fields before they become a mode | [mars-schema](../../crates/mars-schema/.context/state-machines.md) |

## Sync, lock and staging

| Machine | State of | Page |
|---|---|---|
| Sync pipeline phases | One `mars sync`, from config load to the written lock | [sync](../../src/sync/.context/state-machines.md) |
| Canonical write journal | A canonical write that may have landed but is not in `mars.lock` | [sync](../../src/sync/.context/state-machines.md) |
| Per-path classification | One destination path through diff, plan and apply | [sync](../../src/sync/.context/state-machines.md) |
| Invalid dependency hook | One dependency hook whose `hook.toml` stopped validating | [sync](../../src/sync/.context/state-machines.md) |
| MCP scan failed | Whether the MCP servers of a dependency are known this run | [sync](../../src/sync/.context/state-machines.md) |
| Lock output record | One installed path in one target: `Installed` or `PendingDeletion` | [lock](../../src/lock/.context/state-machines.md) |
| `mars.lock` schema lifecycle | Which shapes of the lock file Mars accepts | [lock](../../src/lock/.context/state-machines.md) |
| Hook-entry removal | A target's recorded hook entries: confirmed removed or retained | [surface_ownership](../../src/surface_ownership/.context/state-machines.md) |
| `.mars/sync.lock` | Whether a Mars process is changing this project | [fs](../../src/fs/.context/state-machines.md) |
| Latched read error | A failed read inside a call with no error channel | [package_prepare](../../src/package_prepare/.context/state-machines.md) |
| Staging generation | The projected package trees: scratch, pending, published | [platform](../../src/platform/.context/state-machines.md) |
| Provided lock record | What one host installed into `.mars/skills`, with the `pending` digest | [provided](../../src/provided/.context/state-machines.md) |

## Compile and build

| Machine | State of | Page |
|---|---|---|
| Native agent files | An agent file Mars writes into a harness's directory, claimed by the lock | [compiler](../../src/compiler/.context/state-machines.md) |
| Native model decision | Which model string a native agent file gets: set, cleared or skipped | [compiler](../../src/compiler/.context/state-machines.md) |
| Hook and MCP config entries | One harness's shared config file with Mars entries merged in | [compiler](../../src/compiler/.context/state-machines.md) |
| Plugin output tree | The directory `mars build plugin` writes | [build](../../src/build/.context/state-machines.md) |
| Generated root files | The root files a plugin build writes beside the output | [build](../../src/build/.context/state-machines.md) |
| Plugin publication order | One plugin build across the tree swap, the root files and the marker | [build](../../src/build/.context/state-machines.md) |
| Skill resolution in a launch | One skill the agent profile asks for | [build](../../src/build/.context/state-machines.md) |
| Launch policy | One launch request's model and harness | [build/policy](../../src/build/policy/.context/state-machines.md) |
| Slug tier effort | Whether a requested effort level reached the model slug of a harness that folds tiers into its slugs (Cursor, Antigravity) | [build/policy](../../src/build/policy/.context/state-machines.md) |

## CLI and targets

| Machine | State of | Page |
|---|---|---|
| Project resolution | Which project root a command runs against | [cli](../../src/cli/.context/state-machines.md) |
| Target claim | One target directory becoming managed (`mars link`) | [cli](../../src/cli/.context/state-machines.md) |
| Target teardown | One target directory ceasing to be managed (`mars unlink`) | [cli](../../src/cli/.context/state-machines.md) |
| Item adoption | One unmanaged file moving into `.mars-src` (`mars adopt`) | [cli](../../src/cli/.context/state-machines.md) |
| Item conflict | One canonical item: ok, modified, conflicted or missing | [cli](../../src/cli/.context/state-machines.md) |
| Release | One `mars version` run, from bump to tag and push | [cli](../../src/cli/.context/state-machines.md) |
| Project scaffold | One directory becoming a Mars project (`mars init`) | [cli](../../src/cli/.context/state-machines.md) |
| Managed target output | What is at one destination in a managed target, and whether Mars may write it | [target_sync](../../src/target_sync/.context/state-machines.md) |
| Native skill projection | A skill generated into a target from the canonical skill and its variants | [target_sync](../../src/target_sync/.context/state-machines.md) |
| MCP server entry | One server entry in a target's MCP file | [target](../../src/target/.context/state-machines.md) |
| Hook entry | One entry in a hook config file | [target](../../src/target/.context/state-machines.md) |
| Codex hook trust | Whether Codex will run a hook handler | [target](../../src/target/.context/state-machines.md) |

## Resolution, models and harnesses

| Machine | State of | Page |
|---|---|---|
| Package resolution | One source within one resolve pass, and the pass that restarts | [resolve](../../src/resolve/.context/state-machines.md) |
| Engine-requirement fallback | The candidate versions a run has refused for `requires-*` | [resolve](../../src/resolve/.context/state-machines.md) |
| Source version and commit | Which rung of the lock-or-tag ladder answered | [resolve](../../src/resolve/.context/state-machines.md) |
| Git cache entry | A repository in the global cache, and one commit exported from it | [source](../../src/source/.context/state-machines.md) |
| Archive cache entry | A GitHub tarball in the global cache | [source](../../src/source/.context/state-machines.md) |
| Model catalog snapshot and refresh claim | The catalog in `.mars/`, its freshness and who may refresh it | [models](../../src/models/.context/state-machines.md) |
| Probe cache | One harness's cached model listing | [models/probes](../../src/models/probes/.context/state-machines.md) |
| Harness auth evidence | Whether one harness route is usable right now | [harness](../../src/harness/.context/state-machines.md) |
| Probe loading and known providers | Which probes one command has loaded | [harness](../../src/harness/.context/state-machines.md) |
| Harness routing scan | Which harness candidate the evaluator accepted | [routing](../../src/routing/.context/state-machines.md) |
| Curation fold | Whether one possible model row is shown | [curation](../../src/curation/.context/state-machines.md) |
| Target provenance | Which config file set `targets` | [config](../../src/config/.context/state-machines.md) |
