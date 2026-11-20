# `@meridian-flow/mars-schema`

WebAssembly build of [Mars schema v1](https://github.com/haowjy/mars-agents/blob/main/docs/config/schema-v1.md): the one reader of agent definitions (`agents/<slug>.md`), skills (`skills/<slug>/SKILL.md`) and `mars.toml` (`[package]`, the dependency tables and every consumer table: `[settings]`, `[models]`, `[agents]`, `[skills]`, `[plugin]`), the YAML emitter that lossless editors splice with, the tool vocabulary, and the code that reads a whole repository as Mars packages. The Mars CLI uses the same code, so a file means the same thing to an editor and to `mars sync`.

Types are in `index.d.ts`; the shapes are pinned by the shipped `fixtures/`.

```ts
import { read } from "@meridian-flow/mars-schema";

const reading = read("agents/coder.md", text);
reading.fields?.effort;            // "high" | ... | "auto" | null
reading.diagnostics;               // [{ code, severity, path, message }]
```

The package selects the Node or browser build through conditional exports. The Node build is synchronous. The web build is an ES module and must be instantiated first:

```ts
import init, { read } from "@meridian-flow/mars-schema"; // browser condition
await init();
```

`init` is typed only under the `browser` export condition (`index.web.d.ts`); with TypeScript's `bundler` resolution, set `customConditions: ["browser"]` for the web build.

## API

| Function | Does |
|---|---|
| `read(path, text)` | Reads one file. The package-relative `path` picks the reader: `mars.toml`, `agents/<slug>.md` or `skills/<slug>/SKILL.md`. Any other path throws `read.unrecognized-path`. |
| `narrowRows(rows, providers?)` | The rows of a project's `catalog_providers`, which `mars models resolve` resolves over. |
| `resolveAlias(rows, alias)`, `aliasCandidates(rows, alias)`, `resolveAliases(manifest, rows)` | Resolve a `[models.<alias>]` over models.dev rows, as `mars models resolve` does. See Effort and aliases. |
| `emitValue(value, indent)` | YAML for one value, to splice after a key. |
| `rewriteBlock(frontmatter)` | The whole frontmatter block between the `---` fences for ordered `[key, value]` pairs. |
| `vocabulary()` | The tool capabilities, their aliases, and the native tools of each host. |
| `project(rules, host)` | Lowers an Agent's tool rules (a `capability` may be a whole key such as `bash(git *)`) onto a host with the code `mars sync` uses: the native names each action ends with, each rule's gap or stricter rule, and how the host handles `ask`. |
| `prepare({files, context?, declaredDialect?})` | Reads one root held in memory: classifies it, translates it if it is foreign, and returns each definition's normalized bytes, typed reading and change record. Each Agent's `fields.tools` also holds the package's `[package.tools]` rules for keys the Agent does not name, with `origin: "package"`, as `mars sync` writes them. |
| `check({files, context?, declaredDialect?, dependencies?})` | What is wrong with a package, per file: what `prepare` returns for each file plus the check's own findings (names, duplicates, skill refs, a skill folder with no `SKILL.md`, requirement syntax), each with a level. See [`check`](#check). |
| `inspect({tree, hint?})` | Reports what a repository is: a marketplace's plugins, a plugin's or skill tree's items, the dialect and the evidence for it. |
| `normalize({tree, source?, hint?, selection?})` | Turns a selection into schema v1 packages with a generated `mars.toml` and provenance. |
| `satisfies(constraint, version)` | Whether a version meets a dependency entry's `version` (`^1.2`, `v2`, a bare `0.20` as `^0.20`, `latest`; a branch or commit pin is met by none). |
| `satisfiesRequirement(requirement, version)` | Whether a version meets a package's `requires-mars` or `requires-meridian`, where a bare `0.20` means `>=0.20.0`. |
| `selectVersion({available, constraints, locked?})` | Which ref a package ends at, from every `[requester, version \| null]` on it and the repository's tags: `{pin}` (a branch or commit), `{locked: true}` (the locked version still meets every semver range, whether or not the repository still has its tag), `{tag}` (the newest that meets every semver range) or `{untagged: true}` (no version tags). Throws when none qualifies. |
| `renderHooks(hookToml, targets, tokens?)` | A `hook.toml`'s `[hooks]` table as each target (`.claude`, `.codex`, …) takes it: finished entries or generated files, what the target leaves out, and the reader's diagnostics. |
| `selectItems({packages, root?, requests?})` | Which Agents, skills and bootstrap documents an install of `root` and its dependencies yields, and why. Takes each package's `prepare` result. |
| `resolve({root, locked?, refresh?, engine, bounds?, answers})` | Which version of which source, for a whole dependency graph: the rules of `mars sync` without the fetching. Returns the fetches it still needs, or the resolved lock rows, or the first failure. See [`resolve`](#resolve). |
| `version()` | The version of the schema crate this build was made from. |

A tree is a `Map<string, Uint8Array>` from `/`-separated relative paths to bytes. `prepare`, `inspect` and `normalize` read manifests, definitions and directory listings, never the bytes of a skill's other files.

### `read`

Every result is a plain object or array: absent values are `null`, bytes are `Uint8Array`.

```ts
interface DefinitionRead<Fields> {
  frontmatter: [string, JsonValue][];  // every top-level key in file order, unknown keys included
  fields: Fields | null;               // typed fields in file spellings; null if the file cannot be read at all
  body: string | null;                 // every byte after the closing fence; null for mars.toml
  diagnostics: Diagnostic[];
}
```

- `fields` has the kebab-case spellings the file uses (`"model-invocable"`, `"harness-overrides"`), `xhigh` and `read-only` as written, and `null` or `[]` where the file sets nothing. `AgentFields`, `SkillFields` and `ManifestFields` are in `index.d.ts`; for `mars.toml`, `frontmatter` holds the pairs of the `[package]` table.
- `diagnostics` carry a stable `code` (`field.removed`, `field.unknown`, `yaml.anchor`, ...), a `severity` of `error`, `warning` or `info`, and a `path` of keys and indexes into the file (`["model-policies", 0, "when"]`). Locate the diagnostic in the text with your own parser: `range` is for fence and YAML-syntax errors only, and no reader sets it yet. Errors do not stop the reading, so an editor can show the form and the problem together.
- `frontmatter` omits a key whose value has no JSON form (a YAML tag, a non-string key, an integer beyond ±(2^53 − 1), a `__proto__` key) and reports it as `yaml.non-json-value` (`toml.non-json-value` for `mars.toml`). Mapping keys keep file order at every depth; JavaScript objects list integer-like keys such as `"1"` first.
- An Agent whose first line is not `---` has no frontmatter, so every field takes its default and the file reads as the widest Agent. `read("agents/<slug>.md", text)` reports it as the error `frontmatter.missing`, with the defaults in `fields` and the whole file in `body`; `prepare` reports it the same way for a schema 1 package. A foreign package's body-only Agent is not an error, and a skill with no frontmatter keeps its `skill.no-frontmatter` warning.
- Frontmatter has two limits, because the YAML scanners take time that grows with the square of the nesting depth and a server that reads untrusted files must not stall: a block over 64 KiB is `frontmatter.too-large`, and one that opens `[` or `{` more than 64 deep (counted over the raw text) is `frontmatter.too-deep`. Either gives `fields: null` and no other diagnostic. Real frontmatter is under 2 KiB and nests a few levels; the limits apply to `read`, `prepare` and `normalize` alike.

### Editing without losing the rest of the file

`emitValue` and `rewriteBlock` write the values; your editor decides where the bytes go. For a surgical edit, read the file to learn its structure, find the value's range with a YAML library that has a CST (`yaml`'s `parseDocument` gives byte ranges), and splice:

```ts
import { emitValue } from "@meridian-flow/mars-schema";

// Replace the value of a top-level key; `key:` starts at column 0, so indent is 0.
const next = text.slice(0, valueStart) + emitValue("xhigh", 0) + text.slice(valueEnd);
```

The rules of `emitValue(value, indent)`, where `indent` is the column of the key that owns the value:

- A scalar is one line with no leading newline. Splice it after `key: `.
- A non-empty list or mapping starts with `\n`, and its children sit at column `indent + 2`. Splice it right after `key:`. An empty one is the one-line `[]` or `{}`.
- The output never ends in a newline.
- A string is plain only when it is a safe word (`[A-Za-z_][A-Za-z0-9_./-]*` that is not a YAML boolean, null, number or infinity word). Every other string, a multi-line one included, is JSON double-quoted, and so is every key that is not a safe word. `emitValue("012", 0)` is `"012"` with its quotes, so every YAML parser reads the string.
- A float of any finite magnitude is written so that both Rust's and JavaScript's YAML parsers read the same number.
- It throws for an integer beyond ±(2^53 − 1), `NaN` and `Infinity`, a string with a lone surrogate, and a mapping key written as more than 1024 characters.

`rewriteBlock(pairs)` is the whole-file form: it writes the block between the fences, keys in the order given, ending in exactly one newline (`""` for no keys), and throws for a duplicate key. It re-emits everything, so comments and quoting in the original are gone. Do not call it on a reading that has a `yaml.non-json-value` diagnostic: the left-out keys would vanish from the file.

After you splice, read the result again and check that each key's value equals the old reading with your change applied, and that no byte outside your edit moved. The fixtures include cases where two YAML libraries read a scalar differently (`012` is a string to Rust and the number 12 to JavaScript's `yaml`); this check is what keeps such a difference from reaching a saved file.

