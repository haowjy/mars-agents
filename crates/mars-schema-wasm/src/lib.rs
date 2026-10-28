//! WebAssembly bindings for the pure Mars schema crate.
//!
//! Every result is serialized with `Serializer::json_compatible()`: plain objects and
//! arrays, `null` for absent values, `Uint8Array` for bytes. The shapes are the serde shapes of `mars_schema`'s types, pinned by `crates/mars-schema/fixtures`.
//!
//! Every failure is a thrown `Error` with a `code` property: the crate error's own `code()`, or
//! `input.invalid` for an argument that is not the shape the export documents.

use std::collections::{BTreeMap, HashSet};

use mars_schema::ordered::OrderedValue;
use serde::Serialize;
use serde_bytes::ByteBuf;
use serde_wasm_bindgen::Serializer;
use wasm_bindgen::prelude::*;

/// An `Error` carrying `code`, for JavaScript to tell failures apart without reading messages.
fn fail(code: &str, message: impl std::fmt::Display) -> JsValue {
    let error = js_sys::Error::new(&message.to_string());
    js_sys::Reflect::set(&error, &"code".into(), &code.into())
        .expect("a fresh Error takes properties");
    error.into()
}

/// An argument that does not deserialize as the export documents.
fn parse_input<T: serde::de::DeserializeOwned>(value: JsValue) -> Result<T, JsValue> {
    serde_wasm_bindgen::from_value(value).map_err(|error| fail("input.invalid", error))
}

/// The field names serde derives for `T`: what `deserialize_struct` is handed.
fn field_names<T: serde::de::DeserializeOwned>() -> &'static [&'static str] {
    use serde::de::{Deserializer, Error, Visitor};
    struct Probe(std::cell::Cell<&'static [&'static str]>);
    #[derive(Debug)]
    struct Stop;
    impl std::fmt::Display for Stop {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("probe")
        }
    }
    impl std::error::Error for Stop {}
    impl Error for Stop {
        fn custom<T: std::fmt::Display>(_: T) -> Self {
            Stop
        }
    }
    impl<'de> Deserializer<'de> for &Probe {
        type Error = Stop;
        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Stop> {
            Err(Stop)
        }
        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _: &'static str,
            fields: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, Stop> {
            self.0.set(fields);
            Err(Stop)
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
            byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map enum
            identifier ignored_any
        }
    }
    let probe = Probe(std::cell::Cell::new(&[]));
    let _ = T::deserialize(&probe);
    probe.0.get()
}

/// Refuses a key `T` does not have. `serde-wasm-bindgen` reads a plain object by looking up the
/// field names, so `serde`'s own `deny_unknown_fields` never sees a stray key (a misspelled
/// filter, a camelCase `extra_items`).
fn check_keys<T: serde::de::DeserializeOwned>(value: &JsValue) -> Result<(), JsValue> {
    let Some(object) = value.dyn_ref::<js_sys::Object>() else {
        return Ok(());
    };
    let fields = field_names::<T>();
    for key in js_sys::Object::keys(object).iter() {
        let key = key.as_string().unwrap_or_default();
        if !fields.contains(&key.as_str()) {
            return Err(fail(
                "input.invalid",
                format!("unknown field `{key}`, expected one of {fields:?}"),
            ));
        }
    }
    Ok(())
}

/// [`parse_input`] for a structure of named fields, refusing a key the structure does not have.
fn parse_fields<T: serde::de::DeserializeOwned>(value: JsValue) -> Result<T, JsValue> {
    check_keys::<T>(&value)?;
    parse_input(value)
}

fn to_js(value: &impl Serialize) -> Result<JsValue, JsValue> {
    value
        .serialize(&Serializer::json_compatible().serialize_bytes_as_arrays(false))
        .map_err(|error| fail("output.unserializable", error))
}

