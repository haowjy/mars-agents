# Policy resolution

## Harness preferences and pins

Harness field precedence is CLI → overlay → overlay model-policy → profile →
profile model-policy → settings model-policy → alias. Resolve this independently
from model precedence; a CLI model does not suppress the profile harness preference.

Only CLI harness is fixed. Its blocked assessment is an exhausted model attempt,
so implicit model backups can still run on that same harness. A CLI model pin
prevents those backups. Target/caller exclusion of a CLI harness fails before probes.

For implicit selection, pass the highest-precedence harness and its source to the
shared routing evaluator. It leads configured harness order, default_harness and
remaining permitted registry candidates. Permission filters and stable dedup apply
to the entire list. The preferred route is assessed once, like any other route;
installation, provider mismatch or rejected auth cannot turn it into a default-model
launch. Keep selected field provenance and the matched policy rule when the
preference wins; otherwise use the selected candidate's source.

## Model attempts

The outer loop evaluates the primary, then the shared profile backup iterator.
Each attempt independently resolves model identity, provider constraints and
settings. An eligible route wins immediately; retain the first whole unverified
attempt only when no attempt has an eligible route. Do not reconstruct that attempt
from the last iteration's state. Native auth observations are shared across attempts.

## Related docs

- [Policy overview](../AGENTS.md)
- [Bundle contracts](../../.context/CONTEXT.md)
- [Routing evaluator](../../../routing/.context/CONTEXT.md)
