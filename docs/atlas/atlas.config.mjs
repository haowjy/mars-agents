/*
 * The atlas of the Mars codebase. Check it:  node tools/atlas/check.mjs docs/atlas/atlas.config.mjs
 * View it:  node tools/atlas/serve.mjs docs/atlas/atlas.config.mjs   (http://localhost:4848/atlas/)
 * tools/atlas/README.md explains every key.
 */
export default {
  title: "Mars",
  root: "../..",
  repo: "haowjy/mars-agents",
  branch: "main",
  pagesDir: "docs/atlas",
  // Each directory of the Rust sources lies under some box's Paths.
  coverage: [
    { glob: "src/*" },
    { glob: "crates/*" },
    { glob: "crates/mars-schema/src/*" },
    { glob: "npm/@meridian-flow/*" },
  ],
};
