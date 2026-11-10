# Sync

Sync resolves the complete desired package graph before compiling or changing installed content. Recovery commands may persist the user's config intent and stop before compilation when a hook uses an unreadable schema: the compiler must never receive a partial graph, or pruning and ownership become unsafe. Repair leaves a corrupt lock untouched until a complete sync can replace it.

New canonical outputs are journaled before they are written. Recovery checks the journal against the prior lock and the filesystem and fails closed on changed content or unsafe paths. Finalization publishes ownership before removing the journal. Native and config outputs aren't journaled, nor is `--force` adoption of existing outputs: after a failed apply it needs `--force` again. See [recovery.rs](recovery.rs).

Dry runs must not create project state: they use scratch staging and acquire an existing sync lock without creating one. Every new write phase must honor dry-run mode.

A package may replace a provided canonical item: provider ownership is recognized alongside lock ownership, so host-specific provided items don't change the package lock. See [ownership.rs](ownership.rs) and [validate.rs](validate.rs).

Sync and plugin builds share target selection and renaming. Sync removes unmanaged collisions before rewriting references. Hook and MCP entries are lowered from the selected target state, never rediscovered, and their config must not reference a directory target sync failed to install. Invalid dependency hooks can retain the directories and outputs recorded in the lock rather than being pruned as removed.

See [target sync ownership](../target_sync/.context/CONTEXT.md) and [resolution](../resolve/AGENTS.md).
