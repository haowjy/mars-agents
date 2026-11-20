# Mars atlas

A map of the Mars codebase for someone who has not read the Rust: what it is made of, how a
`mars sync` flows through it, and which module to open to change a given thing. Click a box to go down a level; each box's
section names its files and what to know before changing them.

## The pages

- [Mars](mars.md): the ten parts, and how a command reaches them.
- [A sync, step by step](sync-walk.md): `mars sync` from the command line to the written lock.

## Where to change a thing

| To change | Open |
|---|---|
| a `mars` subcommand, its flags or output | [cli](cli.md) |
| what a key in `mars.toml`, an agent or a skill means, or whether it parses | [schema](schema.md) (the `mars-schema` crate, which the WASM package shares) |
| which version of a source is picked | [schema → resolve](schema.md#resolve), then [sources](sources.md#resolve) |
| the order of a sync, or what a sync does to the canonical store | [sync](sync.md) |
| what an agent, skill, hook or MCP server becomes in `.claude/`, `.codex/` and so on | [compiler](compiler.md) |
| how a file is copied to, kept in or removed from a target | [targets](targets.md) |
| what `mars.lock` records, or who owns an output | [config and lock](config-lock.md#lock) |
| model aliases, the models.dev catalog, probes, harness choice | [models](models.md) |
| `mars build plugin` or `mars build launch-bundle` | [build](build.md) |
| a warning, an error code, a write that must be atomic | [foundation](foundation.md) |

## Editing

A change that adds, moves or removes something a page names updates the page in the same commit.
`node tools/atlas/check.mjs docs/atlas/atlas.config.mjs` fails on a path that no longer exists, a
directory of `src/` or `crates/` no box covers, and a link that is gone. It does not check that a
sentence is still true: read the page you are touching. The pages cover `main`; the harness
registry (`src/harness/`), model probes (`src/models/probes/`) and the target adapters
(`src/target/`) are the pages most likely to need an update when a harness is added or removed, and each lists its
files in one place for that.

Serve it with `node tools/atlas/serve.mjs docs/atlas/atlas.config.mjs`.
