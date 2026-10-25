//! WebAssembly bindings for the pure Mars schema crate.

use wasm_bindgen::prelude::*;

/// Returns the Mars schema package version.
#[wasm_bindgen(js_name = version)]
pub fn version() -> String {
    mars_schema::VERSION.to_owned()
}
