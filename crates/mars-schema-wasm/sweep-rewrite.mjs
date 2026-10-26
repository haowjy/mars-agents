// Real-file check for rewriteBlock: for every agents/*.md and skills/*/SKILL.md under the
// given directories, read → rewriteBlock → read must leave the frontmatter unchanged.
//   node sweep-rewrite.mjs <dir>...
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const pkg = fileURLToPath(new URL("../../npm/@meridian-flow/mars-schema/", import.meta.url));
const wasm = createRequire(import.meta.url)(join(pkg, "node/mars_schema_wasm.js"));

function* walk(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* walk(path);
    else yield path;
  }
}
const schemaPath = path => {
  const parts = path.split("/");
  const at = parts.lastIndexOf("agents");
  if (at >= 0 && at === parts.length - 2 && path.endsWith(".md")) return `agents/${parts.at(-1)}`;
  if (parts.at(-1) === "SKILL.md" && parts.at(-3) === "skills") return `skills/${parts.at(-2)}/SKILL.md`;
  return null;
};

let files = 0, unreadable = 0;
for (const root of process.argv.slice(2)) {
  for (const file of walk(root)) {
    const path = schemaPath(file);
    if (!path) continue;
    const before = wasm.read(path, readFileSync(file, "utf8"));
    if (before.diagnostics.some(d => d.code.startsWith("yaml.") || d.code.startsWith("frontmatter."))) {
      unreadable++;
      continue;
    }
    const block = wasm.rewriteBlock(before.frontmatter);
    const after = wasm.read(path, `---\n${block}---\n${before.body}`);
    assert.deepStrictEqual(after.frontmatter, before.frontmatter, relative(root, file));
    files++;
  }
}
console.log(`rewriteBlock sweep: ${files} files, 0 value changes (${unreadable} skipped as unreadable YAML)`);
