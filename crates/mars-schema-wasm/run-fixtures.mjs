// Runs the published fixtures (crates/mars-schema/fixtures) against the built WASM package
// and deep-compares every result. Build first: scripts/build-schema-wasm.sh, then
// `npm install` in npm/@meridian-flow/mars-schema for the `yaml` library.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const pkg = fileURLToPath(new URL("../../npm/@meridian-flow/mars-schema/", import.meta.url));
const require = createRequire(import.meta.url);
const wasm = require(join(pkg, "node/mars_schema_wasm.js"));
const YAML = require(join(pkg, "node_modules/yaml"));
const fixtures = fileURLToPath(new URL("../mars-schema/fixtures/", import.meta.url));

const cases = suite => readdirSync(join(fixtures, suite), { withFileTypes: true })
  .filter(entry => entry.isDirectory()).map(entry => entry.name).sort();
const text = (...parts) => readFileSync(join(fixtures, ...parts), "utf8");
const json = (...parts) => JSON.parse(text(...parts));
const tree = (dir, prefix = "") => {
  const files = new Map();
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = `${prefix}${entry.name}`;
    if (entry.isDirectory()) for (const [k, v] of tree(join(dir, entry.name), `${path}/`)) files.set(k, v);
    else files.set(path, new Uint8Array(readFileSync(join(dir, entry.name))));
  }
  return files;
};
let checked = 0;
const check = (actual, expected, label) => {
  assert.deepStrictEqual(actual, expected, label);
  checked++;
};

// read
for (const name of cases("read")) {
  const [file, path] = existsSync(join(fixtures, "read", name, "agent.md")) ? ["agent.md", "agents/agent.md"]
    : existsSync(join(fixtures, "read", name, "skill.md")) ? ["skill.md", "skills/skill/SKILL.md"]
    : ["mars.toml", "mars.toml"];
  check(wasm.read(path, text("read", name, file)), json("read", name, "expected.json"), `read/${name}`);
}
assert.throws(() => wasm.read("README.md", ""), /not a schema file/);

// prepare: result equals expected.json once the bytes are checked against expected/ or the input
const stripBytes = value => {
  if (Array.isArray(value)) return value.forEach(stripBytes);
  if (value && typeof value === "object") {
    delete value.normalized_bytes;
    Object.values(value).forEach(stripBytes);
  }
};
const checkBytes = (item, input, expectedFiles, label) => {
  assert.ok(item.normalized_bytes instanceof Uint8Array, `${label} bytes are a Uint8Array`);
  const want = item.change.kind === "none" ? input.get(item.source_path) : expectedFiles.get(item.output_path);
  assert.deepStrictEqual(Buffer.from(item.normalized_bytes), Buffer.from(want ?? []), `${label} ${item.output_path} bytes`);
};
for (const name of cases("prepare")) {
  const request = json("prepare", name, "request.json");
  const input = tree(join(fixtures, "prepare", name, "input"));
  const result = wasm.prepare({ files: input, context: request.context, declaredDialect: request.declared_dialect ?? undefined });
  const expectedDir = join(fixtures, "prepare", name, "expected");
  const expectedFiles = existsSync(expectedDir) ? tree(expectedDir) : new Map();
  for (const item of [result.manifest, ...result.definitions].filter(Boolean)) checkBytes(item, input, expectedFiles, `prepare/${name}`);
  stripBytes(result);
  check(result, json("prepare", name, "expected.json"), `prepare/${name}`);
}
assert.throws(() => wasm.prepare({ files: new Map(), context: "nonsense" }));

