# Mars source

A failed sync's pending canonical journal is ownership evidence: retain it for recovery. Never infer permission to mutate a linked target from a canonical claim or a filesystem scan; see [sync](sync/AGENTS.md) and [target ownership](target_sync/.context/CONTEXT.md).

A root's manifest, local config, locks, store (`.mars`), local source (`.mars-src`) and target folders are paths from `layout::Layout` (`MarsContext::layout`); never join those names onto the root by hand, so a root laid out differently needs no branch at the sites. A root is a project (`Layout::project`) or the user level (`Layout::user`: manifest, lock and `.mars-src` in the Mars config folder, store in the data folder, no targets); the kind is set where the layout is built, and only `config::load`/`save` (absent manifest is empty, `targets` refused) and the CLI's root choice consult it. A package checkout's `mars.toml` is the package format, not a layout.

The user level and a host's content reach a project only as dependencies named with their layer's prefix (`_user-`, `_provided-`); `layer.rs` is the one table of what a layer is offered to, which layer wins a name, and that its records are machine-local. See [sync](sync/AGENTS.md).

`mars-schema` owns package vocabulary, parsing and validation; installed definitions go through its compatibility readers. Keep target-specific lowering behind the target and compiler seams, and route every model command through the shared evaluator. Library code reports diagnostics through the diagnostic collector, not stderr.

`hook_run` is the command a translated `agy` hook runs (`mars hook run`, hidden); `agy` blocks on any exit but 0 and on a reply with another event's field, so past its input it always exits 0 with a reply valid for the event (`PreToolUse` no opinion is empty output, never `{}`). Its rules are `core.ts`'s: see [hook translation](../crates/mars-schema/AGENTS.md#hook-translation).
