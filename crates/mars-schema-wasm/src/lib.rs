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

/// A string argument. `wasm-bindgen`'s own `&str` conversion reads a number or an object as
/// memory, so an export takes the `JsValue` and checks it here.
fn text_arg(value: &JsValue, name: &str) -> Result<String, JsValue> {
    value
        .as_string()
        .ok_or_else(|| fail("input.invalid", format!("`{name}` must be a string")))
}

fn optional_text_arg(value: &JsValue, name: &str) -> Result<Option<String>, JsValue> {
    if value.is_null() || value.is_undefined() {
        Ok(None)
    } else {
        text_arg(value, name).map(Some)
    }
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
pub fn read(path: JsValue, text: JsValue) -> Result<JsValue, JsValue> {
    let (path, text) = (text_arg(&path, "path")?, text_arg(&text, "text")?);
    let reading =
        mars_schema::read::read(&path, &text).map_err(|error| fail(error.code(), error))?;
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

/// One rule in the shape `read` reports a profile's `tools` in.
#[derive(serde::Deserialize)]
struct RuleInput {
    capability: String,
    pattern: Option<String>,
    action: mars_schema::tool_policy::ToolAction,
    #[serde(default)]
    origin: mars_schema::tool_policy::RuleOrigin,
}

/// Lower tool rules onto a host (`claude`, `codex`, `pi`, `opencode`, `antigravity` or
/// `meridian-flow`): the native names per action, each rule's gap or coverage, and how the
/// host handles `ask`. The same code `mars sync` lowers an agent's tools with.
#[wasm_bindgen]
pub fn project(rules: JsValue, host: JsValue) -> Result<JsValue, JsValue> {
    let host = text_arg(&host, "host")?;
    let host = mars_schema::Host::parse(&host)
        .ok_or_else(|| fail("input.invalid", format!("unknown host `{host}`")))?;
    let rules: Vec<RuleInput> = parse_input(rules)?;
    let policy = mars_schema::tool_policy::EffectiveToolPolicy {
        rules: rules
            .iter()
            .map(|rule| {
                mars_schema::tool_policy::ToolRule::from_wire(
                    &rule.capability,
                    rule.pattern.as_deref(),
                    rule.action,
                )
                .map(|projected| projected.with_origin(rule.origin))
                .map_err(|error| {
                    fail(
                        "input.invalid",
                        format!(
                            "tool rule `{}`: expected {}",
                            rule.capability,
                            error.allowed()
                        ),
                    )
                })
            })
            .collect::<Result<_, _>>()?,
    };
    to_js(&mars_schema::tool_project::project(&policy, host))
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
/// `{files: Map<path, Uint8Array>, context?, declaredDialect?}`. A supported export in
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
    }
    let input: Input = parse_fields(argument)?;
    let tree = file_tree(input.files)?;
    let context = input
        .context
        .unwrap_or(mars_schema::prepare::PrepareContext::Package);
    let mut prepared = mars_schema::prepare::prepare(&tree, context, input.declared_dialect);
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

/// Which version of which source, for a whole dependency graph: `{root, locked?, refresh?, engine,
/// bounds?, answers}` to `{needs}`, `{resolved, diagnostics}` or `{failed, diagnostics}`. Each call
/// replays the resolution from every answer so far; see `index.d.ts`. A failure of the resolution
/// is returned as data, an argument that is not that shape throws `input.invalid`.
#[wasm_bindgen]
pub fn resolve(input: JsValue) -> Result<JsValue, JsValue> {
    let input: mars_schema::resolve::wire::Input = parse_fields(input)?;
    let output =
        mars_schema::resolve::wire::run(input).map_err(|error| fail(error.code(), error))?;
    to_js(&output)
}

/// Whether `version` meets a dependency entry's `version` text (`null` or `"latest"` is any
/// version; a branch or commit pin is met by none). A `version` that is not semver throws.
#[wasm_bindgen]
pub fn satisfies(constraint: JsValue, version: JsValue) -> Result<bool, JsValue> {
    let constraint = optional_text_arg(&constraint, "constraint")?;
    let version = text_arg(&version, "version")?;
    let version =
        mars_schema::version::parse_version(&version).map_err(|error| fail(error.code(), error))?;
    let constraint = mars_schema::version::parse_constraint(constraint.as_deref());
    Ok(mars_schema::version::satisfies(&constraint, &version))
}

/// Whether `version` meets a package's `requires-mars` / `requires-meridian`. A requirement that
/// is not one, or a `version` that is not semver, throws.
#[wasm_bindgen(js_name = satisfiesRequirement)]
pub fn satisfies_requirement(requirement: JsValue, version: JsValue) -> Result<bool, JsValue> {
    let (requirement, version) = (
        text_arg(&requirement, "requirement")?,
        text_arg(&version, "version")?,
    );
    let version =
        mars_schema::version::parse_version(&version).map_err(|error| fail(error.code(), error))?;
    mars_schema::version::satisfies_requirement(&requirement, &version)
        .map_err(|error| fail(error.code(), error))
}

/// Which ref a package ends at: `{available, constraints, locked?}` to
/// `{pin} | {locked: true} | {tag} | {untagged: true}`. `available` is the repository's tags; one
/// that is not a version (`nightly`) is ignored. `constraints` is every `[requester, version |
/// null]` on the package, as the manifests spell them; `locked` is kept (`{locked: true}`, even
/// if `available` no longer lists it) when it meets every range, a `latest` or `null` constraint
/// included (it allows any version, so a locked package stays until it is upgraded); otherwise
/// the newest allowed version is chosen. A refusal throws with a `version.*` code.
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

/// The values of a `{name: value}` argument: an object's own values or a `Map`'s, the two forms
/// `serde-wasm-bindgen` reads as a map. `None` for anything else, which `parse_input` reports.
fn map_values(value: &JsValue) -> Option<Vec<JsValue>> {
    if let Some(map) = value.dyn_ref::<js_sys::Map>() {
        Some(map.values().into_iter().filter_map(Result::ok).collect())
    } else {
        value
            .dyn_ref::<js_sys::Object>()
            .map(|object| js_sys::Object::values(object).iter().collect())
    }
}

/// Refuses an unknown key in a package an argument holds, and in its `extra_items`.
fn check_package_keys(package: &JsValue) -> Result<(), JsValue> {
    check_keys::<mars_schema::selection::PackageInput>(package)?;
    let extra_items =
        js_sys::Reflect::get(package, &"extra_items".into()).unwrap_or(JsValue::UNDEFINED);
    // Absent or not an array: `parse_input` reports it.
    if js_sys::Array::is_array(&extra_items) {
        js_sys::Array::from(&extra_items)
            .iter()
            .try_for_each(|item| check_keys::<mars_schema::selection::Item>(&item))?;
    }
    Ok(())
}

/// `selectItems`'s argument has keys at four depths; each is checked against its structure.
fn check_select_items_keys(argument: &JsValue) -> Result<(), JsValue> {
    let get = |object: &JsValue, key: &str| {
        js_sys::Reflect::get(object, &key.into()).unwrap_or(JsValue::UNDEFINED)
    };
    check_keys::<SelectItemsInput>(argument)?;
    let requests = get(argument, "requests");
    if js_sys::Array::is_array(&requests) {
        js_sys::Array::from(&requests)
            .iter()
            .try_for_each(|request| check_keys::<RequestInput>(&request))?;
    }
    map_values(&get(argument, "packages"))
        .into_iter()
        .flatten()
        .try_for_each(|package| check_package_keys(&package))?;
    check_package_keys(&get(argument, "root"))
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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenderTokens {
    hook_dir: Option<String>,
    hook_target: Option<String>,
}

/// A `hook.toml`'s `[hooks]` as each of `targets` (roots such as `.claude`) takes it:
/// `{diagnostics, targets: {<root>: HookOutput}}`, where `HookOutput` is the schema's own
/// serialization (`kind: "merge" | "file"`; see `index.d.ts`). `tokens` (`{hookDir?, hookTarget?}`) fills the hook tokens; one
/// left out stays as written (`hookTarget` defaults to the target's root). `targets` is empty
/// when `diagnostics` holds an error. A root with no hook mechanism throws `input.invalid`.
#[wasm_bindgen(js_name = renderHooks)]
pub fn render_hooks(
    hook_toml: &str,
    targets: Vec<String>,
    tokens: JsValue,
) -> Result<JsValue, JsValue> {
    use mars_schema::hooks::{self, HookOutput, Origin, targets as hook_targets};
    let tokens: RenderTokens = if tokens.is_undefined() || tokens.is_null() {
        RenderTokens {
            hook_dir: None,
            hook_target: None,
        }
    } else {
        parse_fields(tokens)?
    };
    let roots = targets
        .iter()
        .map(|root| {
            hook_targets::target(root)
                .ok_or_else(|| fail("input.invalid", format!("`{root}` has no hook mechanism")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let read = hooks::read(hook_toml, "hook", Origin::Local);
    let mut outputs = Vec::new();
    if let Some(def) = &read.def {
        for target in roots {
            let tokens = hooks::Tokens {
                hook_dir: tokens.hook_dir.as_deref().unwrap_or(hooks::HOOK_DIR_TOKEN),
                hook_target: tokens.hook_target.as_deref().unwrap_or(target.root),
                ..Default::default()
            };
            let output = def
                .output(target, None, Origin::Local, &tokens)
                .map_err(|error| fail("hook.invalid", error))?;
            outputs.push((target.root, output));
        }
    }
    let targets: BTreeMap<_, _> = outputs
        .iter()
        .map(|(root, output)| (*root, output))
        .collect();
    #[derive(Serialize)]
    struct Out<'a> {
        diagnostics: &'a [mars_schema::diagnostic::SchemaDiagnostic],
        targets: BTreeMap<&'a str, &'a HookOutput>,
    }
    to_js(&Out {
        diagnostics: &read.diagnostics,
        targets,
    })
}

/// What a package's files say is wrong with it, from `{files, context?, declaredDialect?,
/// dependencies?}`: `prepare`, `links::package` and `mars_schema::check::check`, in that order.
/// `dependencies` is `{name: {prepared, extra_items?}}`, a plain object or a `Map`, the shape of
/// `selectItems`' `packages` (only each `prepared.selection` is read), so one value serves both.
#[wasm_bindgen]
pub fn check(argument: JsValue) -> Result<JsValue, JsValue> {
    use mars_schema::selection::PackageInput;
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        files: BTreeMap<String, ByteBuf>,
        context: Option<mars_schema::prepare::PrepareContext>,
        declared_dialect: Option<mars_schema::dialect::Dialect>,
        #[serde(default)]
        dependencies: BTreeMap<String, PackageInput>,
    }
    check_keys::<Input>(&argument)?;
    map_values(
        &js_sys::Reflect::get(&argument, &"dependencies".into()).unwrap_or(JsValue::UNDEFINED),
    )
    .into_iter()
    .flatten()
    .try_for_each(|package| check_package_keys(&package))?;
    let input: Input = parse_input(argument)?;
    let tree = file_tree(input.files)?;
    let context = input
        .context
        .unwrap_or(mars_schema::prepare::PrepareContext::Package);
    let mut prepared = mars_schema::prepare::prepare(&tree, context, input.declared_dialect);
    mars_schema::links::package(&mut prepared);
    let dependencies = input
        .dependencies
        .into_iter()
        .map(|(name, package)| {
            (
                name,
                mars_schema::selection::PackageView::from(package).facts,
            )
        })
        .collect();
    to_js(&mars_schema::check::check(&tree, &prepared, &dependencies))
}

// ---- Effort and aliases -------------------------------------------------------------------

/// The reasoning levels, lowest first: `none < minimal < low < medium < high < xhigh < max`.
#[wasm_bindgen(js_name = effortLevels)]
pub fn effort_levels() -> Result<JsValue, JsValue> {
    let levels: Vec<&str> = mars_schema::agent::EffortLevel::LADDER
        .iter()
        .map(|level| level.as_str())
        .collect();
    to_js(&levels)
}

/// The level to send for `requested` on a model and a host: `{ level, moved }`. `requested` is
/// a level, `"auto"` or `null` (both send no effort). `model` is the models.dev row's
/// `{ reasoning, reasoning_options }`, or `null` when the model is unknown. `host` is a harness
/// or `meridian-flow`. A level the model or host does not take moves to the nearest one they
/// share, and `moved` says from what, to what and why.
#[wasm_bindgen(js_name = resolveEffort)]
pub fn resolve_effort(
    requested: Option<String>,
    model: JsValue,
    host: &str,
) -> Result<JsValue, JsValue> {
    use mars_schema::agent::EffortRequest;
    let requested = match requested {
        None => EffortRequest::Default,
        Some(text) => EffortRequest::parse(&text).ok_or_else(|| {
            fail(
                "input.invalid",
                format!(
                    "unknown effort `{text}`; allowed: {}, or null",
                    EffortRequest::ALLOWED
                ),
            )
        })?,
    };
    let host = mars_schema::Host::parse(host)
        .ok_or_else(|| fail("input.invalid", format!("unknown host `{host}`")))?;
    let row = if model.is_null() || model.is_undefined() {
        None
    } else {
        let model: serde_json::Value = parse_input(model)?;
        mars_schema::models::ModelReasoningRow::from_models_dev(&model)
    };
    let reasoning = mars_schema::effort::ModelReasoning::from_row(row.as_ref());
    to_js(&mars_schema::effort::resolve_effort(
        requested,
        &reasoning,
        &mars_schema::effort::harness_levels(host),
    ))
}

/// A models.dev model as Flow holds it: the fields an alias reads.
#[derive(serde::Deserialize)]
struct AliasRow {
    id: String,
    /// models.dev's provider key (`anthropic`); the key a row read from Mars's own snapshot
    /// carries as `provider_key` wins, since there `provider` is the display name.
    provider: String,
    provider_key: Option<String>,
    release_date: Option<String>,
}

impl AliasRow {
    fn key(&self) -> &str {
        self.provider_key.as_deref().unwrap_or(&self.provider)
    }
}

fn alias_rows(rows: JsValue) -> Result<Vec<mars_schema::models::CachedModel>, JsValue> {
    let rows: Vec<AliasRow> = parse_input(rows)?;
    Ok(rows
        .into_iter()
        .map(|row| mars_schema::models::CachedModel {
            provider_key: row.key().to_owned(),
            provider: row.provider,
            id: row.id,
            release_date: row.release_date,
            ..Default::default()
        })
        .collect())
}

/// An alias as the three alias exports take it: a plain object of `ModelAlias`'s keys, and no
/// other.
fn alias_arg(alias: JsValue) -> Result<mars_schema::alias::ModelAlias, JsValue> {
    parse_fields(alias)
}

/// The rows of the providers a project's `catalog_providers` names, the rows `mars models
/// resolve` resolves over: `providers` is that list, `["*"]` allows every provider, and
/// `undefined` is the list a project that sets none gets. A row's provider is its `provider_key`,
/// else its `provider`. The rows come back as given.
#[wasm_bindgen(js_name = narrowRows)]
pub fn narrow_rows(rows: JsValue, providers: JsValue) -> Result<JsValue, JsValue> {
    let providers: Vec<String> = if providers.is_undefined() {
        mars_schema::models::DEFAULT_CATALOG_PROVIDERS
            .iter()
            .map(|provider| (*provider).to_owned())
            .collect()
    } else {
        parse_input(providers)?
    };
    let allowed = mars_schema::models::provider_filter(&providers);
    if !js_sys::Array::is_array(&rows) {
        return Err(fail("input.invalid", "`rows` must be an array"));
    }
    let kept = js_sys::Array::new();
    for row in js_sys::Array::from(&rows).iter() {
        let parsed: AliasRow = parse_input(row.clone())?;
        if allowed(parsed.key()) {
            kept.push(&row);
        }
    }
    Ok(kept.into())
}

/// The model id an alias resolves to among models.dev rows, or `null`: a pinned `model` as
/// written, else the best row an `auto` alias selects. The code `mars models resolve` runs.
#[wasm_bindgen(js_name = resolveAlias)]
pub fn resolve_alias(rows: JsValue, alias: JsValue) -> Result<JsValue, JsValue> {
    let alias = alias_arg(alias)?;
    to_js(&alias.resolve(&alias_rows(rows)?))
}

/// Every id an alias accepts, in `resolveAlias`'s order.
#[wasm_bindgen(js_name = aliasCandidates)]
pub fn alias_candidates(rows: JsValue, alias: JsValue) -> Result<JsValue, JsValue> {
    let alias = alias_arg(alias)?;
    to_js(&alias.candidates(&alias_rows(rows)?))
}

/// Resolve every alias of a parsed manifest's `models`, the way `mars models resolve` does. The
/// manifest is a full reading, so its other keys are not checked; each alias is checked as
/// `resolveAlias` checks its own.
#[wasm_bindgen(js_name = resolveAliases)]
pub fn resolve_aliases(manifest: JsValue, rows: JsValue) -> Result<JsValue, JsValue> {
    let not_object = || fail("input.invalid", "`manifest` must be an object");
    if !manifest.is_object() || js_sys::Array::is_array(&manifest) {
        return Err(not_object());
    }
    let models = js_sys::Reflect::get(&manifest, &"models".into()).map_err(|_| not_object())?;
    let mut aliases = Vec::new();
    if !models.is_undefined() && !models.is_null() {
        if !models.is_object() || js_sys::Array::is_array(&models) {
            return Err(fail("input.invalid", "`manifest.models` must be an object"));
        }
        for entry in js_sys::Object::entries(&js_sys::Object::from(models)).iter() {
            let entry = js_sys::Array::from(&entry);
            let name = entry.get(0).as_string().unwrap_or_default();
            // The result is a plain object, which cannot hold this key.
            if name == "__proto__" {
                return Err(fail(
                    "input.invalid",
                    "an alias cannot be named `__proto__`: JavaScript cannot hold it as a plain property",
                ));
            }
            aliases.push((name, alias_arg(entry.get(1))?));
        }
    }
    let rows = alias_rows(rows)?;
    to_js(
        &aliases
            .iter()
            .map(|(name, alias)| (name, alias.resolve(&rows)))
            .collect::<indexmap::IndexMap<_, _>>(),
    )
}
