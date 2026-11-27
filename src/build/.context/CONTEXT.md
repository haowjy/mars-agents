# Build contracts

## Plugin publication

A plugin build uses `PublishLens` and the shared sync selection and rename pipeline. It must not depend on local or user config, the project lock, installed targets, model caches or installed harnesses: those are consumer state and would make the published tree machine-dependent. An explicit `--out` build does not write package-root files.

Publication stages under a lock and refuses to replace a nonempty tree without a valid Mars sentinel. It swaps the tree before updating generated root files; the sentinel records the previous root hashes for interrupted-publication recovery. A retired path is removable only while its bytes match the recorded ownership evidence. Recovery preserves ambiguous rollback evidence and refuses a symlink or non-directory at its reserved staging path.

The input fingerprint is computed after recovery under the output lock, before staging, so crash leftovers and generated output cannot make a clean package appear stale. It covers authored inventory and relevant manifest tables, not resolved dependencies; the sentinel records dependency identities separately. Configured builds reject paths still ignored by Git, while explicit `--out` may target ignored CI output.

OpenCode loads no agent files from a plugin, so its generated plugin registers agents through `ctx.agent`. OpenCode's runtime API also expects different permission keys from its file loader. Hook loading is isolated per hook so one failed import or setup does not prevent later hooks. Claude plugin MCP environment pass-throughs use `env_vars`; renamed references retain Claude syntax and warn because Codex passes those values literally.

## Launch bundles

Ad-hoc mode works without a project or agent profile, allowing callers to route without a sync; an omitted model lets the selected harness choose its default. Agent mode reads the synced profile and skills, or the profile from `--agent-file`: a caller that found the profile itself (Meridian ships one in its wheel) hands Mars that file so the two lookups cannot disagree, while overlays stay keyed by the `--agent` name. The project-discovery bypass is limited to explicit supported routes.

`routing.harness_model` is the selected runtime ID, including effort when encoded into the slug. `routing.catalog_row` is the models.dev row of the model the harness runs, joined once per harness for effort (`models::reasoning::catalog_row`); consumers read prices from it and never rebuild the join. `candidate_slugs` and route traces are diagnostic, not launch instructions. A selected trace records route selection, not successful bundle construction or process launch. A literal model bypasses alias lookup and backups but still goes through permission, support and authentication assessment.

An explicitly empty literal model selects the harness default rather than falling through to profile or project defaults. This lets continuation callers preserve a recorded choice even if aliases change. Route facts such as passthrough and unknown confidence are not warnings; warnings identify actionable degradation. Tool permission gaps belong in rule status and notes because launchers may display every warning on every launch.