### `vocabulary`

```ts
const { capabilities, natives } = vocabulary();
capabilities;                      // [{ name: "bash", aliases: ["shell", ...] }, ...]
natives.claude.edit;               // { kind: "scoped-tools", unscoped: [...], scoped: [...], note }
natives["meridian-flow"].work;                 // { kind: "tools", tools: ["work"] }
```

`natives` has one table for each of `claude`, `codex`, `opencode`, `pi`, `cursor`, `agy` and `meridian-flow` (Meridian Flow's own tools, which is not a harness). An entry is the host's tool names (`tools`), file tools scoped through a permission category (`scoped-tools`), `absent` (the host has no such tool) or `no-switch` (the host can do it but has no tool to turn off). The capability table is explained in [Tool capabilities](https://github.com/haowjy/mars-agents/blob/main/docs/config/schema-v1.md#tool-capabilities).

### `project`

```ts
const { tools } = read("agents/a.md", text).fields;      // [{ capability, pattern, action }, ...]
const p = project(tools, "meridian-flow");
p.allow; p.deny; p.ask;     // native tool names after stricter rules win shared names
p.default;                  // "deny" for `{"*": deny}`: what a tool no rule names gets; null without a `*` rule
p.rules[0].status;          // "known" | "absent" | "unknown" | "unscopable" | null (for `*`)
```

`host` is `claude`, `codex`, `pi`, `opencode`, `cursor`, `agy` or `meridian-flow`. A rule with a gap has `status` other than `known`, no `native` names and the reason in `note`; a rule a stricter rule beats keeps its place in `rules` with `covered_by` set or the shared name missing from `native`. The result decides nothing: whether a gap is a warning or an error is the caller's policy. Meridian Flow's tools take no scope, so a rule with a pattern there is `unscopable`.

On `meridian-flow`, `mcp(...)` rules and MCP wire tokens (`mcp__server__tool`) lower onto Flow's connection tools (`mcp_describe`, `mcp_call`, `mcp_run`), which take the server as an argument and cannot be scoped to one. An `mcp(...)` deny, whatever it names, is `known` and disallows all of them, which can be broader than written, the safe direction (the rule's `note` says it covers every server). An `ask` lands on all of them the same way. An `allow` or `ask` under an allow-list (a `*` rule that is not `allow`) is `known` with no native names and a `note` saying none is granted, because granting them would give every connection and not the one named; show the note as a warning. With no allow-list the tools are available anyway, so an `allow` is `known` and loses nothing.

### `prepare`, `inspect`, `normalize`

`prepare` is the single-root core. It classifies the root (`schema-1`, `schema-newer`, a foreign dialect, `marketplace`, `invalid` or `empty`), selects definitions, translates a foreign package, and reads each result. For each definition it returns the source and output paths, the normalized bytes, the typed reading, and a change record (`none`, `generated` or `fully-rewritten`); payload files come back as source-to-output paths without bytes. Findings are keyed by source path in `diagnostics`; what preparing dropped, moved or repaired in each file (a foreign translation's moves) is keyed the same way in `changes`, and a file counts as changed when its change record is not `none` or `generated`. `context` is `"package"` (the default), `"authored-local"` (a consumer project's `.mars-src`) or `"project"` (a project directory: a `mars.toml` with no `[package]` is consumer configuration, and the tree is read as authored content). A tree with no manifest is read as one dialect, Claude unless an Agent has a `permission` map or a `tools` map of booleans (OpenCode). A translated definition is never refused: what schema v1 does not accept is moved under `harness-overrides.<dialect>` or dropped, except a key Mars lowers itself (`model`, `effort`, `tools`, `disallowed-tools`, `skills`, `mode`), which is never moved: a mapping loses only its invalid entries and a scalar is dropped. Losing a restriction is a change coded `field.restriction-lost` that names the agent; report these every time, since each leaves an agent less restricted than its file says. `fields.tools` of an Agent in `prepare`'s result is what `mars sync` installs: the package's `[package.tools]` rules for each key the Agent's own `tools:` does not name (two spellings of one key are one key) follow its own rules, marked `origin: "package"`; `normalized_bytes` stay the authored file's. `read` sees one file, so it never adds them. When the package's `mars.toml` has any error (an invalid `[package.tools]` entry included), each Agent's reading carries a `package-tools.unreadable` error: `mars sync` refuses that package and installs no Agent, so a consumer must not install it either. The findings that cost the author nothing are `info`: `manifest.version-missing`, `skill.name-invalid`, `skill.description-too-long` and `frontmatter.repaired`. The warning `skill.name-mismatch` no longer exists: a name that differs from the file or folder is `definition.name-mismatch` (`info`) in `check`, for every dialect. `prepare` is supported on its own, for reading one root without choosing items; `normalize` is `prepare` plus a selection, a generated `mars.toml` and provenance.

`inspect` reports what is in a repository before anything is chosen, and `normalize` builds the packages for a selection. A remote marketplace plugin is fetched by the caller: `inspect` the marketplace, fetch the entry's repository, then `inspect` or `normalize` the fetched tree with `hint.plugin` set to the entry. For a root whose `kind` is `invalid`, `inspect` puts the manifest's findings under `diagnostics["mars.toml"]` (`manifest.schema-missing` for a package with no `schema`).

Marketplace fields are untrusted input: a marketplace is third-party text, and an entry's repository and ref reach `git` as arguments. The crate checks them when it reads the entry, and an entry that fails comes back as `source.kind === "unsupported"` with a `reason` (and `refused` on the plugin), never as `remote`. A repository must be an `https://` or `ssh://` URL or a `user@host:path` address; a local repository (`file://`, an absolute path), `ext::`, `git://`, `http://` and the like are refused. A repository or ref that is empty, starts with `-` or holds whitespace or a control character is refused, and so is a `sha` that is not a full 40-character commit (it is never read as an unpinned checkout). A plugin name must be letters, digits, `-`, `_` and `.`, starting with a letter or digit (`is_plugin_name`); an entry that fails it is left out with `marketplace.plugin-name-invalid`, and a `plugin.json` name that fails it is ignored with `plugin.name-invalid`. The crate does not check that a repository exists, is reachable, or is on a host you allow: restricting hosts (GitHub only, say) is the caller's policy. The Claude rules (a plugin's `skills` list adds to the default folder, its `agents` list replaces it, commands are reported and not imported) are applied here; see [Dialect](https://github.com/haowjy/mars-agents/blob/main/docs/config/mars-toml.md#dialect).

### `check`

`check` is what `mars check` reports about a package's own files, from the files and its dependencies. It prepares the root as `prepare` does, so it takes `prepare`'s input and returns a `Check`: `diagnostics` (by source path, `mars.toml` first, then lexical), `dependency_diagnostics`, `missing_dependencies` and `unreadable_dependencies`.

A level says what the author loses. **Error**: the file or package won't load or install (a duplicate `name`, a skill ref no package ships, a requirement that isn't one). **Warning**: something written won't take effect (a skill folder with no `SKILL.md`, a restriction translation dropped, a skill ref to a package the file doesn't declare). **Info**: nothing written is lost (an Agent with no `description`, a `name` that differs from the file name, a repaired value). Show errors and warnings by default and info on request. A file with an error is unusable; an error on `mars.toml` makes the whole package unusable.

