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

/// Errors on a JavaScript string with a lone surrogate, which would otherwise cross as U+FFFD.
/// Walks the plain arrays and objects (keys included) that `emitValue` and `rewriteBlock` take.
fn check_well_formed(value: &JsValue) -> Result<(), JsError> {
    fn string(js: &js_sys::JsString) -> Result<(), JsError> {
        let rust = String::from(js.clone());
        if rust.encode_utf16().eq(js.iter()) {
            Ok(())
        } else {
            Err(JsError::new(
                "string contains a lone surrogate, which has no UTF-8 form",
            ))
        }
    }
    if let Some(text) = value.dyn_ref::<js_sys::JsString>() {
        string(text)
    } else if js_sys::Array::is_array(value) {
        js_sys::Array::from(value)
            .iter()
            .try_for_each(|item| check_well_formed(&item))
    } else if let Some(object) = value.dyn_ref::<js_sys::Object>() {
        for key in js_sys::Object::keys(object).iter() {
            if let Some(key) = key.dyn_ref::<js_sys::JsString>() {
                string(key)?;
            }
            if let Ok(item) = js_sys::Reflect::get(object, &key) {
                check_well_formed(&item)?;
            }
        }
        Ok(())
    } else {
        Ok(())
    }
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
    check_well_formed(&value)?;
    let value: OrderedValue = serde_wasm_bindgen::from_value(value)?;
    Ok(mars_schema::edit::emit_value(&value, indent as usize)?)
}

/// The full frontmatter block for `[key, value][]`, ending in exactly one newline.
#[wasm_bindgen(js_name = rewriteBlock)]
pub fn rewrite_block(frontmatter: JsValue) -> Result<String, JsError> {
    check_well_formed(&frontmatter)?;
    let pairs: Vec<(String, OrderedValue)> = serde_wasm_bindgen::from_value(frontmatter)?;
    Ok(mars_schema::edit::rewrite_block(&pairs)?)
}

#[wasm_bindgen]
pub fn vocabulary() -> Result<JsValue, JsError> {
    to_js(&mars_schema::tool_natives::vocabulary())
}

fn file_tree(files: BTreeMap<String, ByteBuf>) -> Result<mars_schema::tree::FileTree, JsError> {
    Ok(mars_schema::tree::FileTree::from_files(
        files
            .into_iter()
            .map(|(path, bytes)| (path, bytes.into_vec())),
    )?)
}

/// Prepare one root from `{files: Map<path, Uint8Array>, context?, declaredDialect?}`. A supported
/// export in its own right: `normalize` is this plus a selection, a generated manifest and
/// provenance. An unrecognized `context` or `declaredDialect` throws.
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
    let tree = file_tree(input.files)?;
    let context = input
        .context
        .unwrap_or(mars_schema::prepare::PrepareContext::Package);
    to_js(&mars_schema::prepare::prepare(
        &tree,
        context,
        input.declared_dialect,
    ))
}

/// What a repository is: `{tree: Map<path, Uint8Array>, hint?}` to an `Inspection`. Reads
/// manifests, listings and definitions, never payload bytes.
#[wasm_bindgen]
pub fn inspect(input: JsValue) -> Result<JsValue, JsError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        tree: BTreeMap<String, ByteBuf>,
        #[serde(default)]
        hint: mars_schema::inspect::Hint,
    }
    let input: Input = serde_wasm_bindgen::from_value(input)?;
    to_js(&mars_schema::inspect::inspect(
        &file_tree(input.tree)?,
        &input.hint,
    ))
}

/// Schema v1 packages for a selection: `{tree, source?, hint?, selection?}` to a `Normalized`.
/// A remote marketplace plugin is fetched by the caller and normalized as its own tree, with
/// `hint.plugin` set to the plugin's entry.
#[wasm_bindgen]
pub fn normalize(input: JsValue) -> Result<JsValue, JsError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        tree: BTreeMap<String, ByteBuf>,
        #[serde(default)]
        source: mars_schema::normalize::SourceInfo,
        #[serde(default)]
        hint: mars_schema::inspect::Hint,
        #[serde(default)]
        selection: mars_schema::normalize::Selection,
    }
    let input: Input = serde_wasm_bindgen::from_value(input)?;
    let tree = file_tree(input.tree)?;
    to_js(&mars_schema::normalize::normalize(
        &mars_schema::normalize::NormalizeRequest {
            tree: &tree,
            source: input.source,
            hint: input.hint,
            selection: input.selection,
        },
    ))
}
