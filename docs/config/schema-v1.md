# Schema v1

Schema v1 is the format of a Mars package's definition files: Agents (`agents/<name>.md`), skills (`skills/<name>/SKILL.md`) and the `[package]` table of `mars.toml`. One reader in the [`mars-schema`](../../crates/mars-schema/AGENTS.md) crate parses them for the Mars binary, for the [`@meridian-flow/mars-schema`](../../npm/@meridian-flow/mars-schema/README.md) npm package that editors use, and for Meridian Flow. A file the reader accepts means the same thing to every consumer.

A package declares the format it uses; a package that predates it can be upgraded with [`mars migrate`](migrating-to-schema-v1.md).

## What schema 1 promises

```toml
[package]
name = "my-package"
version = "1.0.0"
schema = 1
```

- **Same bytes, same meaning.** Within schema 1 a key keeps its meaning and keeps being accepted. A minor Mars release can add an optional key. Changing what a key means, or removing one, needs `schema = 2`, with an upgrade path from 1.
- **Strict.** A package that declares `schema = 1` is read as written. A key schema 1 removed, a value it does not allow, and a malformed file are errors that name the file and the replacement; there is no silent fallback. Unknown keys are preserved and reported as warnings.
- **Published as data.** The key tables below, the tool vocabulary, and a corpus of input files with their expected readings are fixtures that Mars's tests and Flow's tests run. See [The contract as data](#the-contract-as-data).

## How Mars reads a package

Mars classifies a package root before it reads any definition:

| `mars.toml` | Mars does |
|---|---|
| `[package] schema = 1` | Reads every definition strictly. Errors fail `mars sync`. |
| `[package]` without `schema` | Treats the package as pre-v1: upgrades its definitions **in memory**, installs the upgraded text, and warns once per package: ``package `name` uses the pre-v1 format (3 files to upgrade); run `mars migrate` in its repository``. The package's files are not touched. |
| `schema` higher than 1, or not an integer | Refuses the package. A Mars that does not know the schema cannot read it. |
| no `mars.toml` | The tree is a foreign layout (a Claude plugin, a marketplace, a tree of `SKILL.md` directories, a Codex, OpenCode or Cursor repository). Mars translates it into a schema v1 package; a tree of plain `agents/` and `skills/` folders is translated by Claude's rules (or OpenCode's, when an Agent has a `permission` map or a `tools` map of booleans), and the keys only pre-v1 Mars defined are rewritten per key whatever the dialect. A key or value the translation cannot make schema v1 is moved under `harness-overrides.<dialect>` (or dropped when no harness reads it, or when Mars lowers it itself) with a warning, never an error: only an authored schema v1 file is read strictly. See [Dialect](mars-toml.md#dialect). |

The same classification runs for dependencies, the local package, `.mars-src/`, `mars check`, `mars validate`, `mars migrate` and plugin builds. Reading finishes for the whole dependency graph before any content is written; see [the sync pipeline](../internals/sync-pipeline.md#preparation-and-the-gate).

### File rules

- **Frontmatter** is the block between a first line that is exactly `---` (after an optional byte-order mark) and the next line that is exactly `---`. Lines end in LF or CRLF. A file whose first line is not `---` has no frontmatter and is all body. A fence that opens and never closes is an error (`frontmatter.unclosed`).
- **The body** is every byte after the closing fence's line.
- **YAML** must be a mapping, or empty. Schema 1 refuses duplicate keys, anchors and aliases (`yaml.anchor`), and a `...` document-end marker (`yaml.document-end`). A top level that is a list or a scalar is `frontmatter.not-mapping`.
- **Booleans** are `true` and `false`. `yes`, `no`, `on` and `off` are strings, so they are errors in a boolean field.
- **Case.** Enumerated values are exact, except `effort` and `harness`, which are case-insensitive.
- **Unknown keys** are kept in the authored file and reach no native file, in Agents and skills alike. Each produces a `field.unknown` warning; [Unknown keys](#unknown-keys) says where harness-specific keys belong.

## Agent keys

| Key | Value | Notes |
|---|---|---|
| `name` | non-empty string | Optional. The file name is the Agent's identity when omitted. |
| `description` | string | Shown in listings and written to native files. |
| `harness` | `claude`, `codex`, `opencode`, `cursor`, `pi` | Optional routing hint. An unknown name is an error. |
| `model` | string | An alias from `[models]` or a concrete model ID. |
| `mode` | `primary`, `subagent` | |
| `model-invocable` | boolean, default `true` | Whether another model can invoke this Agent. |
| `user-invocable` | boolean, default `true` | Whether the user can invoke it directly. |
| `approval` | `default`, `auto`, `confirm`, `never` | Tool-call approval policy. `default` is an explicit request for the harness default; leaving the key out means unset. |
| `sandbox` | `default`, `read-only`, `workspace-write`, `danger-full-access` | Filesystem access. |
| `network` | `allow`, `deny`, `allowlist` | Shell network access; see [Network](#network). |
| `network-allow` | list of hosts, or `{replace: [...]}` | Hosts an `allowlist` permits. |
| `network-deny` | list of hosts | Hosts that stay denied. A package's Agent cannot replace this set. |
| `effort` | `none`, `low`, `medium`, `high`, `xhigh`, `max` | See [Effort](#effort). |
| `skills` | list, or `{load: [...], available: [...]}` | A bare list means `load`. |
| `subagents` | list of names | The Agents this one may spawn. |
| `tools` | map of capability to `allow`, `deny` or `ask` | See [Tool capabilities](#tool-capabilities). |
| `harness-overrides` | map of harness name to a map of native keys | See [harness-overrides](#harness-overrides-and-the-flow-namespace). |
| `model-policies` | list of rules | See [Model policies](#model-policies). |

Field-by-field behaviour is in [Agent profiles](agent-profiles.md).

### Removed Agent keys

A schema 1 file that carries one of these is an error (`field.removed`) naming the replacement. [`mars migrate`](migrating-to-schema-v1.md) applies the replacement where one can be applied mechanically.

| Removed | Replace with |
|---|---|
| `disallowed-tools` | `tools: {<capability>: deny}` |
| `tools` as a list | a `tools` map (`tools.list-form`) |
| `mcp-tools` | `mcp(server)` or `mcp(server/tool)` entries in `tools` |
| `model_invocable`, `user_invocable` | `model-invocable`, `user-invocable` |
| `approval: yolo` | `approval: never` |
| `permission` | `harness-overrides.flow.permission` |
| `fanout` | the consumer's `[settings.meridian.fanout]` |
| `models` | `model-policies` |
| `network_allow`, `network_deny` | `network-allow`, `network-deny` |
| `autocompact`, `autocompact_pct` | the same keys on the model alias, in `[models.<alias>]` |

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

### Removed skill keys

| Removed | Replace with |
|---|---|
| `type` | `metadata.type` |
| `when_to_use` | fold it into `description` |
| `tools`, `disallowed-tools`, `allowed-tools` (and their snake_case spellings) | nothing: a skill carries no tool policy |
| `model_invocable`, `user_invocable` | `model-invocable`, `user-invocable` |
| `invocation`, `disable-model-invocation`, `allow_implicit_invocation` | `model-invocable` and `user-invocable` |

## Manifest keys

The reader covers the `[package]` table of `mars.toml`. The other tables (`[dependencies]`, `[settings]`, `[models]`, `[plugin]`, the `[agents.*]` and `[skills.*]` overlays) are Mars's consumer and publishing configuration; see [mars.toml](mars-toml.md).

| Key | Value | Notes |
|---|---|---|
| `name` | non-empty string | Required. |
| `version` | string | A missing version is a warning. |
| `schema` | integer `1` | Required for schema 1. |
| `description`, `homepage`, `license` | string | |
| `authors` | list of strings or `{name, email}` tables | |
| `requires-mars`, `requires-meridian` | version requirement | Tool-version hints; see [mars.toml](mars-toml.md#package-optional). |
| `uses-providers` | list of provider names | The hosts the package expects to supply skills at sync time, such as `["meridian"]`. Each entry matches `[a-z0-9][a-z0-9-]*` (a `field.value` error otherwise). Mars never fetches or requires a provider; the list only lets the package's agents name that provider's skills. See [mars.toml](mars-toml.md#provided-skills-and-uses-providers). |

Removed: `package.visibility`, with nothing to replace it.

## Effort

`effort` is the reasoning effort for the Agent's model: `none`, `low`, `medium`, `high`, `xhigh` or `max`, in rising order; values are case-insensitive. `xhigh` and `max` are two levels. Claude Code has both. Codex's highest is `xhigh`, so Mars writes `max` there as `xhigh`, in the native file and in the launch bundle, and warns that it approximated. On Cursor the model's slug carries the effort: `max` selects the model's `-max` variant, else its `-xhigh`, else its `-high`, and warns when it used a lower one.

**`none` means reasoning off.** It is a value of its own: an absent `effort` means "the model's default", and `none` asks for no reasoning. Mars keeps the distinction end to end. The launch bundle carries `"effort": "none"`, and each harness's native file spells it as that harness does:

| Harness | `effort: none` in the native agent file |
|---|---|
| Claude Code | `effort` is omitted, with a lossiness warning: Claude Code accepts `low` to `max` and has no off value |
| Codex | `model_reasoning_effort = "none"`. Some Codex models refuse `none`; that is for the author to fix |
| OpenCode, Pi | no effort field is written for any value |
| Cursor | no effort field is written; the model slug is chosen by effort, and `none` selects Cursor's default tier |

The launcher reads the bundle's `effort` and maps it for its harness. The bundle never carries an effort the harness refuses: Codex gets `xhigh` for `max`, and Cursor gets none, because the effort is already in `harness_model`. `mars build launch-bundle --effort none` overrides the Agent from the command line.

`adaptive` and `disabled` are not values. `mars migrate` rewrites them to `medium` and `none`.

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
- **`when`** is one non-empty line saying when this model is a good choice for this Agent. It is guidance for the caller, not a condition Mars evaluates. `mars models prompting @agent` prints each rule's `when`, and the Agent inventory in the system prompt marks an Agent that has a guide. `mars check` warns when a rule's `alias` is not defined.
- **`no-fallback`** keeps a rule from being a launch backup. See [Agent profiles](agent-profiles.md#model-policies).

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

So the Agent's own policy wins over its top-level field: top-level `effort` is the default for any model without a rule, and a rule is the exception for one model. Native agent files and plugin builds resolve the same chain, without the command-line layer, so a native file and a launch bundle agree.

## harness-overrides and the flow namespace

`harness-overrides.<name>` holds configuration that one consumer owns, on an Agent or a skill. Mars checks only that it is a map of serializable values and keeps it in the installed file. It does two things with a harness block:

- **Native file.** The block for a harness is written into that harness's own Agent or skill file (frontmatter for Markdown, top-level keys for Codex TOML), and into that harness's plugin tree. No other harness sees it. A key Mars lowers itself (`model`, `tools`, `effort`, `description` and the rest) keeps Mars's value; the override is dropped with a warning that names it.
- **Launch bundle.** The block for the resolved harness reaches the launcher as `execution_policy.native_config`.

It never replaces a top-level field of the same name.

```yaml
harness-overrides:
  codex:
    model_reasoning_summary: concise                  # a Codex config key, carried as written
  flow:
    permission: read                                  # Meridian Flow's own key
```

- A name that is one of the five harnesses is a harness block.
- **`flow` is reserved** for Meridian Flow. It is not a harness: routing never sees it and Mars never interprets its keys. It is the home of Flow-only keys, such as `permission` (formerly a top-level key).
- Any other name is a warning (`harness.unknown`) and the block is kept.

### Unknown keys

An unknown key is a warning (`field.unknown`) and stays in the authored file, but it reaches **no** native file, in Agents and skills alike. Harness-specific keys belong under `harness-overrides.<harness>`, where they go to that harness alone.

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

The vocabulary also records, per host, which native tools a capability becomes. The hosts are `claude`, `codex`, `opencode`, `pi`, `cursor` and `flow`. Each entry is one of:

- the host's tool names;
- *scoped tools*: Claude spells a scoped file rule through a permission category, such as `Edit(path)`;
- *absent*: the host has no such tool, so a deny is already met (`ls` on Claude, Codex and Cursor; `work` everywhere but Flow);
- *no switch*: the host can do it but has no tool to turn off (Codex reads files through its shell).

The per-harness names are in [Agent compilation](agent-compilation.md#tool-name-projection). Flow is not a harness; its tools are `read`, `write`, `glob`, `grep`, `ls`, `skill`, `ask`, `work`, and `spawn` with the `thread_*` family.

## The contract as data

The reader is exercised by fixtures in [`crates/mars-schema/fixtures/`](../../crates/mars-schema/fixtures/), a directory of cases per suite. Mars's tests (`cargo test -p mars-schema`) and the npm package (`node crates/mars-schema-wasm/run-fixtures.mjs`) both run them, and Flow runs the same corpus against the version of `@meridian-flow/mars-schema` it pins. A change to a pinned reader that changes a fixture's reading is a change to the contract.

| Suite | Pins |
|---|---|
| `read` | the reading of one Agent, skill or `mars.toml`: typed fields, ordered frontmatter pairs, body, diagnostics with codes and paths |
| `emit` | how a value is written as YAML for a splice, including quoting and refusals |
| `upgrade` | the pre-v1 to v1 rewrite of one definition or manifest |
| `prepare` | classifying and reading one package root |
| `normalize` | reading a foreign repository as schema v1 packages |
| `vocabulary` | the capabilities, aliases and per-host native tools |

The API that reads them is described in the [crate](../../crates/mars-schema/README.md) and [npm](../../npm/@meridian-flow/mars-schema/README.md) READMEs.
