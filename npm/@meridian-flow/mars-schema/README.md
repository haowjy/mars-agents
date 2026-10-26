# `@meridian-flow/mars-schema`

WASM bindings for Mars schema v1: the one reader of agent definitions
(`agents/<slug>.md`), skills (`skills/<slug>/SKILL.md`) and `mars.toml`, the YAML
emitter that lossless editors splice with, and the tool vocabulary. Types are in
`index.d.ts`; the shapes are pinned by the shipped `fixtures/`.

```ts
import { read, emitValue, rewriteBlock, vocabulary } from "@meridian-flow/mars-schema";

const reading = read("agents/coder.md", text); // { frontmatter, fields, body, diagnostics }
```

The package selects the Node or browser build through conditional exports. The Node
build is synchronous. The web build is an ES module and must be instantiated first:

```ts
import init, { read } from "@meridian-flow/mars-schema"; // browser condition
await init();
```

`init` is typed only under the `browser` export condition (`index.web.d.ts`); with TypeScript's
`bundler` resolution, set `customConditions: ["browser"]` for the web build.

`inspect` and `normalize` read any repository as Mars packages: `inspect` reports what is there
(a marketplace's plugins, a plugin's or skill tree's items, the dialect and the evidence for it),
and `normalize` turns a selection into schema v1 packages with a generated `mars.toml` and
provenance. A remote plugin is fetched by the caller and inspected on its own. `prepare` is the
single-root core both build on.

Build with `npm run build` (needs `wasm-bindgen-cli`); `node/`, `web/` and `fixtures/` are
generated and not committed.