Per path, `diagnostics` lists what `prepare` returns for the file and then the check's own findings, errors first, so the two never disagree: dropping the check's codes leaves `prepare`'s list. The check's own codes are `package.requirement`, `agent.description-missing`, `agent.name-missing`, `definition.name-mismatch`, `definition.duplicate-name`, `selection.duplicate-slug`, `skill-ref.missing`, `skill-ref.provider`, `skill.no-definition`, and from a translated package `field.restriction-lost`, `field.execution-ignored` and `frontmatter.repaired`.

`dependencies` is the whole closure: every package reachable from the root through any `[dependencies]` entry, filtered or not, one entry per source, keyed by the name the declaring table gives it, each as `{ prepared: prepare(...) }` (the shape of `selectItems`' `packages`, `extra_items` included though `check` ignores it), as a plain object or a `Map`; an unknown key in an entry throws `input.invalid` either way. A dependency's own Agent that names a skill no package ships is `skill-ref.dependency` in `dependency_diagnostics`, by dependency and then the Agent's output path: it is not the root's to fix, so a Save that fails on errors in `diagnostics` leaves it out.

**`missing_dependencies` and `unreadable_dependencies` are not a verdict.** `missing_dependencies` lists the dependencies the call left out, with `declared_in` naming the package whose `mars.toml` declares each (`null` for the root). `unreadable_dependencies` has the same shape and lists the dependencies you passed that could not be read (their `prepared.selection.unreadable` is `true`: an invalid or newer-schema manifest); their own errors are in `prepare`'s result for them. While either is not empty no skill ref was judged, so a clean `diagnostics` does not mean the package passes: a missing dependency may be the one that ships the skill, and so may an unreadable one. Fetch or fix them and call again. A `prepare` result stored before 0.34.0 has no `unreadable` flag, so re-prepare stored dependencies.

`mars check` can fail a package this accepts: it also reports the `requires-mars` floor, hooks, plugin output, model aliases against `mars.lock`, what a harness target loses from an Agent, and an empty package, none of which depends on the files alone. See [`mars check`](https://github.com/haowjy/mars-agents/blob/main/docs/cli/commands.md#mars-check).

### `resolve`

`resolve` is the part of `mars sync` that decides: one version per name, every requester's constraint combined, restarts, `requires-*` exclusion, source identity, conflicts. It never fetches. It asks for what it needs, and you answer from wherever you fetch. It is stateless: keep the list of answers, and each call replays the resolution from all of them.

```ts
const answers: Answer[] = [];
for (;;) {
  const out = resolve({ root: { manifest }, locked, engine: { mars: version() }, answers });
  if (!("needs" in out)) return out;          // { resolved } or { failed }
  answers.push(...(await Promise.all(out.needs.map(fetchAnswer))));
}
```

| `Need` | You | Answer |
|---|---|---|
| `{kind: "tags", url}` | list the tags | `{need, tags: [{tag, commit}]}` |
| `{kind: "ref", url, ref}` | resolve a branch, `HEAD` or a commit to a commit | `{need, commit}` |
| `{kind: "tree", url, commit, path}` | fetch the files under `path` at `commit` (`null`: the whole repository) | `{url, commit, path, files}` |

- **Needs** are every open need on the frontier (one with no answer yet), sorted (tags, then refs, then trees, each by its fields), so fetch them in parallel. A later need can depend on an earlier answer, so a call can return needs more than once. While one is open the result is `needs`, even if a failure is already certain: it waits for the needs before it.
- **Commits are peeled.** A tag's `commit` is the commit an annotated tag points at, never the tag object. Tags need no order, and tags that are not versions are ignored.
- **A tree need's `commit` can be a tag object's id.** Locks written by Mars 0.34 and earlier name annotated tags that way. Peel it (`GET /git/tags/{sha}`), or fetch through `tarball`, and answer with the id from the need.
- **Answers are keyed, and the later one wins.** A tags answer is keyed by `url`, a ref answer by `(url, ref)`, a tree answer by `(url, commit, path)`. Two answers with one key: the later one in the list is the answer, found, `missing` or `failed`, so a retry is appended. The result depends on which answer is last for each key, never on the order of the list.
- **Tree answers** also cover the folders below their `path`, so a whole-repository answer cached by commit serves every later need at that `url` and `commit`. A need takes the answer for its own key, else the one for the nearest folder above it, else the whole repository's. A found answer for the need, or for a nearer folder, beats a broader one that `failed` or is `missing`, and the reverse: a `failed` answer for the need itself is that need's answer even when a whole-repository answer was found. `files` paths are repository-relative. A package with a `subpath` asks for that folder; a package with none, and every marketplace plugin, asks for the root, because the marketplace reader needs its root `mars.toml`, the marketplace file and the local plugins' folders. Send files only (a symlink as its target's content; one that points outside the repository is an error to report as `failed`). Binary files may be left out.
- **`{need, missing: true}`** says the commit or ref does not exist. For a commit from `locked` it drops that row with the `locked-commit-unreachable` warning, and resolution picks again from the tags, as `mars sync` does; anywhere else it is an `unavailable` failure. **`{need, failed: reason}`** is any other error, always `unavailable`, and never drops a lock, so a transient error cannot move one.
- **Path dependencies** (`path = …`) are never a need: a fetched package's path names its author's disk, so it is the `path` failure. `[local-dependencies]` are never followed.

`locked` is mars.lock's `[dependencies.*]` rows. A row is kept while it meets every constraint and its source matches the declared one. `refresh` ignores it for the names listed (`mars upgrade <name>`) or for all (`"all"`). `engine.mars` judges `requires-mars`; `engine.meridian`, when given, judges `requires-meridian`; a version whose requirement is unmet is skipped for an older one, with a `requires-mars-fallback` advisory. `bounds.packages` caps the dependencies in the closure. `bounds.bytes` caps the bytes of the files Mars reads: it counts afresh at each restart and is checked at the end of each pass, so a whole-repository answer costs only the files Mars opens. A `bounds` failure's `at` is the package where the limit was crossed.

The result is one of:

- `{ needs }`
- `{ resolved: { lock, dependencies }, diagnostics }`: `lock` is exactly the `[dependencies.<name>]` rows of `mars.lock`, to store verbatim (urls in the lock's canonical spelling). `dependencies` has, per name, what to vendor and to give `check` and `selectItems`: `url`, `commit`, `subpath`, the `version` tag (`null` when untagged or pinned to a ref), `content` for a marketplace plugin (where its files are, in another repository or in the marketplace's own tree) and `required_by`, each requester (`"root"` for the root) with the constraint it asked for.
- `{ failed, diagnostics }`: the first failure, with `kind`, `message` (the text `mars sync` prints for it) and the fields of its kind (see `ResolveFailure` in `index.d.ts`). A failure of the resolution is data; only an argument that is not the shape above throws `input.invalid`.

`diagnostics` are the notes of the final pass, plus the notes of choices kept across restarts (an engine fallback, a stale lock row).

A package cycle resolves when its constraints agree, as in `mars sync`; each package is in the other's `required_by`. The edge that closes a cycle is held to its constraint like any other: `a ^1` that needs `b`, which needs `a ^2`, is a `conflict`. `supplied` (an edited manifest over a fetched tree) is reserved and throws for now.

### Effort and aliases

`resolveEffort(requested, model, host)` is the effort rule `mars build launch-bundle` and `mars sync` apply, so a gateway and Mars agree on what is sent. The ladder is `none < minimal < low < medium < high < xhigh < max` (`effortLevels()`); `"auto"` and `null` both mean "send no effort". `model` is the models.dev row's `reasoning` and `reasoning_options`, or `null` for an unknown model; `host` is a harness or `meridian-flow`. A level that is on neither the model's list nor the host's moves to the nearest level on the ladder that both take, below or above (a tie goes up), and never lands on `none` unless `none` was asked (off is not less thinking); the result is `{ level, moved }`, with `moved.why` one of `model_does_not_reason`, `model_levels`, `harness_levels`, `no_common_level`. A model that reasons but lists no effort values (toggle only, budget only, no control) takes every level, less `none` unless it has a toggle, so `none` moves up to the lowest level the host takes; an unknown model (`null`) takes any level; a model that does not reason gets none. Each host's levels are recorded, with the version they were checked against, on `harness_levels` in the crate.

`read('mars.toml', text)` returns every table Mars reads, so a caller needs no second reader: beside `fields` (`[package]`) and `dependencies` / `local-dependencies` it has `models`, `settings`, `agents`, `skills` and `plugin`, each left out when empty. A `mars.toml` with no `[package]` is consumer configuration: `fields` is `null`, nothing is reported for the missing table, and the dependencies and the other tables are returned. The CLI loads `mars.toml` through the same code, so the diagnostics match what `mars validate` prints. Each table is read on its own: `[settings]`, each `[models.<alias>]`, each `[agents.<name>]`, each `[skills.<name>]`. One that breaks a rule is left out with a `config.invalid` error whose `path` names it (`["models", "a"]`), and every other table is returned; a broken table never hides another error. A refusal (`[settings.tools]` with a `*`, a `min_mars_version` that is not a requirement) is an error at its path with the table kept. (`mars sync` refuses a project's own `mars.toml` whole on any of these; a caller that wants the same refusal treats any `config.invalid` error as one.) A key Mars does not read is a `field.unknown` info. Table fields come back as Mars writes them (`default_effort`, `model-invocable`), not as the file spelled them: `network_allow` reads as `network-allow`. Every table is plain JSON like `fields`' `frontmatter`: a TOML datetime is its text, and an integer beyond ±(2^53 − 1) or a `__proto__` key is left out with a `toml.non-json-value` error at its path.

`resolveAlias(rows, alias)` is the rule `mars models resolve` applies to an alias: it returns the model id, or `null`. `alias` is a `ModelAlias`, the object `read` returns under `models` or one built in code. A pinned `model` is taken as written; an `auto` alias selects among the rows by `match`, `exclude` and `provider` (newest `release_date` first). The old call form, `{ match, exclude, provider }`, is an `auto` alias and still works. `aliasCandidates(rows, alias)` lists every id the alias accepts in the same order: a pinned `model` first, then the selected rows. `resolveAliases(manifest, rows)` resolves each alias of a parsed manifest's `models` and returns `{ [name]: id | null }`. The three check an alias the same way: a key `ModelAlias` does not have throws `input.invalid`. Rows are models.dev rows, with `provider` the models.dev key; a row from Mars's own snapshot may carry `provider_key`, which then wins.

**Narrow the rows first.** `mars models resolve` resolves over the providers the project's `catalog_providers` names (`anthropic`, `openai`, `google`, `meta`, `deepseek`, `xai` and `openrouter` when it sets none), not over all of models.dev. Handed every row, `resolveAlias` can pick a different model, because a provider the project does not read has a newer or shorter id. `narrowRows(rows, providers)` is that rule: pass the project's `settings.catalog_providers` (`["*"]` keeps every provider, `undefined` the default list) and resolve over what it returns.

### Versions and selection

The version functions are the rules `mars sync` applies, so a caller that resolves its own dependencies agrees with it. `selectVersion` is the whole decision for one package: it settles the constraints first (one pin wins; two pins, or a pin beside a range, throw), ignores tags that are not versions (`latest`, `nightly-2024`), treats `latest` and `null` constraints as any version (a locked package stays), and with no version tag left answers `{untagged: true}`. Tags are strings (`v1.2.3` or `1.2.3`).

Every export throws an `Error` with a `code` (`MarsSchemaError` in `index.d.ts`), so a caller maps failures by code and never by message.

`selectItems` runs the pass that decides what an install yields. Prepare each package of the graph, then pass the results unchanged. For a package you import, `root` is the package and `packages` its dependencies; the root's `[dependencies]` entries apply with their filters and `exclude`, as the consumer's own requests:

```ts
const result = selectItems({
  requests: [],
  root: { prepared: prepare({ files: rootFiles }) },
  packages: { base: { prepared: prepare({ files: baseFiles }) } },
});
// result.items: [{package, kind, slug, path, required_by}], result.missing_skills, result.diagnostics
```

Each package's edges and `uses-providers` are read from `prepared.selection`, so nothing is derived twice. A filtered entry of a dependency's manifest seeds nothing; only the root's or the consumer's own `exclude` blocks. A skill an Agent names resolves to a package in its own, then its dependencies, then any other package in registry order, which `selectItems` computes from `root` and `requests`; `packages` is a set. Items of `root` have `package: null`, and a slug that `root` and a dependency both install is a `selection.duplicate-slug` diagnostic. `missing_skills` lists refs no package ships. `kind` values are `agent`, `skill`, `hook`, `mcp-server` and `bootstrap-doc`; `selectItems` decides only Agents, skills and bootstrap documents; an `mcp-server` in `extra_items` throws `input.invalid`. Hooks and MCP servers are selected by the binary (visibility and depth), so Flow and `mars sync` are guaranteed to agree only on those three kinds. Input keys are snake_case; an unknown key (a misspelled filter, `rename`) throws `input.invalid`.

## Fixtures

`fixtures/` holds the cases the package is tested with, one directory per case:

| Suite | Case holds | Pins |
|---|---|---|
| `read` | the file, `expected.json` | the reading of one definition or manifest |
| `emit` | `value.json`, `expected.txt` | the YAML for one value |
| `prepare` | `input/`, `request.json`, `expected.json` | classifying and reading one root |
| `normalize` | `input/`, `request.json`, `inspection.json`, `normalized.json`, `expected/` | reading a foreign repository |
| `vocabulary` | `expected.json` | the capabilities, aliases and native tools |
| `project` | `request.json`, `expected.json` | `project` for one rule list on one host |
| `version` | `input.json`, `expected.json` | `satisfies`, `satisfiesRequirement` and `selectVersion`, with the `code` of each refusal (the Rust leg also pins `parse-constraint` and `parse-tag`, which are not exported) |
| `check` | `input/`, `request.json`, optional `deps/<name>/`, `expected.json` | `check` over a root and its dependencies (each prepared first), the order of the paths included |
| `selection` | `request.json`, `packages/` (or `packages_from`: a sibling case's), `expected.json` | `selectItems` over packages prepared from `packages/` |
| `resolve` | `steps.json` (a base input and the answers each call adds, file contents as text), `expected.json` | the output of `resolve` at every call of a loop |

`node crates/mars-schema-wasm/run-fixtures.mjs` in the Mars repository runs every case against this package and compares results deeply, key order included. Run the same cases against the version you pin; a change to a pinned reader that changes a result is a change to the contract.

## Build

`npm run build` (needs `wasm-bindgen-cli`) builds `node/`, `web/` and `fixtures/`, which are generated and not committed.
