import { createRequire } from "node:module";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
const require = createRequire(import.meta.url);
const wasm = require("../../npm/@meridian-flow/mars-schema/node/mars_schema_wasm.js");
const root = new URL("../mars-schema/fixtures/", import.meta.url);
const readDir = name => readdirSync(new URL(name, root), { withFileTypes: true }).filter(e => e.isDirectory());
for (const dir of readDir("read")) {
  const path = new URL(`read/${dir.name}/`, root);
  const file = dir.name.startsWith("skill") ? "skill.md" : dir.name.startsWith("manifest") ? "mars.toml" : "agent.md";
  const kind = file === "skill.md" ? "skill" : "agent";
  wasm.read({ kind, text: readFileSync(new URL(file, path), "utf8") });
}
for (const dir of readDir("emit")) {
  const input = JSON.parse(readFileSync(new URL(`emit/${dir.name}/value.json`, root), "utf8"));
  const actual = wasm.emitValue(input.value, input.indent);
  const expected = readFileSync(new URL(`emit/${dir.name}/expected.txt`, root), "utf8").trimEnd();
  if (actual !== expected) throw new Error(`emit fixture ${dir.name}: ${JSON.stringify(actual)} != ${JSON.stringify(expected)}`);
}
if (!wasm.vocabulary().get("capabilities").some(entry => entry.get("name") === "work")) throw new Error("vocabulary missing work");
console.log("mars-schema-wasm fixtures passed");
