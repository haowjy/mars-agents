#!/usr/bin/env bash
# Build the @meridian-flow/mars-schema package: Node and web bindings plus the shared fixtures.
# Outputs are replaced only after wasm-bindgen succeeds for both targets.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
pkg="$root/npm/@meridian-flow/mars-schema"
target_dir="$(realpath -m "${CARGO_TARGET_DIR:-$root/target}")"
wasm="$target_dir/wasm32-unknown-unknown/release/mars_schema_wasm.wasm"

cargo build --manifest-path "$root/Cargo.toml" -p mars-schema-wasm --release --target wasm32-unknown-unknown

stage="$(mktemp -d "$pkg/.build.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
wasm-bindgen "$wasm" --target nodejs --out-dir "$stage/node" --typescript
wasm-bindgen "$wasm" --target web --out-dir "$stage/web" --typescript
# The package is CommonJS; the web build is an ES module.
printf '{ "type": "module" }\n' > "$stage/web/package.json"
cp -R "$root/crates/mars-schema/fixtures" "$stage/fixtures"

rm -rf "$pkg/node" "$pkg/web" "$pkg/fixtures"
mv "$stage/node" "$stage/web" "$stage/fixtures" "$pkg/"
