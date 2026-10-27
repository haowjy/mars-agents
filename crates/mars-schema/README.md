# mars-schema

The reader and editor for [Mars](https://github.com/haowjy/mars-agents) schema v1: the file format of agent definitions (`agents/<slug>.md`), skills (`skills/<slug>/SKILL.md`) and the `[package]` table of `mars.toml`. It holds the key tables, the validation, the tool vocabulary, the pre-v1 upgrade, the foreign-layout translation and the YAML emitter. The `mars-agents` binary calls it directly, and the [`@meridian-flow/mars-schema`](https://github.com/haowjy/mars-agents/tree/main/npm/@meridian-flow/mars-schema) npm package compiles it to WebAssembly, so an editor and `mars sync` read a file the same way.

The format is documented in [Schema v1](https://github.com/haowjy/mars-agents/blob/main/docs/config/schema-v1.md). This page covers the crate.

The crate is pure: no filesystem, network, process or clock access. Callers hand it text, or a `TreeSource` they implement, and take back values. `cargo check -p mars-schema --target wasm32-unknown-unknown` stays green.

## Reading one file

```rust
use mars_schema::read::{read, Reading};

let text = std::fs::read_to_string("agents/coder.md").unwrap();
if let Reading::Agent(reading) = read("agents/coder.md", &text).unwrap() {
    for diagnostic in &reading.diagnostics {
        eprintln!("{}: {}", diagnostic.code, diagnostic.message);
    }
    if let Some(agent) = reading.fields {
        println!("{:?}", agent.effort);
    }
}
```

`read::read(path, text)` picks the reader from the package-relative path (`mars.toml`, `agents/<slug>.md` or `skills/<slug>/SKILL.md`) and returns `Err(UnrecognizedPath)` for any other path. The result is a `DefinitionRead<T>`:

| Field | Holds |
|---|---|
| `frontmatter` | every top-level key in file order, unknown keys included, as `OrderedValue` (for `mars.toml`, the `[package]` table) |
| `fields` | the typed `AgentProfile`, `SkillProfile` or `PackageMetadata`; `None` when the file cannot be read at all |
| `body` | every byte after the closing fence; `None` for `mars.toml` |
| `diagnostics` | `SchemaDiagnostic { code, severity, path, message, range }` |

The typed fields serialize with the spellings the file uses (kebab-case keys and values, `xhigh`, `read-only`); that serialization is the published shape, pinned by `fixtures/read/`. A refactor that changes it changes the contract. `agent::read`, `skill::read` and `manifest::read` are the three readers behind `read::read`, and nothing else in the workspace reads those keys. An installed `.mars/` tree is read with `read::installed_agent` and `read::installed_skill`, which upgrade a file whose strict reading reports a pre-v1 spelling, because those bytes are whatever Mars version installed them.

Diagnostics are built directly with a structured `path` of keys and indexes. `range` is set only for fence and YAML-syntax errors, and no reader sets it yet; consumers locate a diagnostic by `path`.

Unknown keys are kept and reported as `field.unknown` warnings. Removed keys are `field.removed` errors that name the replacement.

## Editing

`edit::emit_value(&OrderedValue, indent)` writes one value as YAML to splice after a key, and `edit::rewrite_block(&[(String, OrderedValue)])` writes a whole frontmatter block. Raw values are `ordered::OrderedValue`, whose mappings keep authored key order; `serde_json::Value` would sort the keys.

- One scalar rule serves keys and strings: plain only for a safe word (`[A-Za-z_][A-Za-z0-9_./-]*`, not a YAML boolean, null, number or infinity word), otherwise JSON double-quoted with every non-printable, line-separator and byte-order-mark character escaped. A multi-line string is always quoted.
- `indent` is the column of the key that owns the value. A scalar is one line with no leading newline. A non-empty collection starts with `\n` and its children sit at `indent + 2`. The output never ends in a newline.
- It refuses (`EmitError`) an integer beyond ±(2^53 − 1), NaN and Infinity, and a key written as more than 1024 characters. A float of any finite magnitude is written so that both Rust and JavaScript YAML parsers read the same value. `rewrite_block` also refuses a duplicate key and returns the block ending in exactly one newline.

## Preparing a package

`prepare::prepare(&source, context, declared_dialect)` classifies one package root and returns a `Prepared`:

- the classification (`Schema1`, `MarsLegacy`, `SchemaNewer`, `Foreign`, `Marketplace`, `Invalid`, `Empty`) and the evidence for a dialect;
- the manifest reading, normalized to schema 1 (a pre-v1 manifest gains `schema = 1` and a `requires-mars` floor of `0.20.0`);
- one entry per selected definition: kind, slug, source and output paths, normalized bytes, the typed reading and a `ChangeRecord` (`None`, `Generated`, `Edited` with the changed key paths, or `FullyRewritten`);
- payload files as source-to-output paths, never bytes;
- findings by source path (`diagnostics`) and what preparing changed in each file (`changes`: keys dropped, moved or repaired), with `Prepared::changed_files` for the count.

`TreeSource` is the read interface (`read`, `is_file`, `is_dir`, `children`) that the caller implements for a directory, an archive or memory; `FileTree` is the in-memory implementation and `SubTree` roots a tree below a directory. Preparation reads manifests, definitions and directory listings only.

`PrepareContext::Package` is a package root; `PrepareContext::AuthoredLocal` is a consumer project's `.mars-src`; `PrepareContext::Project` is a directory that is either (a `mars.toml` with no `[package]` is consumer configuration, and the tree is read as authored content). A pre-v1 package is upgraded with `upgrade::upgrade_definition` and `upgrade::upgrade_manifest`, which edit only the top-level keys that change and keep every other byte; `mars migrate` writes `Prepared`'s normalized bytes back to the authored files. Only `mars migrate` adds `requires-mars = ">=0.20.0"`, through `upgrade::ensure_requires_mars_floor`, and only when the manifest has no requirement.

`inspect::inspect(&source, &hint)` reports what a repository is (a marketplace's plugins, a plugin's or skill tree's items, the dialect and its evidence), and `normalize::normalize(&request)` runs `prepare` on each selected root of a Claude plugin, marketplace, skill tree or other foreign layout and adds a generated `mars.toml` and provenance. Neither fetches anything: the caller fetches a remote marketplace plugin and inspects or normalizes the fetched tree with `hint.plugin` set to the plugin's entry.

## Vocabulary and policy types

| Module | Holds |
|---|---|
| `tool_names` | the capability vocabulary and the `capability(pattern)` grammar; `capabilities()` lists them with their aliases |
| `tool_natives` | per-host native tool tables as data (Claude, Codex, OpenCode, Pi, Cursor, and the Flow table); `vocabulary()` returns the published whole |
| `tool_policy`, `mcp` | the tool rules of an Agent and the `mcp(server/tool)` reference grammar |
| `model_policy` | the `model-policies` rule and its validator |
| `harness` | `HarnessId` and the reserved consumer namespaces (`flow`, `meridian`) |
| `dialect` | the layouts Mars reads: the foreign ones it translates (`claude`, `agent-skills`, `codex`, `opencode`, `cursor`) and `mars-native` |

Projection onto a particular harness, overlay application, precedence and everything that touches the filesystem stay in the `mars-agents` binary.

## Fixtures

`fixtures/<suite>/<case>/` holds the corpus that fixes the behaviour: `read`, `emit`, `upgrade`, `prepare`, `normalize` and `vocabulary`. `cargo test -p mars-schema` checks every case, and the npm package's `run-fixtures.mjs` checks the same cases through WebAssembly. Regenerate expected output with `MARS_UPDATE_FIXTURES=1 cargo test -p mars-schema` and review the diff: a changed reading is a change to the format.

## License

MIT
