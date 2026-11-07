# Sync

Sync resolves the complete desired package graph before compiling or changing installed content. The compiler and target sync use that graph to produce canonical and harness-specific surfaces.

Recovery commands may persist the user's config intent and stop before compilation when a hook uses an unreadable schema. The compiler must receive a complete desired graph: treating unreadable content as an exception would make pruning and ownership unsafe. Repair also leaves a corrupt lock untouched until a complete sync can replace it.

New canonical outputs are journaled before they are written. Recovery checks the journal against the prior lock and the filesystem, and fails closed on changed content or unsafe paths. Finalization publishes ownership before removing the journal. Native and config outputs are not covered by this journal. Explicit force adoption of existing outputs is not journaled; after a failed apply it requires `--force` again. See [recovery.rs](recovery.rs).

Dry runs must not create project state. They use scratch staging and acquire an existing sync lock without creating one, so the lock and recovery journal are read as a consistent snapshot. Every new write phase must honor dry-run mode.

A package may replace a provided canonical item: provider ownership is recognized alongside lock ownership, so host-specific provided items do not change the resulting package lock. Skill reference diagnostics distinguish declared providers from undeclared or unavailable skills. See [ownership.rs](ownership.rs) and [validate.rs](validate.rs).

Sync and plugin builds share target selection and renaming. Sync removes unmanaged collisions before rewriting references, so references reflect the items that survive. Hook and MCP entries must be lowered from the selected target state, not rediscovered; their config must not reference a directory that target sync failed to install. Invalid dependency hooks can retain the directories and outputs recorded in the lock rather than being pruned as if removed.

See [target sync ownership](../target_sync/.context/CONTEXT.md) for linked-target writes and [resolution](../resolve/AGENTS.md) for dependency selection.
