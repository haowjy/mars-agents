# Mars

Mars installs agents, skills, hooks and MCP servers from git and local sources into each harness's own directories, through a canonical store in `.mars/`.

```mermaid
flowchart LR
  cli["cli<br/><small>one module per mars command</small>"]:::module
  sync["sync<br/><small>the phases of one run, in order</small>"]:::module
  sources["sources<br/><small>fetch, resolve, read each package</small>"]:::module
  schema["mars-schema<br/><small>the package format, parsed once for Rust and JS</small>"]:::package
  state["config and lock<br/><small>what the user asked for, what Mars owns</small>"]:::module
  compiler["compiler<br/><small>lowers agents, skills, hooks, MCP per harness</small>"]:::module
  targets["targets<br/><small>copies into .claude, .codex and the rest</small>"]:::module
  models["models<br/><small>catalog, probes, which harness runs which model</small>"]:::module
  build["build<br/><small>plugin trees and launch bundles</small>"]:::module
  base["foundation<br/><small>diagnostics, errors, atomic writes, repo tooling</small>"]:::module
  cli -->|"1. builds a request for"| sync
  sync -->|"2. loads and resolves through"| sources
  sources -->|"3. reads packages with"| schema
  sync <-->|"reads and writes"| state
  sync -->|"4. hands the resolved graph to"| compiler
  compiler -->|"5. writes outputs through"| targets
  compiler -->|"lowers model names with"| models
  cli -->|"model and build commands"| models
  cli -->|"mars build"| build
  build -->|"picks a harness with"| models
  build -->|"publishes as a consumer through"| sources
  compiler -.->|"reports through"| base
  targets -.->|"writes atomically with"| base
```

## cli

One clap module per `mars` subcommand; it builds a request and prints the result.

- Kind: module
- Opens: [cli](cli.md)
- Docs: [commands](../cli/commands.md)

## sync

The phases of one run and what each hands the next.

- Kind: module
- Opens: [sync](sync.md)
- Docs: [sync pipeline](../internals/sync-pipeline.md)

## sources

Fetches git and path sources, picks versions, and reads each package into a prepared form.

- Kind: module
- Opens: [sources](sources.md)

## schema

The `mars-schema` crate: package vocabulary, parsing, validation and the resolver core, with no file or network access, so the CLI and the npm WASM package read the same way.

- Kind: package
- Opens: [schema](schema.md)
- Docs: [schema v1](../config/schema-v1.md)

## state

`mars.toml`, `mars.local.toml`, the user config and `mars.lock`: what was asked for and what Mars installed and owns.

- Kind: module
- Opens: [config and lock](config-lock.md)
- Docs: [lock file](../internals/lock-file.md), [mars.toml](../config/mars-toml.md)

## compiler

Turns the selected packages into what each harness reads: native agent files, skills, hook and MCP config entries.

- Kind: module
- Opens: [compiler](compiler.md)
- Docs: [agent compilation](../config/agent-compilation.md), [MCP and hooks](../config/mcp-and-hooks.md)

## targets

Per-harness adapters and the copy from `.mars/` into each linked target, gated by who owns each path.

- Kind: module
- Opens: [targets](targets.md)

## models

The models.dev catalog, aliases, harness probes and the routing that chooses a harness for a model.

- Kind: module
- Opens: [models](models.md)

## build

`mars build plugin` and `mars build launch-bundle`: outputs built from a package without a sync into a project.

- Kind: module
- Opens: [build](build.md)
- Docs: [plugin authoring](../plugin-authoring.md)

## base

Shared plumbing every other part uses, plus the scripts, tests and CI around the code.

- Kind: module
- Opens: [foundation](foundation.md)
