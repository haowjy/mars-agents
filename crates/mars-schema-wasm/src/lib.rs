//! WebAssembly bindings for the pure Mars schema crate.
//!
//! Every result is serialized with `Serializer::json_compatible()`: plain objects and
//! arrays, `null` for absent values, `Uint8Array` for bytes. The shapes are the serde
//! shapes of `mars_schema`'s types, pinned by `crates/mars-schema/fixtures`.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_bytes::ByteBuf;
use serde_wasm_bindgen::Serializer;
use wasm_bindgen::prelude::*;

fn to_js(value: &impl Serialize) -> Result<JsValue, JsError> {
    Ok(value.serialize(&Serializer::json_compatible())?)
}

/// A JSON value read from JavaScript. Unlike `serde_json::Value`, it refuses NaN and
/// Infinity instead of turning them into `null`.
struct Json(serde_json::Value);

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Json, E> {
        Ok(Json(v.into()))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Json, E> {
        Ok(Json(v.into()))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Json, E> {
        Ok(Json(v.into()))
    }
    fn visit_i128<E: de::Error>(self, v: i128) -> Result<Json, E> {
        i64::try_from(v)
            .map(|v| Json(v.into()))
            .map_err(|_| E::custom("integer is beyond ±2^53"))
    }
    fn visit_u128<E: de::Error>(self, v: u128) -> Result<Json, E> {
        u64::try_from(v)
            .map(|v| Json(v.into()))
            .map_err(|_| E::custom("integer is beyond ±2^53"))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Json, E> {
        serde_json::Number::from_f64(v)
            .map(|n| Json(n.into()))
            .ok_or_else(|| E::custom("NaN and Infinity cannot be written as YAML"))
    }
    fn visit_str<E>(self, v: &str) -> Result<Json, E> {
        Ok(Json(v.into()))
    }
    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json(serde_json::Value::Null))
    }
    fn visit_none<E>(self) -> Result<Json, E> {
        Ok(Json(serde_json::Value::Null))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut out = Vec::new();
        while let Some(Json(value)) = seq.next_element()? {
            out.push(value);
        }
        Ok(Json(out.into()))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut out = serde_json::Map::new();
        while let Some((key, Json(value))) = map.next_entry::<String, Json>()? {
            out.insert(key, value);
        }
        Ok(Json(out.into()))
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
    let Json(value) = serde_wasm_bindgen::from_value(value)?;
    Ok(mars_schema::edit::emit_value(&value, indent as usize)?)
}

/// The full frontmatter block for `[key, value][]`, ending in exactly one newline.
#[wasm_bindgen(js_name = rewriteBlock)]
pub fn rewrite_block(frontmatter: JsValue) -> Result<String, JsError> {
    let pairs: Vec<(String, Json)> = serde_wasm_bindgen::from_value(frontmatter)?;
    let pairs: Vec<_> = pairs.into_iter().map(|(k, Json(v))| (k, v)).collect();
    Ok(mars_schema::edit::rewrite_block(&pairs)?)
}

#[wasm_bindgen]
pub fn vocabulary() -> Result<JsValue, JsError> {
    to_js(&mars_schema::tool_natives::vocabulary())
}

/// Prepare a package from `{files: Map<path, Uint8Array>, context?}`. Provisional: P5
/// replaces this surface with `inspect` and `normalize`.
#[wasm_bindgen]
pub fn prepare(input: JsValue) -> Result<JsValue, JsError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        files: BTreeMap<String, ByteBuf>,
        context: Option<mars_schema::prepare::PrepareContext>,
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
    to_js(&mars_schema::prepare::prepare(&tree, context, None))
}
