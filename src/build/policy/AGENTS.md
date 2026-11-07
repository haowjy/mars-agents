# Launch policy

Resolve model, harness and execution fields independently. A CLI model pin disables model backups; a CLI harness pin restricts every attempt to that harness. Profile, overlay and alias harnesses are ordered preferences, not pins. Never clear a requested model to make a harness work.

Target scope and caller exclusions both restrict candidates; an exclusion cannot widen target permission. Reject excluded candidates before auth probes. Exhaustion is an error, not a reason to substitute an unassessed harness.

Try the primary model, then concrete profile policy entries in declaration order. Defer an unverified route until candidates are exhausted, keeping each model's settings and provenance together. `no-fallback` excludes its entry, not the entire chain. Share native auth observations across attempts.

See [policy contracts](.context/CONTEXT.md) and [routing](../../routing/AGENTS.md).
