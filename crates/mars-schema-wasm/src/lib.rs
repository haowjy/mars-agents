//! WebAssembly bindings for the pure Mars schema crate.

use wasm_bindgen::prelude::*;

fn from_js(value: JsValue) -> Result<serde_json::Value, JsValue> {
    serde_wasm_bindgen::from_value(value).map_err(|e| JsValue::from_str(&e.to_string()))
}

fn to_js(value: impl serde::Serialize) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(&value).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Returns the Mars schema package version.
#[wasm_bindgen(js_name = version)]
pub fn version() -> String {
    mars_schema::VERSION.to_owned()
}

/// Read one definition. `kind` is `agent` or `skill`; `text` is UTF-8 markdown.
#[wasm_bindgen]
pub fn read(input: JsValue) -> Result<JsValue, JsValue> {
    let input: serde_json::Value = from_js(input)?;
    let kind = input.get("kind").and_then(|v| v.as_str()).unwrap_or("agent");
    let text = input.get("text").and_then(|v| v.as_str()).ok_or_else(|| JsValue::from_str("read requires text"))?;
    match kind {
        "agent" => to_js(mars_schema::agent::read(text)),
        "skill" => to_js(mars_schema::skill::read(text)),
        _ => Err(JsValue::from_str("read kind must be agent or skill")),
    }
}

#[wasm_bindgen(js_name = emitValue)]
pub fn emit_value(value: JsValue, indent: u32) -> Result<String, JsValue> {
    Ok(mars_schema::edit::emit_value(&from_js(value)?, indent as usize))
}

#[wasm_bindgen(js_name = rewriteBlock)]
pub fn rewrite_block(value: JsValue) -> Result<String, JsValue> {
    let values: Vec<(String, serde_json::Value)> = serde_wasm_bindgen::from_value(value)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(mars_schema::edit::rewrite_block(&values))
}

#[wasm_bindgen]
pub fn vocabulary() -> Result<JsValue, JsValue> {
    to_js(mars_schema::tool_natives::vocabulary())
}

/// Prepare a package from `{files: {path: Uint8Array}, context, declaredDialect}`.
#[wasm_bindgen]
pub fn prepare(input: JsValue) -> Result<JsValue, JsValue> {
    let input: serde_json::Value = from_js(input)?;
    let files = input.get("files").cloned().ok_or_else(|| JsValue::from_str("prepare requires files"))?;
    let files: std::collections::BTreeMap<String, Vec<u8>> = serde_wasm_bindgen::from_value(
        serde_wasm_bindgen::to_value(&files).map_err(|e| JsValue::from_str(&e.to_string()))?
    ).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let tree = mars_schema::tree::FileTree::from_files(files).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let context = match input.get("context").and_then(|v| v.as_str()).unwrap_or("package") {
        "authored-local" => mars_schema::prepare::PrepareContext::AuthoredLocal,
        _ => mars_schema::prepare::PrepareContext::Package,
    };
    let prepared = mars_schema::prepare::prepare(&tree, context, None);
    let mut value = serde_json::to_value(&prepared).map_err(|e| JsValue::from_str(&e.to_string()))?;
    if let Some(manifest) = value.get_mut("manifest") {
        if let Some(source) = prepared.manifest.as_ref() {
            manifest["normalized_bytes"] = serde_json::json!(source.normalized_bytes);
        }
    }
    if let Some(definitions) = value.get_mut("definitions").and_then(|v| v.as_array_mut()) {
        for (output, definition) in definitions.iter_mut().zip(prepared.definitions.iter()) {
            output["normalized_bytes"] = serde_json::json!(definition.normalized_bytes);
        }
    }
    to_js(value)
}
