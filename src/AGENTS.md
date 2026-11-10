# Mars source

A failed sync's pending canonical journal is ownership evidence: retain it for recovery. Never infer permission to mutate a linked target from a canonical claim or a filesystem scan; see [sync](sync/AGENTS.md) and [target ownership](target_sync/.context/CONTEXT.md).

`mars-schema` owns package vocabulary, parsing and validation; installed definitions go through its compatibility readers. Keep target-specific lowering behind the target and compiler seams, and route every model command through the shared evaluator. Library code reports diagnostics through the diagnostic collector, not stderr.
