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

/// Prepare one root from
/// `{files: Map<path, Uint8Array>, context?, declaredDialect?, migrate?}`. A supported export in
/// its own right: `normalize` is this plus a selection, a generated manifest and provenance. An
/// unrecognized `context` or `declaredDialect` throws.
#[wasm_bindgen]
pub fn prepare(input: JsValue) -> Result<JsValue, JsError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct Input {
        files: BTreeMap<String, ByteBuf>,
        context: Option<mars_schema::prepare::PrepareContext>,
        declared_dialect: Option<mars_schema::dialect::Dialect>,
        #[serde(default)]
        migrate: bool,
    }
    let input: Input = serde_wasm_bindgen::from_value(input)?;
    let tree = file_tree(input.files)?;
    let context = input
        .context
        .unwrap_or(mars_schema::prepare::PrepareContext::Package);
    let prepared = if input.migrate {
        mars_schema::prepare::prepare_for_migration(&tree, context, input.declared_dialect)
    } else {
        mars_schema::prepare::prepare(&tree, context, input.declared_dialect)
    };
    to_js(&prepared)
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

fn parse_version(text: &str) -> Result<semver::Version, JsError> {
    semver::Version::parse(text).map_err(|error| JsError::new(&format!("`{text}`: {error}")))
}

fn parse_tags(tags: &[String]) -> Result<Vec<semver::Version>, JsError> {
    tags.iter()
        .map(|tag| {
            mars_schema::version::parse_tag(tag)
                .ok_or_else(|| JsError::new(&format!("`{tag}` is not a version tag")))
        })
        .collect()
}

/// Whether `version` meets a dependency entry's `version` text (`null` or `"latest"` is any
/// version; a branch or commit pin is met by none). A `version` that is not semver throws.
#[wasm_bindgen]
pub fn satisfies(constraint: Option<String>, version: &str) -> Result<bool, JsError> {
    let constraint = mars_schema::version::parse_constraint(constraint.as_deref());
    Ok(mars_schema::version::satisfies(
        &constraint,
        &parse_version(version)?,
    ))
}

/// Whether `version` meets a package's `requires-mars` / `requires-meridian`. A requirement that
/// is not one, or a `version` that is not semver, throws.
#[wasm_bindgen(js_name = satisfiesRequirement)]
pub fn satisfies_requirement(requirement: &str, version: &str) -> Result<bool, JsError> {
    Ok(mars_schema::version::satisfies_requirement(
        requirement,
        &parse_version(version)?,
    )?)
}

/// The ref a package ends at, from `[requester, version][]`: the one pin if there is one, `null`
/// if `selectVersion` should choose among the tags. Constraints that cannot all hold throw.
#[wasm_bindgen]
pub fn settle(constraints: JsValue) -> Result<JsValue, JsError> {
    let constraints: Vec<(String, Option<String>)> = serde_wasm_bindgen::from_value(constraints)?;
    let parsed: Vec<_> = constraints
        .iter()
        .map(|(requester, version)| {
            (
                requester.as_str(),
                mars_schema::version::parse_constraint(version.as_deref()),
            )
        })
        .collect();
    let borrowed: Vec<_> = parsed
        .iter()
        .map(|(requester, constraint)| (*requester, constraint))
        .collect();
    to_js(&mars_schema::version::settle(&borrowed)?)
}

/// Choose a tag: `{available, constraints, locked?, policy?, excluded?}` to one of `available`.
/// `constraints` is `[requester, version][]` of semver ranges; `policy` is `prefer-lock-then-latest`
/// (the default), `latest-only` or `lock-only`. No tag meeting every constraint throws, with the
/// message naming each requirement and the versions there are.
#[wasm_bindgen(js_name = selectVersion)]
pub fn select_version(input: JsValue) -> Result<String, JsError> {
    use mars_schema::version::{Constraint, SelectionPolicy};
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        available: Vec<String>,
        constraints: Vec<(String, String)>,
        locked: Option<String>,
        policy: Option<Policy>,
        excluded: Option<Vec<String>>,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "kebab-case")]
    enum Policy {
        PreferLockThenLatest,
        LatestOnly,
        LockOnly,
    }
    let input: Input = serde_wasm_bindgen::from_value(input)?;
    let versions = parse_tags(&input.available)?;
    let requirements = input
        .constraints
        .iter()
        .map(
            |(requester, text)| match mars_schema::version::parse_constraint(Some(text)) {
                Constraint::Semver(req) => Ok((requester.as_str(), req)),
                other => Err(JsError::new(&format!(
                    "`{requester}`: `{other}` is not a semver range"
                ))),
            },
        )
        .collect::<Result<Vec<_>, _>>()?;
    let constraints: Vec<_> = requirements
        .iter()
        .map(|(requester, req)| (*requester, req))
        .collect();
    let locked = input
        .locked
        .as_deref()
        .map(|tag| parse_tags(&[tag.to_owned()]).map(|mut versions| versions.remove(0)))
        .transpose()?;
    let excluded = parse_tags(&input.excluded.unwrap_or_default())?
        .into_iter()
        .collect();
    let policy = match input.policy.unwrap_or(Policy::PreferLockThenLatest) {
        Policy::PreferLockThenLatest => SelectionPolicy::PreferLockThenLatest,
        Policy::LatestOnly => SelectionPolicy::LatestOnly,
        Policy::LockOnly => SelectionPolicy::LockOnly,
    };
    let index =
        mars_schema::version::select(&versions, &constraints, locked.as_ref(), policy, &excluded)?;
    Ok(input.available[index].clone())
}

/// The items a consumer installs from a settled graph:
/// `{requests, packages, root?}` to a `SelectionResult`. `packages` is `{name: {prepared,
/// extraItems?}}` with each `prepared` the result of `prepare` as it returned it; edges and
/// `uses-providers` come from its manifest reading, so nothing is re-derived. `requests` are the
/// consumer's entries (`{name, agents?, skills?, exclude?, only_skills?, only_agents?}`); `root`
/// is a package selected whole as the first request (see `mars_schema::selection`).
#[wasm_bindgen(js_name = selectItems)]
pub fn select_items(input: JsValue) -> Result<JsValue, JsError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        requests: Vec<RequestInput>,
        packages: BTreeMap<String, mars_schema::selection::PackageInput>,
        root: Option<mars_schema::selection::PackageInput>,
    }
    #[derive(serde::Deserialize)]
    struct RequestInput {
        name: String,
        #[serde(flatten)]
        filter: mars_schema::filter::FilterConfig,
    }
    let input: Input = serde_wasm_bindgen::from_value(input)?;
    let view = mars_schema::selection::GraphView {
        requests: input
            .requests
            .into_iter()
            .map(|request| mars_schema::selection::Request {
                name: request.name,
                filter: request.filter.to_mode(),
            })
            .collect(),
        root: input.root.map(Into::into),
        packages: input
            .packages
            .into_iter()
            .map(|(name, package)| (name, package.into()))
            .collect(),
    };
    to_js(&mars_schema::selection::select(&view))
}