/// Errors on a JavaScript string with a lone surrogate, which would otherwise cross as U+FFFD.
/// Walks the plain arrays and objects (keys included) that `emitValue` and `rewriteBlock` take.
fn check_well_formed(value: &JsValue) -> Result<(), JsValue> {
    fn string(js: &js_sys::JsString) -> Result<(), JsValue> {
        let rust = String::from(js.clone());
        if rust.encode_utf16().eq(js.iter()) {
            Ok(())
        } else {
            Err(fail(
                "input.lone-surrogate",
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
pub fn read(path: &str, text: &str) -> Result<JsValue, JsValue> {
    let reading = mars_schema::read::read(path, text).map_err(|error| fail(error.code(), error))?;
    to_js(&reading)
}

/// YAML for the value of the key at column `indent`; see `index.d.ts`.
#[wasm_bindgen(js_name = emitValue)]
pub fn emit_value(value: JsValue, indent: u32) -> Result<String, JsValue> {
    check_well_formed(&value)?;
    let value: OrderedValue = parse_input(value)?;
    mars_schema::edit::emit_value(&value, indent as usize)
        .map_err(|error| fail(error.code(), error))
}

/// The full frontmatter block for `[key, value][]`, ending in exactly one newline.
#[wasm_bindgen(js_name = rewriteBlock)]
pub fn rewrite_block(frontmatter: JsValue) -> Result<String, JsValue> {
    check_well_formed(&frontmatter)?;
    let pairs: Vec<(String, OrderedValue)> = parse_input(frontmatter)?;
    mars_schema::edit::rewrite_block(&pairs).map_err(|error| fail(error.code(), error))
}

#[wasm_bindgen]
pub fn vocabulary() -> Result<JsValue, JsValue> {
    to_js(&mars_schema::tool_natives::vocabulary())
}

fn file_tree(files: BTreeMap<String, ByteBuf>) -> Result<mars_schema::tree::FileTree, JsValue> {
    mars_schema::tree::FileTree::from_files(
        files
            .into_iter()
            .map(|(path, bytes)| (path, bytes.into_vec())),
    )
    .map_err(|error| fail(error.code(), error))
}

/// Prepare one root from
/// `{files: Map<path, Uint8Array>, context?, declaredDialect?, migrate?}`. A supported export in
/// its own right: `normalize` is this plus a selection, a generated manifest and provenance. An
/// unrecognized `context` or `declaredDialect` throws.
#[wasm_bindgen]
pub fn prepare(argument: JsValue) -> Result<JsValue, JsValue> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        files: BTreeMap<String, ByteBuf>,
        context: Option<mars_schema::prepare::PrepareContext>,
        declared_dialect: Option<mars_schema::dialect::Dialect>,
        #[serde(default)]
        migrate: bool,
    }
    let input: Input = parse_fields(argument)?;
    let tree = file_tree(input.files)?;
    let context = input
        .context
        .unwrap_or(mars_schema::prepare::PrepareContext::Package);
    let mut prepared = if input.migrate {
        mars_schema::prepare::prepare_for_migration(&tree, context, input.declared_dialect)
    } else {
        mars_schema::prepare::prepare(&tree, context, input.declared_dialect)
    };
    // The verdict on the package's own layout; the binary leaves it to the consumer's install.
    mars_schema::links::package(&mut prepared);
    to_js(&prepared)
}

/// What a repository is: `{tree: Map<path, Uint8Array>, hint?}` to an `Inspection`. Reads
/// manifests, listings and definitions, never payload bytes.
#[wasm_bindgen]
pub fn inspect(argument: JsValue) -> Result<JsValue, JsValue> {
    #[derive(serde::Deserialize)]
    struct Input {
        tree: BTreeMap<String, ByteBuf>,
        #[serde(default)]
        hint: mars_schema::inspect::Hint,
    }
    let input: Input = parse_fields(argument)?;
    to_js(&mars_schema::inspect::inspect(
        &file_tree(input.tree)?,
        &input.hint,
    ))
}

/// Schema v1 packages for a selection: `{tree, source?, hint?, selection?}` to a `Normalized`.
/// A remote marketplace plugin is fetched by the caller and normalized as its own tree, with
/// `hint.plugin` set to the plugin's entry.
#[wasm_bindgen]
pub fn normalize(argument: JsValue) -> Result<JsValue, JsValue> {
    #[derive(serde::Deserialize)]
    struct Input {
        tree: BTreeMap<String, ByteBuf>,
        #[serde(default)]
        source: mars_schema::normalize::SourceInfo,
        #[serde(default)]
        hint: mars_schema::inspect::Hint,
        #[serde(default)]
        selection: mars_schema::normalize::Selection,
    }
    let input: Input = parse_fields(argument)?;
    let tree = file_tree(input.tree)?;
    let mut normalized =
        mars_schema::normalize::normalize(&mars_schema::normalize::NormalizeRequest {
            tree: &tree,
            source: input.source,
            hint: input.hint,
            selection: input.selection,
        });
    normalized.report_links();
    to_js(&normalized)
}

/// Whether `version` meets a dependency entry's `version` text (`null` or `"latest"` is any
/// version; a branch or commit pin is met by none). A `version` that is not semver throws.
#[wasm_bindgen]
pub fn satisfies(constraint: Option<String>, version: &str) -> Result<bool, JsValue> {
    let version =
        mars_schema::version::parse_version(version).map_err(|error| fail(error.code(), error))?;
    let constraint = mars_schema::version::parse_constraint(constraint.as_deref());
    Ok(mars_schema::version::satisfies(&constraint, &version))
}

/// Whether `version` meets a package's `requires-mars` / `requires-meridian`. A requirement that
/// is not one, or a `version` that is not semver, throws.
#[wasm_bindgen(js_name = satisfiesRequirement)]
pub fn satisfies_requirement(requirement: &str, version: &str) -> Result<bool, JsValue> {
    let version =
        mars_schema::version::parse_version(version).map_err(|error| fail(error.code(), error))?;
    mars_schema::version::satisfies_requirement(requirement, &version)
        .map_err(|error| fail(error.code(), error))
}

/// Which ref a package ends at: `{available, constraints, locked?}` to
/// `{pin} | {locked: true} | {tag} | {untagged: true}`. `available` is the repository's tags; one
/// that is not a version (`nightly`) is ignored. `constraints` is every `[requester, version |
/// null]` on the package, as the manifests spell them; `locked` is kept (`{locked: true}`, even
/// if `available` no longer lists it) when it meets every range and no constraint is `latest`,
/// else the newest allowed version is chosen. A refusal throws with a `version.*` code.
#[wasm_bindgen(js_name = selectVersion)]
pub fn select_version(argument: JsValue) -> Result<JsValue, JsValue> {
    use mars_schema::version::{Chosen, SelectionPolicy, require_tag};
    #[derive(serde::Deserialize)]
    struct Input {
        available: Vec<String>,
        constraints: Vec<(String, Option<String>)>,
        locked: Option<String>,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "lowercase")]
    enum Choice<'a> {
        Pin(&'a str),
        Locked(bool),
        Tag(&'a str),
        Untagged(bool),
    }
    let input: Input = parse_fields(argument)?;
    let parsed: Vec<_> = input
        .constraints
        .iter()
        .map(|(requester, text)| {
            (
                requester.as_str(),
                mars_schema::version::parse_constraint(text.as_deref()),
            )
        })
        .collect();
    let constraints: Vec<_> = parsed
        .iter()
        .map(|(requester, constraint)| (*requester, constraint))
        .collect();
    let locked = input
        .locked
        .as_deref()
        .map(require_tag)
        .transpose()
        .map_err(|error| fail(error.code(), error))?;
    let chosen = mars_schema::version::choose(
        &input.available,
        &constraints,
        locked.as_ref(),
        SelectionPolicy::PreferLockThenLatest,
        &HashSet::new(),
    )
    .map_err(|error| fail(error.code(), error))?;
    to_js(&match chosen {
        Chosen::Pin(reference) => Choice::Pin(reference),
        Chosen::Locked => Choice::Locked(true),
        Chosen::Tag(tag) => Choice::Tag(tag),
        Chosen::Untagged => Choice::Untagged(true),
    })
}

#[derive(serde::Deserialize)]
struct SelectItemsInput {
    #[serde(default)]
    requests: Vec<RequestInput>,
    packages: BTreeMap<String, mars_schema::selection::PackageInput>,
    root: Option<mars_schema::selection::PackageInput>,
}

#[derive(serde::Deserialize)]
struct RequestInput {
    name: String,
    agents: Option<Vec<mars_schema::names::ItemName>>,
    skills: Option<Vec<mars_schema::names::ItemName>>,
    exclude: Option<Vec<mars_schema::names::ItemName>>,
    #[serde(default)]
    only_skills: bool,
    #[serde(default)]
    only_agents: bool,
}

/// `selectItems`'s argument has keys at four depths; each is checked against its structure.
fn check_select_items_keys(argument: &JsValue) -> Result<(), JsValue> {
    use mars_schema::selection::{Item, PackageInput};
    let get = |object: &JsValue, key: &str| {
        js_sys::Reflect::get(object, &key.into()).unwrap_or(JsValue::UNDEFINED)
    };
    // Absent or not an array: `parse_input` reports it.
    let elements = |value: &JsValue| {
        if js_sys::Array::is_array(value) {
            js_sys::Array::from(value).iter().collect()
        } else {
            Vec::new()
        }
    };
    let check_package = |package: &JsValue| {
        check_keys::<PackageInput>(package)?;
        elements(&get(package, "extra_items"))
            .iter()
            .try_for_each(check_keys::<Item>)
    };
    check_keys::<SelectItemsInput>(argument)?;
    elements(&get(argument, "requests"))
        .iter()
        .try_for_each(check_keys::<RequestInput>)?;
    let packages = get(argument, "packages");
    if let Some(packages) = packages.dyn_ref::<js_sys::Object>() {
        js_sys::Object::values(packages)
            .iter()
            .try_for_each(|package| check_package(&package))?;
    }
    check_package(&get(argument, "root"))
}

/// The items a graph installs: `{requests, packages, root?}` to a `SelectionResult`. `packages` is
/// `{name: {prepared, extra_items?}}` with each `prepared` the result of `prepare` as it
/// returned it (selection reads its `selection`). `root` is the package being selected for,
/// shaped the same: it is selected whole and its manifest entries are the consumer's requests, so
/// `{root, packages}` is the whole call and `requests` is for entries the root does not hold (a
/// project's consumer configuration). A request is `{name, agents?, skills?, exclude?,
/// only_skills?, only_agents?}`; any other key, `rename` included, throws.
#[wasm_bindgen(js_name = selectItems)]
pub fn select_items(argument: JsValue) -> Result<JsValue, JsValue> {
    check_select_items_keys(&argument)?;
    let input: SelectItemsInput = parse_input(argument)?;
    let requests = input
        .requests
        .into_iter()
        .map(|request| {
            let filter = mars_schema::filter::FilterConfig {
                agents: request.agents,
                skills: request.skills,
                exclude: request.exclude,
                rename: None,
                only_skills: request.only_skills,
                only_agents: request.only_agents,
            };
            filter
                .validate(&request.name)
                .map_err(|error| fail(error.code(), error))?;
            Ok(mars_schema::selection::Request {
                name: request.name,
                filter: filter.to_mode(),
            })
        })
        .collect::<Result<_, JsValue>>()?;
    let view = mars_schema::selection::GraphView {
        requests,
        root: input.root.map(Into::into),
        packages: input
            .packages
            .into_iter()
            .map(|(name, package)| (name, package.into()))
            .collect(),
    };
    to_js(&mars_schema::selection::select(&view))
}