// upgrade: the legacy file in a legacy package comes back as expected.*, with the expected diagnostics
const legacyManifest = '[package]\nname = "legacy"\nversion = "1.0.0"\n';
const enc = new TextEncoder();
for (const name of cases("upgrade")) {
  const kind = text("upgrade", name, "kind.txt").trim();
  if (kind === "payload") continue;
  const ext = kind === "manifest" ? "toml" : "md";
  const path = { manifest: "mars.toml", agent: "agents/probe.md", skill: "skills/probe/SKILL.md" }[kind];
  const files = new Map([["mars.toml", enc.encode(kind === "manifest" ? text("upgrade", name, `input.${ext}`) : legacyManifest)]]);
  if (kind !== "manifest") files.set(path, enc.encode(text("upgrade", name, `input.${ext}`)));
  const result = wasm.prepare({ files, context: "package" });
  const item = kind === "manifest" ? result.manifest : result.definitions.find(d => d.source_path === path);
  assert.equal(Buffer.from(item.normalized_bytes).toString("utf8"), text("upgrade", name, `expected.${ext}`), `upgrade/${name} text`);
  assert.equal(item.change.kind === "fully-rewritten", existsSync(join(fixtures, "upgrade", name, "fully-rewritten.txt")), `upgrade/${name} rewrite mode`);
  const messages = (result.diagnostics[path] ?? []).filter(d => d.code !== "legacy-package").map(d => d.message);
  const keys = json("upgrade", name, "expected-diagnostics.json");
  assert.equal(messages.length, keys.length, `upgrade/${name} diagnostics ${JSON.stringify(messages)}`);
  for (const key of keys) assert.ok(messages.some(m => m.includes(`\`${key}\``)), `upgrade/${name} names ${key}`);
  checked++;
}

// vocabulary
check(wasm.vocabulary(), json("vocabulary", "expected.json"), "vocabulary");

// emit: byte-exact, then spliced into a real frontmatter and read back by Mars and by `yaml`
const splice = (emitted, indent) => {
  let out = "---\nname: probe\n";
  for (let level = 0; level <= indent / 2; level++) {
    out += `${" ".repeat(level * 2)}${level === 0 ? "x-test" : "k"}:`;
    if (level * 2 === indent) out += (emitted.startsWith("\n") ? "" : " ") + emitted;
    out += "\n";
  }
  return `${out}---\nbody\n`;
};
const nest = (value, indent) => {
  for (let i = 0; i < indent / 2; i++) value = { k: value };
  return value;
};
for (const name of cases("emit")) {
  const { value, indent } = json("emit", name, "value.json");
  if (existsSync(join(fixtures, "emit", name, "expected-error.txt"))) {
    assert.throws(() => wasm.emitValue(value, indent), (e) => e.message.includes(text("emit", name, "expected-error.txt").trim()), `emit/${name}`);
    checked++;
    continue;
  }
  const emitted = wasm.emitValue(value, indent);
  assert.equal(emitted, text("emit", name, "expected.txt"), `emit/${name} bytes`);
  const doc = splice(emitted, indent);
  const reading = wasm.read("agents/probe.md", doc);
  assert.deepStrictEqual(reading.diagnostics.filter(d => d.code.startsWith("yaml.")), [], `emit/${name} yaml diagnostics`);
  const marsValue = reading.frontmatter.find(([key]) => key === "x-test")?.[1];
  assert.deepStrictEqual(marsValue, nest(value, indent), `emit/${name} read back by Mars`);
  const front = doc.split("---\n")[1];
  assert.deepStrictEqual(YAML.parse(front)["x-test"], nest(value, indent), `emit/${name} read back by yaml`);
  checked++;
}

// rewriteBlock: read → rewrite → read keeps every value
const source = `---\nname: a\ndescription: "x\\ny\\n"\ntools:\n  "*": deny\n  read: allow\nx-keys:\n  "#k": 1\n---\nbody\n`;
const first = wasm.read("agents/a.md", source);
const block = wasm.rewriteBlock(first.frontmatter);
assert.ok(block.endsWith("\n") && !block.endsWith("\n\n"));
check(wasm.read("agents/a.md", `---\n${block}---\nbody\n`).frontmatter, first.frontmatter, "rewriteBlock round trip");
assert.equal(wasm.rewriteBlock([]), "");
assert.throws(() => wasm.rewriteBlock([["a", 1], ["a", 2]]), /duplicate/);
assert.throws(() => wasm.emitValue(NaN, 0), /NaN/);

console.log(`mars-schema-wasm fixtures passed (${checked} checks)`);
