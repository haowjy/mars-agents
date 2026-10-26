#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
pkg="$root/npm/@meridian-flow/mars-schema"
cargo build --manifest-path "$root/Cargo.toml" -p mars-schema-wasm --release --target wasm32-unknown-unknown
rm -rf "$pkg/node" "$pkg/web"
wasm-bindgen "$root/target/wasm32-unknown-unknown/release/mars_schema_wasm.wasm" --target nodejs --out-dir "$pkg/node" --typescript
wasm-bindgen "$root/target/wasm32-unknown-unknown/release/mars_schema_wasm.wasm" --target web --out-dir "$pkg/web" --typescript
rm -rf "$pkg/fixtures"
cp -R "$root/crates/mars-schema/fixtures" "$pkg/fixtures"
