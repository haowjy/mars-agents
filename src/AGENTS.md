# Mars source

Mars resolves packages, writes a derived `.mars/` canonical store, and projects managed content into target directories. Authored packages and config remain the source of intent; a failed sync's pending canonical journal is ownership evidence and must be retained for recovery.

`mars-schema` owns package vocabulary, parsing and validation. Authored content goes through preparation; installed definitions use the schema's compatibility readers. Keep target-specific lowering behind the target and compiler seams, and route every model command through the shared evaluator.

Never infer permission to mutate a linked target from a canonical claim or from a filesystem scan. See [sync](sync/AGENTS.md) and [target ownership](target_sync/.context/CONTEXT.md). Diagnostics from library code go through the diagnostic collector, not stderr.
