# `@meridian-flow/mars-schema`

WebAssembly build of [Mars schema v1](https://github.com/haowjy/mars-agents/blob/main/docs/config/schema-v1.md): the one reader of agent definitions (`agents/<slug>.md`), skills (`skills/<slug>/SKILL.md`) and the `[package]` table of `mars.toml`, the YAML emitter that lossless editors splice with, the tool vocabulary, and the code that reads a whole repository as Mars packages. The Mars CLI uses the same code, so a file means the same thing to an editor and to `mars sync`.

Types are in `index.d.ts`; the shapes are pinned by the shipped `fixtures/`.

```ts
import { read } from "@meridian-flow/mars-schema";

const reading = read("agents/coder.md", text);
reading.fields?.effort;            // "high" | ... | null
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
| `emitValue(value, indent)` | YAML for one value, to splice after a key. |
| `rewriteBlock(frontmatter)` | The whole frontmatter block between the `---` fences for ordered `[key, value]` pairs. |
| `vocabulary()` | The tool capabilities, their aliases, and the native tools of each host. |
| `prepare({files, context?, declaredDialect?, migrate?})` | Reads one root held in memory: classifies it, upgrades or translates it, and returns each definition's normalized bytes, typed reading and change record. `migrate` adds `mars migrate`'s repair: a removed Agent key whose replacement is a `harness-overrides` key moves there. |
| `inspect({tree, hint?})` | Reports what a repository is: a marketplace's plugins, a plugin's or skill tree's items, the dialect and the evidence for it. |
| `normalize({tree, source?, hint?, selection?})` | Turns a selection into schema v1 packages with a generated `mars.toml` and provenance. |
| `satisfies(constraint, version)` | Whether a version meets a dependency entry's `version` (`^1.2`, `v2`, a bare `0.20` as `^0.20`, `latest`; a branch or commit pin is met by none). |
| `satisfiesRequirement(requirement, version)` | Whether a version meets a package's `requires-mars` or `requires-meridian`, where a bare `0.20` means `>=0.20.0`. |
| `selectVersion({available, constraints, locked?})` | Which ref a package ends at, from every `[requester, version \| null]` on it and the repository's tags: `{pin}` (a branch or commit), `{tag}` (the newest that meets every semver range, or the locked one when it still meets them) or `{untagged: true}` (no version tags). Throws when none qualifies. |
| `selectItems({packages, root?, requests?})` | Which Agents, skills and bootstrap documents an install of `root` and its dependencies yields, and why. Takes each package's `prepare` result. |
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

`natives` has one table for each of `claude`, `codex`, `opencode`, `pi`, `cursor` and `meridian-flow` (Meridian Flow's own tools, which is not a harness). An entry is the host's tool names (`tools`), file tools scoped through a permission category (`scoped-tools`), `absent` (the host has no such tool) or `no-switch` (the host can do it but has no tool to turn off). The capability table is explained in [Tool capabilities](https://github.com/haowjy/mars-agents/blob/main/docs/config/schema-v1.md#tool-capabilities).

### `prepare`, `inspect`, `normalize`

`prepare` is the single-root core. It classifies the root (`schema-1`, `mars-legacy`, `schema-newer`, a foreign dialect, `marketplace`, `invalid` or `empty`), selects definitions, upgrades a pre-v1 package or translates a foreign one, and reads each result. For each definition it returns the source and output paths, the normalized bytes, the typed reading, and a change record (`none`, `generated`, `edited` with the changed key paths, or `fully-rewritten`); payload files come back as source-to-output paths without bytes. Findings are keyed by source path in `diagnostics`; what preparing dropped, moved or repaired in each file (a pre-v1 upgrade's removals, a foreign translation's moves) is keyed the same way in `changes`, and a file counts as changed when its change record is not `none` or `generated`. `context` is `"package"` (the default), `"authored-local"` (a consumer project's `.mars-src`) or `"project"` (a project directory: a `mars.toml` with no `[package]` is consumer configuration, and the tree is read as authored content). A tree with no manifest is read as one dialect, Claude unless an Agent has a `permission` map or a `tools` map of booleans (OpenCode), and the keys only pre-v1 Mars defined are rewritten one by one whatever that dialect is, so they never change how the rest of the tree is read. A translated or upgraded definition is never refused: what schema v1 does not accept is moved under `harness-overrides.<dialect>` or dropped, except a key Mars lowers itself (`model`, `effort`, `tools`, `disallowed-tools`, `skills`, `mode`), which is never moved: a mapping loses only its invalid entries and a scalar is dropped. Losing a restriction is a change coded `field.restriction-lost` that names the agent; report these every time, since each leaves an agent less restricted than its file says. `prepare` is supported on its own, for reading one root without choosing items; `normalize` is `prepare` plus a selection, a generated `mars.toml` and provenance.

`inspect` reports what is in a repository before anything is chosen, and `normalize` builds the packages for a selection. A remote marketplace plugin is fetched by the caller: `inspect` the marketplace, fetch the entry's repository, then `inspect` or `normalize` the fetched tree with `hint.plugin` set to the entry.

Marketplace fields are untrusted input: a marketplace is third-party text, and an entry's repository and ref reach `git` as arguments. The crate checks them when it reads the entry, and an entry that fails comes back as `source.kind === "unsupported"` with a `reason` (and `refused` on the plugin), never as `remote`. A repository must be an `https://` or `ssh://` URL or a `user@host:path` address; a local repository (`file://`, an absolute path), `ext::`, `git://`, `http://` and the like are refused. A repository or ref that is empty, starts with `-` or holds whitespace or a control character is refused, and so is a `sha` that is not a full 40-character commit (it is never read as an unpinned checkout). A plugin name must be letters, digits, `-`, `_` and `.`, starting with a letter or digit (`is_plugin_name`); an entry that fails it is left out with `marketplace.plugin-name-invalid`, and a `plugin.json` name that fails it is ignored with `plugin.name-invalid`. The crate does not check that a repository exists, is reachable, or is on a host you allow: restricting hosts (GitHub only, say) is the caller's policy. The Claude rules (a plugin's `skills` list adds to the default folder, its `agents` list replaces it, commands are reported and not imported) are applied here; see [Dialect](https://github.com/haowjy/mars-agents/blob/main/docs/config/mars-toml.md#dialect).

### Versions and selection

The version functions are the rules `mars sync` applies, so a caller that resolves its own dependencies agrees with it. `selectVersion` is the whole decision for one package: it settles the constraints first (one pin wins; two pins, or a pin beside a range, throw), ignores tags that are not versions (`latest`, `nightly-2024`), treats `latest` and `null` constraints as any version, and with no version tag left answers `{untagged: true}`. Tags are strings (`v1.2.3` or `1.2.3`).

Every export throws an `Error` with a `code` (`MarsSchemaError` in `index.d.ts`), so a caller maps failures by code and never by message.

`selectItems` runs the pass that decides what an install yields. Prepare each package of the graph, then pass the results unchanged. For a package you import, `root` is the package and `packages` its dependencies; the root's `[dependencies]` entries apply with their filters and `exclude`, as the consumer's own requests:

```ts
const result = selectItems({
  requests: [],
  root: { prepared: prepare({ files: rootFiles, migrate: true }) },
  packages: { base: { prepared: prepare({ files: baseFiles, migrate: true }) } },
});
// result.items: [{package, kind, slug, path, required_by}], result.missing_skills, result.diagnostics
```

Each package's edges and `uses-providers` are read from `prepared.selection`, so nothing is derived twice. A filtered entry of a dependency's manifest seeds nothing; only the root's or the consumer's own `exclude` blocks. A skill an Agent names resolves to a package in its own, then its dependencies, then any other package in registry order, which `selectItems` computes from `root` and `requests`; `packages` is a set. Items of `root` have `package: null`, and a slug that `root` and a dependency both install is a `selection.duplicate-slug` diagnostic. `missing_skills` lists refs no package ships. `kind` values are `agent`, `skill`, `hook`, `mcp-server` and `bootstrap-doc`; `selectItems` decides only Agents, skills and bootstrap documents. Hooks and MCP servers are selected by the binary (visibility and depth), so Flow and `mars sync` are guaranteed to agree only on those three kinds. Input keys are snake_case; an unknown key (a misspelled filter, `rename`) throws `input.invalid`.

## Fixtures

`fixtures/` holds the cases the package is tested with, one directory per case:

| Suite | Case holds | Pins |
|---|---|---|
| `read` | the file, `expected.json` | the reading of one definition or manifest |
| `emit` | `value.json`, `expected.txt` | the YAML for one value |
| `upgrade` | `input.md`, `expected.md`, `expected-diagnostics.json` | the pre-v1 to v1 rewrite |
| `prepare` | `input/`, `request.json`, `expected.json` | classifying and reading one root, and with `"migrate": true` the repair `mars migrate` writes back |
| `normalize` | `input/`, `request.json`, `inspection.json`, `normalized.json`, `expected/` | reading a foreign repository |
| `vocabulary` | `expected.json` | the capabilities, aliases and native tools |
| `version` | `input.json`, `expected.json` | `satisfies`, `satisfiesRequirement` and `selectVersion`, with the `code` of each refusal (the Rust leg also pins `parse-constraint` and `parse-tag`, which are not exported) |
| `selection` | `request.json`, `packages/` (or `packages_from`: a sibling case's), `expected.json` | `selectItems` over packages prepared from `packages/` |

`node crates/mars-schema-wasm/run-fixtures.mjs` in the Mars repository runs every case against this package and compares results deeply, key order included. Run the same cases against the version you pin; a change to a pinned reader that changes a result is a change to the contract.

## Build

`npm run build` (needs `wasm-bindgen-cli`) builds `node/`, `web/` and `fixtures/`, which are generated and not committed.
