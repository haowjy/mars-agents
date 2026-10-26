//! WebAssembly bindings for the pure Mars schema crate.
//!
//! Every result is serialized with `Serializer::json_compatible()`: plain objects and
//! arrays, `null` for absent values, `Uint8Array` for bytes. The shapes are the serde shapes of `mars_schema`'s types, pinned by `crates/mars-schema/fixtures`.

use std::collections::BTreeMap;

use mars_schema::ordered::OrderedValue;
use serde::Serialize;
use serde_bytes::ByteBuf;
use serde_wasm_bindgen::Serializer;
use wasm_bindgen::prelude::*;

fn to_js(value: &impl Serialize) -> Result<JsValue, JsError> {
    Ok(value.serialize(&Serializer::json_compatible().serialize_bytes_as_arrays(false))?)
}

/// Returns the Mars schema package version.
#[wasm_bindgen(js_name = version)]
pub fn version() -> String {
    mars_schema::VERSION.to_owned()
}

/// Read one schema file. `path` is package-relative and picks the reader: `mars.toml`,
/// `agents/<slug>.md` or `skills/<slug>/SKILL.md`. Any other path throws.
#[wasm_bindgen]
pub fn read(path: &str, text: &str) -> Result<JsValue, JsError> {
    let reading = mars_schema::read::read(path, text)?;
    to_js(&reading)
}

/// YAML for the value of the key at column `indent`; see `index.d.ts`.
#[wasm_bindgen(js_name = emitValue)]
pub fn emit_value(value: JsValue, indent: u32) -> Result<String, JsError> {
    let value: OrderedValue = serde_wasm_bindgen::from_value(value)?;
    Ok(mars_schema::edit::emit_value(&value, indent as usize)?)
}

/// The full frontmatter block for `[key, value][]`, ending in exactly one newline.
#[wasm_bindgen(js_name = rewriteBlock)]
pub fn rewrite_block(frontmatter: JsValue) -> Result<String, JsError> {
    let pairs: Vec<(String, OrderedValue)> = serde_wasm_bindgen::from_value(frontmatter)?;
    Ok(mars_schema::edit::rewrite_block(&pairs)?)
}

#[wasm_bindgen]
pub fn vocabulary() -> Result<JsValue, JsError> {
    to_js(&mars_schema::tool_natives::vocabulary())
}

/// Prepare a package from `{files: Map<path, Uint8Array>, context?, declaredDialect?}`.
/// An unrecognized `context` or `declaredDialect` throws. Provisional: P5 replaces this
/// surface with `inspect` and `normalize`.
#[wasm_bindgen]
pub fn prepare(input: JsValue) -> Result<JsValue, JsError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct Input {
        files: BTreeMap<String, ByteBuf>,
        context: Option<mars_schema::prepare::PrepareContext>,
        declared_dialect: Option<mars_schema::dialect::Dialect>,
    }
    let input: Input = serde_wasm_bindgen::from_value(input)?;
    let tree = mars_schema::tree::FileTree::from_files(
        input
            .files
            .into_iter()
            .map(|(path, bytes)| (path, bytes.into_vec())),
    )?;
    let context = input
        .context
        .unwrap_or(mars_schema::prepare::PrepareContext::Package);
    to_js(&mars_schema::prepare::prepare(
        &tree,
        context,
        input.declared_dialect,
    ))
}
