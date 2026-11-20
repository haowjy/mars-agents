# Schema v1

Schema v1 is the format of a Mars package's definition files: Agents (`agents/<name>.md`), skills (`skills/<name>/SKILL.md`) and the `[package]` table of `mars.toml`. One reader in the [`mars-schema`](../../crates/mars-schema/AGENTS.md) crate parses them for the Mars binary, for the [`@meridian-flow/mars-schema`](../../npm/@meridian-flow/mars-schema/README.md) npm package that editors use, and for Meridian Flow. A file the reader accepts means the same thing to every consumer.

A package declares the format it uses. Mars 0.34.0 dropped the format before it; see [Retired keys](retired-keys.md).

## What schema 1 promises

```toml
[package]
name = "my-package"
version = "1.0.0"
schema = 1
```

- **Not frozen before Mars 1.0.** While Mars is 0.x, a minor release may still change or remove a schema 1 key; the CHANGELOG lists each change. From Mars 1.0, schema 1 is fixed: the same bytes mean the same thing, a minor release only adds optional keys, and a change in meaning needs `schema = 2` with an upgrade path from 1.
- **Strict.** A package that declares `schema = 1` is read as written. A value it does not allow and a malformed file are errors that name the file; there is no silent fallback. A key it no longer reads is a [`field.retired`](retired-keys.md) warning, and any other unknown key is preserved and reported as `field.unknown`, an `Info`.
- **Published as data.** The key tables below, the tool vocabulary, and a corpus of input files with their expected readings are fixtures that Mars's tests and Meridian Flow's tests run. See [The contract as data](#the-contract-as-data).

## How Mars reads a package

Mars classifies a package root before it reads any definition:

| `mars.toml` | Mars does |
|---|---|
| `[package] schema = 1` | Reads every definition strictly. Errors fail `mars sync`. |
| `[package]` without `schema` | Refuses the package with `manifest.schema-missing`, which says to add `schema = 1` ([Retired keys](retired-keys.md#package-without-schema)). Mars before 0.34.0 read such a package as the format before schema 1. |
| `schema` higher than 1, or not an integer | Refuses the package. A Mars that does not know the schema cannot read it. |
| no `mars.toml` | The tree is a foreign layout (a Claude plugin, a marketplace, a tree of `SKILL.md` directories, a Codex or OpenCode repository, or a Cursor one). Mars translates it into a schema v1 package; a tree of plain `agents/` and `skills/` folders is translated by Claude's rules (or OpenCode's, when an Agent has a `permission` map or a `tools` map of booleans). A key or value the translation cannot make schema v1 is moved under `harness-overrides.<dialect>` (or dropped when no harness reads it, or when Mars lowers it itself) with a warning, never an error: only an authored schema v1 file is read strictly. See [Dialect](mars-toml.md#dialect). |

The same classification runs for dependencies, the local package, `.mars-src/`, `mars check`, `mars validate` and plugin builds. A consumer's own items in `.mars-src/`, and a manifest-less tree declared `dialect = "mars-native"`, are read as schema 1. Reading finishes for the whole dependency graph before any content is written; see [the sync pipeline](../internals/sync-pipeline.md#preparation-and-the-gate).

An installed `.mars/` tree is read by the same readers, with one exception: an Agent with no frontmatter is `frontmatter.missing` only in an authored file.

### File rules

- **Frontmatter** is the block between a first line that is exactly `---` (after an optional byte-order mark) and the next line that is exactly `---`. Lines end in LF or CRLF. A file whose first line is not `---` has no frontmatter and is all body: that is an error for an Agent (`frontmatter.missing`), because every field would take its default, which is the widest permission, and a stray line above `---` would turn a narrow Agent into an unrestricted one. A skill with no frontmatter is a body-only skill (below). The rule is for schema 1 files, a project's own `.mars-src/` included; a foreign package's body-only Agent still installs. A fence that opens and never closes is an error (`frontmatter.unclosed`).
- **The body** is every byte after the closing fence's line.
- **YAML** must be a mapping, or empty. Schema 1 refuses duplicate keys, anchors and aliases (`yaml.anchor`), and a `...` document-end marker (`yaml.document-end`). A top level that is a list or a scalar is `frontmatter.not-mapping`.
- **Booleans** are `true` and `false`. `yes`, `no`, `on` and `off` are strings, so they are errors in a boolean field.
- **Case.** Enumerated values are exact, except `effort` and `harness`, which are case-insensitive.
- **Unknown keys** are kept in the authored file and reach no native file, in Agents and skills alike. Each produces a `field.unknown` `Info` (printed by `mars validate`, `mars check` and `--verbose`); [Unknown keys](#unknown-keys) says where harness-specific keys belong.

## Agent keys

| Key | Value | Notes |
|---|---|---|
| `name` | non-empty string | Optional. The file name is the Agent's identity when omitted. |
| `description` | string | Shown in listings and written to native files. |
| `harness` | `claude`, `codex`, `opencode`, `pi`, `agy` | Optional routing hint. An unknown name is an error. `agy` is accepted but never routed to: Meridian does not launch it. |
| `model` | string | An alias from `[models]` or a concrete model ID. |
| `mode` | `primary`, `subagent` | |
| `model-invocable` | boolean, default `true` | Whether another model can invoke this Agent. |
| `user-invocable` | boolean, default `true` | Whether the user can invoke it directly. |
| `approval` | `default`, `auto`, `confirm`, `never` | Tool-call approval policy. `default` is an explicit request for the harness default; leaving the key out means unset. |
| `sandbox` | `default`, `read-only`, `workspace-write`, `danger-full-access` | Filesystem access. |
| `network` | `allow`, `deny`, `allowlist` | Shell network access; see [Network](#network). |
| `network-allow` | list of hosts, or `{replace: [...]}` | Hosts an `allowlist` permits. |
| `network-deny` | list of hosts | Hosts that stay denied. A package's Agent cannot replace this set. |
| `effort` | `auto`, `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` | See [Effort](#effort). |
| `skills` | list, or `{load: [...], available: [...]}` | A bare list means `load`. |
| `subagents` | list of names | The Agents this one may spawn. |
| `tools` | map of capability to `allow`, `deny` or `ask` | See [Tool capabilities](#tool-capabilities). |
| `harness-overrides` | map of harness or consumer name to a map of that consumer's keys | See [harness-overrides](#harness-overrides-and-the-consumer-namespaces). |
| `model-policies` | list of rules | See [Model policies](#model-policies). |

Field-by-field behaviour is in [Agent profiles](agent-profiles.md).

### Retired Agent keys

A key Mars no longer reads is a `field.retired` warning with a link to [Retired keys](retired-keys.md), which says what each became. The list is one table, `retired::RETIRED` in the `mars-schema` crate; delete a row and its key is an ordinary `field.unknown`. `tools` as a list is an error (`tools.list-form`): write the action map. A model policy's `override` refuses what it does not define, so `autocompact`, `autocompact_pct` and `harness` there are errors that list the keys it takes.

The consumer tables of `mars.toml`, `mars.local.toml` and the user `config.toml` are not part of the schema: a key they do not read is an `Info`. See [Keys Mars does not read](mars-toml.md#keys-mars-does-not-read).

## Skill keys

| Key | Value | Notes |
|---|---|---|
| `name` | non-empty string | Required when the file has frontmatter. |
| `description` | non-empty string | Required when the file has frontmatter. |
| `license` | string | |
| `compatibility` | string | |
| `metadata` | map | `metadata.type` is the skill's type (`principle`, `guardrail`, `reference`, ...), a string. The rest is yours. |
| `model-invocable` | boolean, default `true` | |
| `user-invocable` | boolean, default `true` | |

A skill with no frontmatter is a body-only skill: it reads with a `skill.no-frontmatter` warning. Field behaviour is in [Skill compilation](skill-compilation.md).

### Retired skill keys

See [Retired keys](retired-keys.md): `type`, `when_to_use`, the tool-policy keys, the underscore invocability spellings, `disable-model-invocation`, `invocation` and `allow_implicit_invocation` are `field.retired` warnings.
## Manifest keys

The reader covers the `[package]` table of `mars.toml`. The other tables (`[dependencies]`, `[settings]`, `[models]`, `[plugin]`, the `[agents.*]` and `[skills.*]` overlays) are Mars's consumer and publishing configuration; see [mars.toml](mars-toml.md).

| Key | Value | Notes |
|---|---|---|
| `name` | non-empty string | Required. |
| `version` | string | A missing version is a warning. |
| `schema` | integer `1` | Required for schema 1. |
| `description`, `homepage`, `license` | string | |
| `authors` | list of strings or `{name, email}` tables | |
| `requires-mars`, `requires-meridian` | version requirement | Tool-version requirements; see [mars.toml](mars-toml.md#package-optional). A dependency's `requires-mars` that the running Mars does not meet fails `mars sync` when no release of it is compatible. A project's own floor is `[settings].min_mars_version`, which `mars sync` and `mars build launch-bundle` enforce. |
| `uses-providers` | list of provider names | The hosts the package expects to supply skills at sync time, such as `["meridian"]`. Each entry matches `[a-z0-9][a-z0-9-]*` (a `field.value` error otherwise). Mars never fetches or requires a provider; the list only lets the package's agents name that provider's skills. See [mars.toml](mars-toml.md#provided-skills-and-uses-providers). |
| `tools` | table of tool key to `allow`, `deny` or `ask` | Rules added to the `tools:` map of every agent the package ships, unless the profile already has the key. Keys follow a profile's `tools:` keys; `*` is refused, and so are two spellings of one key. Needs `schema = 1`. `@meridian-flow/mars-schema`'s `prepare` adds them to each Agent's `tools` reading the same way, with `origin: "package"`. See [mars.toml](mars-toml.md#package-tool-rules-packagetools). |

Removed: `package.visibility`, with nothing to replace it.

## Effort

`effort` is the reasoning effort for the Agent's model: `none`, `minimal`, `low`, `medium`, `high`, `xhigh` or `max`, in rising order (every effort value models.dev lists); values are case-insensitive. `auto`, or YAML `null`, means "send no effort": the harness's own default, then the model's. Claude Code's `effortLevel` setting, for example, comes before the model's default. In `mars.toml`, which has no null, write `auto`. A key left out is different: it falls through to the next layer, while `auto` overrides the lower layers. `none` turns reasoning off.

**The level a launch sends is one both the model and the harness take.** Mars reads what the model takes from its models.dev row, and what the harness takes from one table in the code (`harness_levels`, which records the version each set was checked against):

| Harness | Levels |
|---|---|
| Claude Code | `low`, `medium`, `high`, `xhigh`, `max`; no off |
| Codex | every level |
| Pi | every level (`none` is `--thinking off`) |
| OpenCode | any: it reads models.dev's `reasoning_options` itself |
| Agy | any: Meridian does not launch it, so Mars sets no effort for it |
| Meridian Flow (WASM only) | `none`, `low`, `medium`, `high`, `max` |

A requested level in both sets is kept. Otherwise it moves to the nearest level on the ladder that both take, below or above, and a tie goes up, and Mars warns: "`max` → `high`: `o3` takes low, medium, high". When both sets limit the request, the warning names both. A move never lands on `none` unless `none` was asked, because off is not less thinking. What the model takes comes from its row: the effort values it lists, plus `none` when it has a toggle. A model that reasons but lists no values (toggle only, budget only, or no control) takes every level, less `none` when it cannot be switched off, so `none` moves up to `minimal` or `low` there. A model with no row is unknown and takes any level. A model that does not reason gets none, with a warning when a level other than `none` was asked. The row is the one for the provider the harness calls: `openai/o3` takes `low`, `medium` and `high`, while `openrouter/openai/o3` is toggle only. A provider-qualified model (`openai/o3` pinned by an alias) and an Antigravity slug that carries its tier (`gemini-3.8-flash-low`) find the row of the model behind them. On a machine whose catalog still comes from a build before 0.35, no row has reasoning data: `mars sync` and `mars build launch-bundle` say so, and send the effort as written until the first refresh succeeds.

`mars models list` shows what each row says, not what a launch could send: the levels the model lists that the harness takes, `on/off` for a toggle, `budget` for a budget only, nothing for a model that reasons with no control, and `—` for a model that does not reason (`reasoning.levels` is `null` for all but the first).

The warning is `launch.warning` in a launch bundle. At sync it is `agent.effort-moved`: one line for all Agents, and with `--verbose` one per Agent and harness.

What each harness's native file or launch carries for the resolved level:

| Harness | Resolved effort |
|---|---|
| Claude Code | `effort: <level>` in the native file |
| Codex | `model_reasoning_effort = "<level>"` in the native file |
| OpenCode, Pi | no effort field is written; the bundle carries the level for the launcher |
| Agy | the native agent file carries no effort |

The launcher reads the bundle's `effort` and passes it through; it never interprets it. `mars build launch-bundle --effort none` overrides the Agent from the command line, and `--config-effort` carries the caller's own configuration (Meridian's `primary.effort`), which sits above the model alias's `default_effort` and below the Agent and model policies.

**Older Mars.** A Mars before 0.35.0 refuses `minimal`, `auto` and `null` and fails the whole sync, so a file that uses one needs `requires-mars >= 0.35.0`: `mars check` errors until it is set and `mars version` writes it. The same goes for a model alias's `default_effort` of `none` or `minimal`, and for any spelling of it that Mars before 0.35.0 did not read: it matched `low`, `medium`, `high`, `xhigh`, `max` and `auto` in lowercase only, so `HIGH` and `Auto` need the floor too. An Agent's `effort` was already case-insensitive. The error quotes the value as written.

`adaptive` and `disabled` are not values; the values they stood for are `auto` and `none`.

## Network

`network` is the Agent's shell network policy: `allow`, `deny` or `allowlist`. An absent `network` means unset, and Mars does not pick a default: the launch bundle carries `"mode": null` and Meridian follows the sandbox. `network-allow` and `network-deny` are host sets. A host is an exact name, an IP address, `*.example.com` (subdomains only) or `**.example.com` (the apex and its subdomains); the reader lowercases it, drops a trailing dot and refuses a URL, a port, a path, a bare `*` and a non-punycode name.

Host sets are permission sets, so a plain list adds and `{replace: [...]}` clears the lower entries. An Agent in a package may replace `network-allow` but not `network-deny`: a package cannot lift a deny the consumer set.

The mode resolves through the [precedence chain for `effort`, `sandbox` and `approval`](#model-policies), with the consumer's `[settings] default-network` just above the end of it. Host sets do not take a winner; they merge in layer order (settings, Agent, the consumer's overlay, the command line), come out sorted, and the project's deny entries are never replaced by a higher layer. Two rules then close the policy:

- `allowlist` with no allowed host resolves to `deny` and warns.
- `deny` empties both host sets, and `allow` empties the allow set.

The launch bundle carries the result as `execution_policy.network` (`{mode, allow, deny}`), and `provenance.network_source` names the layer that set the mode: `cli`, `overlay`, `overlay-model-policy`, `profile`, `profile-model-policy`, `settings-model-policy`, `config` or `unset`. Mars does not enforce the policy; Meridian does. Native agent files and plugin trees resolve the same chain without the command-line layer and without `[settings]`, and report `network` as meridian-only, or as dropped for Codex, whose native file keeps Codex's own network default. See [Agent profiles](agent-profiles.md#network).

## Model policies

`model-policies` lists per-model settings for one Agent. Each rule is:

```yaml
model-policies:
  - match: {alias: opus}            # exactly one of alias, model, model-glob
    override: {effort: medium}      # optional: effort, sandbox, approval, network
    when: Architecture review across module boundaries   # optional, one line
    no-fallback: true               # optional, default false
```

- **`match`** holds exactly one of `alias` (the alias the model was chosen through), `model` (the concrete model ID) or `model-glob` (a glob over the concrete model ID).
- **`override`** may set `effort`, `sandbox`, `approval` and `network`. Any other key, such as `harness`, is an error, and so are the host sets `network-allow` and `network-deny`: a rule picks the mode, and the hosts come from the Agent, the consumer and the command line.
- **`when`** is one non-empty line saying when this model is a good choice for this Agent. It is guidance for the caller, not a condition Mars evaluates. `mars models prompting @agent` prints each rule's `when`. `mars check` warns when a rule's `alias` is not defined.
- **`no-fallback`** keeps a rule from being a backup, at launch and in a plugin build. See [Agent profiles](agent-profiles.md#model-policies).

**First match wins; there is no merging.** Mars looks for the first rule that matches the chosen model: the consumer's `[agents.<name>]` rules first, then the Agent's own, then `[[settings.model-policies]]`. Only that rule's `override` applies. A field the rule does not set falls through to the next layer below.

**Precedence for `effort`, `sandbox`, `approval` and `network`**, highest first:

1. the command line (`--effort`, `--sandbox`, `--approval`, `--network`)
2. the consumer's `[agents.<name>]` overlay field
3. the matched rule's `override`, when the rule comes from the overlay
4. the matched rule's `override`, when the rule comes from the Agent
5. the Agent's top-level field
6. the matched rule's `override`, when the rule comes from `[[settings.model-policies]]`
7. the consumer's `[settings] default-network` (`network` only), then the user config's
8. the model alias's `default_effort` (`effort` only)
9. unset: the harness decides

For `effort`, layer 7 is `--config-effort`. Whatever layer wins, the model and harness then resolve it as above.

So the Agent's own policy wins over its top-level field: top-level `effort` is the default for any model without a rule, and a rule is the exception for one model. Native agent files and plugin builds resolve the same chain, without the command-line layer, so a native file and a launch bundle agree.

## harness-overrides and the consumer namespaces

`harness-overrides.<name>` holds configuration that one consumer owns, on an Agent or a skill. Mars checks only that it is a map of serializable values and keeps it in the installed file. It does two things with a harness block:

- **Native file.** The block for a harness is written into that harness's own Agent or skill file (frontmatter for Markdown, top-level keys for Codex TOML), and into that harness's plugin tree. No other harness sees it. A key Mars lowers itself (`model`, `tools`, `effort`, `description` and the rest) keeps Mars's value; the override is dropped. On an Agent, `mars sync` reports a dropped key that carries permissions (`sandbox_mode`, `permission`, `disallowed-tools`, …), and `mars sync --verbose` names every dropped key.
- **Launch bundle.** The block for the resolved harness reaches the launcher as `execution_policy.native_config`.

It never replaces a top-level field of the same name.

An Agent or skill `hooks` key is Claude's own, and lives here as `harness-overrides.claude.hooks`. It is not a package's `hooks/<name>/hook.toml`; see [Declaring Hooks in a Package](mcp-and-hooks.md#declaring-hooks-in-a-package).

```yaml
harness-overrides:
  codex:
    model_reasoning_summary: concise                  # a Codex config key, carried as written
  meridian-flow:
    permission: read                                  # Meridian Flow's own key
  meridian-cli:
    resident-rearm-budget: 3                          # the Meridian CLI's own keys
    capabilities: {spawn: true}
```

- A name that is one of the five harnesses is a harness block.
- **`meridian-flow` and `meridian-cli` are reserved** for the two Meridian hosts, Meridian Flow and the Meridian CLI. Neither is a harness: routing never sees them, Mars never interprets their keys, and neither block is written to any native file or launch bundle. They are the home of the consumer's own keys: `meridian-flow.permission` (formerly a top-level `permission`), and `meridian-cli.resident-rearm-budget` and `meridian-cli.capabilities` (formerly top-level `resident-rearm-budget` and `meridian-capabilities`). The short names `flow` and `meridian` are not read: the reader reports each as a `field.retired` warning ([Retired keys](retired-keys.md#host-namespaces)).
- Any other name is a warning (`harness.unknown`) and the block is kept.

### Unknown keys

An unknown key is an `Info` (`field.unknown`) and stays in the authored file, but it reaches **no** native file, in Agents and skills alike. This includes a key of a `model-policies` rule; a key of its `override` is an error (the table takes `effort`, `sandbox`, `approval` and `network`). Harness-specific keys belong under `harness-overrides.<harness>`, where they go to that harness alone.

## Tool capabilities

`tools` states policy over **capabilities**, not harness tool names. A capability is a kind of tool (`bash`, `edit`, `web`); Mars maps each to the real tool names of every harness. A rule can carry a scope in parentheses (`bash(git push *)`).

```yaml
tools:
  bash: allow
  "bash(git push *)": ask
  edit: deny
  mcp(context7): allow
  "*": deny          # with the allows above: an allowlist
```

Names are case-insensitive, and harness spellings and aliases (`Bash`, `shell`, `AskUserQuestion`) resolve to the capability. A name Mars does not recognize is kept and reported as `unknown` in the launch bundle.

| Capability | Covers | Also accepted as |
|---|---|---|
| `bash` | shell commands | `shell`, `terminal`, `exec_command`, `shell_command` |
| `read`, `write`, `edit` | read, create, modify files | `cat`, `view`, `file_read`; `file_write`, `apply_patch`; `sed`, `str_replace` |
| `glob`, `grep` | search by name, by content | `find`; `rg`, `search`, `ripgrep` |
| `ls` | list one folder | |
| `notebook` | Jupyter notebook editing | `jupyter`, `notebook_edit` |
| `agent` | sub-agents | `subagent`, `spawn_agent` |
| `task` | the task list (not sub-agents) | `todo_read`, `todo_write`, `task_create`, `task_get`, `task_list`, `task_update`, `update_plan`, and the no-underscore spellings |
| `skill`, `workflow` | load a skill; run a workflow | |
| `web` | `web_search` and `web_fetch` together | |
| `web_search`, `web_fetch` | one of the two | `websearch`; `webfetch`, `fetch`, `curl` |
| `image` | view an image file | `view_image` |
| `ask_user` | ask the user a question | `askuser`, `request_user_input`, `ask_question`, `ask_user_question` |
| `work` | Meridian Flow's work tool | |
| `plan_mode`, `worktree`, `lsp`, `cron`, `monitor`, `notifications`, `send_user_file`, `schedule_wakeup`, `remote_trigger`, `tool_search` | features some harnesses have | `planmode`, `switch_mode`; `pushnotification`; `senduserfile`; `schedulewakeup`; `remotetrigger`; `toolsearch` |

`*` stands for every tool no other rule names, and `mcp(server)`, `mcp(server/tool)`, `mcp(server/*)`, `mcp(*/tool)` and `mcp(*/*)` gate MCP tools. Neither is a capability in the table.

The vocabulary also records, per host, which native tools a capability becomes. The hosts are `claude`, `codex`, `opencode`, `pi`, `antigravity` and `meridian-flow`. Each entry is one of:

- the host's tool names;
- *scoped tools*: Claude spells a scoped file rule through a permission category, such as `Edit(path)`;
- *absent*: the host has no such tool, so a deny is already met (`ls` on Claude and Codex; `work` everywhere but Meridian Flow);
- *no switch*: the host can do it but has no tool to turn off (Codex reads files through its shell).

The per-harness names are in [Agent compilation](agent-compilation.md#tool-name-projection). Meridian Flow is not a harness; its tools are `read`, `write`, `grep`, `ls`, `skill`, `ask_user`, `work`, and `spawn` with the `thread_*` family. Its `mcp(...)` rules and MCP wire tokens lower onto its connection tools (`mcp_describe`, `mcp_call`, `mcp_run`), which cannot be scoped to one server: a deny or ask covers all of them, and an allow or ask under an allow-list grants none (`known`, with a `note` saying so) ([`project`](../../npm/@meridian-flow/mars-schema/README.md#project)).

## The contract as data

The reader is exercised by fixtures in [`crates/mars-schema/fixtures/`](../../crates/mars-schema/fixtures/), a directory of cases per suite. Mars's tests (`cargo test -p mars-schema`) and the npm package (`node crates/mars-schema-wasm/run-fixtures.mjs`) both run them, and Meridian Flow runs the same corpus against the version of `@meridian-flow/mars-schema` it pins. A change to a pinned reader that changes a fixture's reading is a change to the contract.

| Suite | Pins |
|---|---|
| `read` | the reading of one Agent, skill or `mars.toml`: typed fields, ordered frontmatter pairs, body, diagnostics with codes and paths |
| `emit` | how a value is written as YAML for a splice, including quoting and refusals |
| `prepare` | classifying and reading one package root |
| `normalize` | reading a foreign repository as schema v1 packages |
| `vocabulary` | the capabilities, aliases and per-host native tools |

The API that reads them is described in the [crate](../../crates/mars-schema/README.md) and [npm](../../npm/@meridian-flow/mars-schema/README.md) READMEs.
