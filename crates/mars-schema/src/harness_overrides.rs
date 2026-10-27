//! `harness-overrides`: the per-harness passthrough blocks of an Agent or skill.
//!
//! Mars validates only shape and serializability. Nested keys belong to the target harness and
//! are never interpreted as Mars fields.

use indexmap::IndexMap;
use serde_yaml::Value;

use crate::HarnessId;
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity, key_path, push_invalid};
use crate::frontmatter::Frontmatter;
use crate::item::DefinitionKind;
use crate::ordered::OrderedValue;

/// Per-harness target-native passthrough blocks (`harness-overrides:`).
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(transparent)]
pub struct HarnessOverrides {
    pub entries: IndexMap<String, IndexMap<String, OrderedValue>>,
}

impl HarnessOverrides {
    pub fn get(&self, harness: &HarnessId) -> Option<&IndexMap<String, OrderedValue>> {
        self.entries.get(harness.as_str())
    }
}

fn push_unknown_harness_override(diags: &mut Vec<SchemaDiagnostic>, value: &str) {
    let mut path = key_path("harness-overrides");
    path.push(PathSegment::Key(value.to_owned()));
    diags.push(SchemaDiagnostic::new(
        "harness.unknown",
        Severity::Warning,
        path,
        format!("unknown harness override `{value}`; preserving passthrough block"),
    ));
}

/// Report a key JavaScript cannot hold as a plain property; the key is left out of the reading.
fn push_unholdable_key(diags: &mut Vec<SchemaDiagnostic>, path: &[PathSegment], key: &str) -> bool {
    let Err(message) = crate::ordered::reject_unholdable_key(key) else {
        return false;
    };
    diags.push(SchemaDiagnostic::new(
        "yaml.non-json-value",
        Severity::Error,
        path.to_vec(),
        message,
    ));
    true
}

fn parse_native_config_value(
    kind: &str,
    path: &[PathSegment],
    value: &Value,
    diags: &mut Vec<SchemaDiagnostic>,
) -> Option<OrderedValue> {
    match value {
        Value::Null => {
            push_invalid(
                kind,
                diags,
                path.to_vec(),
                "null".to_string(),
                "non-null scalar, array, or map value",
            );
            None
        }
        Value::Bool(v) => Some(OrderedValue::Bool(*v)),
        Value::String(v) => Some(OrderedValue::String(v.clone())),
        Value::Number(v) => {
            if v.as_i64()
                .is_some_and(|n| n.unsigned_abs() > crate::ordered::MAX_SAFE_INTEGER)
                || v.as_u64()
                    .is_some_and(|n| n > crate::ordered::MAX_SAFE_INTEGER)
            {
                push_invalid(
                    kind,
                    diags,
                    path.to_vec(),
                    v.to_string(),
                    "integer within ±(2^53 − 1)",
                );
                None
            } else if let Some(number) = v.as_i64().map(serde_json::Number::from) {
                Some(OrderedValue::Number(number))
            } else if let Some(number) = v.as_u64().map(serde_json::Number::from) {
                Some(OrderedValue::Number(number))
            } else if let Some(float) = v.as_f64() {
                match serde_json::Number::from_f64(float) {
                    Some(number) => Some(OrderedValue::Number(number)),
                    None => {
                        push_invalid(
                            kind,
                            diags,
                            path.to_vec(),
                            float.to_string(),
                            "finite JSON number",
                        );
                        None
                    }
                }
            } else {
                push_invalid(
                    kind,
                    diags,
                    path.to_vec(),
                    crate::ordered::plain(value),
                    "JSON number",
                );
                None
            }
        }
        Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for (index, entry) in seq.iter().enumerate() {
                let mut child_path = path.to_vec();
                child_path.push(PathSegment::Index(index));
                if let Some(parsed) = parse_native_config_value(kind, &child_path, entry, diags) {
                    out.push(parsed);
                }
            }
            Some(OrderedValue::Array(out))
        }
        Value::Mapping(mapping) => {
            let mut out = IndexMap::new();
            for (key, entry) in mapping {
                let Some(key_text) = key.as_str() else {
                    push_invalid(
                        kind,
                        diags,
                        path.to_vec(),
                        crate::ordered::plain(key),
                        "string keys",
                    );
                    continue;
                };
                let mut child_path = path.to_vec();
                child_path.push(PathSegment::Key(key_text.to_owned()));
                if push_unholdable_key(diags, &child_path, key_text) {
                    continue;
                }
                if let Some(parsed) = parse_native_config_value(kind, &child_path, entry, diags) {
                    out.insert(key_text.to_string(), parsed);
                }
            }
            Some(OrderedValue::Object(out))
        }
        _ => {
            push_invalid(
                kind,
                diags,
                path.to_vec(),
                crate::ordered::plain(value),
                "YAML/TOML/JSON-serializable value",
            );
            None
        }
    }
}

/// `harness-overrides` of an Agent or skill; `kind` names the item in diagnostics.
pub(crate) fn parse_harness_overrides(
    kind: &str,
    val: &Value,
    diags: &mut Vec<SchemaDiagnostic>,
) -> HarnessOverrides {
    let root = key_path("harness-overrides");
    let mut out = HarnessOverrides::default();
    let Some(mapping) = val.as_mapping() else {
        push_invalid(
            kind,
            diags,
            root.clone(),
            crate::ordered::plain(val),
            "mapping of harness name to target-native config mapping",
        );
        return out;
    };

    for (k, v) in mapping {
        let Some(harness_name) = k.as_str() else {
            push_invalid(
                kind,
                diags,
                root.clone(),
                crate::ordered::plain(k),
                "string harness keys",
            );
            continue;
        };
        let mut harness_path = root.clone();
        harness_path.push(PathSegment::Key(harness_name.to_owned()));
        if push_unholdable_key(diags, &harness_path, harness_name) {
            continue;
        }
        let Some(sub_mapping) = v.as_mapping() else {
            push_invalid(
                kind,
                diags,
                harness_path,
                crate::ordered::plain(v),
                "mapping of target-native keys",
            );
            continue;
        };
        if !crate::harness::RESERVED_CONSUMER_NAMESPACES.contains(&harness_name)
            && crate::harness::parse(harness_name).is_none()
        {
            push_unknown_harness_override(diags, harness_name);
        }
        let mut parsed = IndexMap::new();
        for (target_key, target_value) in sub_mapping {
            let Some(key_text) = target_key.as_str() else {
                push_invalid(
                    kind,
                    diags,
                    harness_path.clone(),
                    crate::ordered::plain(target_key),
                    "string target-native keys",
                );
                continue;
            };
            let mut value_path = harness_path.clone();
            value_path.push(PathSegment::Key(key_text.to_owned()));
            if push_unholdable_key(diags, &value_path, key_text) {
                continue;
            }
            if let Some(value) = parse_native_config_value(kind, &value_path, target_value, diags) {
                parsed.insert(key_text.to_string(), value);
            }
        }
        if !parsed.is_empty() {
            out.entries.insert(harness_name.to_string(), parsed);
        }
    }

    out
}

/// How a move of top-level keys under `harness-overrides.<harness>` ended.
pub(crate) enum Moved {
    /// `harness-overrides` or its `<harness>` entry is not a mapping; nothing moved.
    Blocked,
    /// `moved` keys are under the override now; `displaced` keys were dropped because the
    /// override already sets that key, and an override already written wins.
    Done {
        moved: Vec<String>,
        displaced: Vec<String>,
    },
}

/// Move the top-level `keys` of `fm`, in the order given and unchanged, under
/// `harness-overrides.<harness>`; each `(from, to)` pair renames the key as it moves.
pub(crate) fn move_keys_under(fm: &mut Frontmatter, harness: &str, keys: &[(&str, &str)]) -> Moved {
    let harness_key = Value::String(harness.to_owned());
    let mut overrides = match fm.get("harness-overrides") {
        None => serde_yaml::Mapping::new(),
        Some(Value::Mapping(existing)) => existing.clone(),
        Some(_) => return Moved::Blocked,
    };
    if overrides
        .get(&harness_key)
        .is_some_and(|entry| !entry.is_mapping())
    {
        return Moved::Blocked;
    }
    let destination = |overrides: &serde_yaml::Mapping, to: &str| {
        overrides
            .get(&harness_key)
            .and_then(Value::as_mapping)
            .and_then(|entry| entry.get(to))
            .cloned()
    };
    let (mut moved, mut displaced, mut carried) = (Vec::new(), Vec::new(), Vec::new());
    for (from, to) in keys {
        let Some(value) = fm.get(from).cloned() else {
            continue;
        };
        match destination(&overrides, to) {
            // An override already written wins, and its top-level twin is dropped.
            Some(existing) if existing != value => displaced.push((*from).to_owned()),
            Some(_) => moved.push((*from).to_owned()),
            None => {
                moved.push((*from).to_owned());
                carried.push(((*to).to_owned(), value));
            }
        }
    }
    if moved.is_empty() && displaced.is_empty() {
        return Moved::Done { moved, displaced };
    }
    if let Some(Value::Mapping(entry)) = overrides.get_mut(&harness_key) {
        for (to, value) in carried {
            entry.insert(Value::String(to), value);
        }
    } else {
        let entry = carried
            .into_iter()
            .map(|(to, value)| (Value::String(to), value))
            .collect::<serde_yaml::Mapping>();
        overrides.insert(harness_key, Value::Mapping(entry));
    }
    // Writing the block back where it already sits keeps its place in the file, and only a file
    // with no `harness-overrides` gains one, at the end.
    match fm.get_mut("harness-overrides") {
        Some(block) => *block = Value::Mapping(overrides),
        None => fm.insert("harness-overrides", Value::Mapping(overrides)),
    }
    for key in moved.iter().chain(&displaced) {
        fm.remove(key);
    }
    Moved::Done { moved, displaced }
}

fn backticked(keys: &[String]) -> String {
    keys.iter()
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Move the keys only `harness` reads (those of `keys` present in `fm`) to
/// `harness-overrides.<harness>`, so that harness's native files keep them and no other
/// harness's does. Reports an info note of what moved, a warning for a key an existing override
/// displaced, and a warning when the keys could not move.
pub(crate) fn move_harness_only_keys(
    fm: &mut Frontmatter,
    kind: DefinitionKind,
    harness: HarnessId,
    keys: &[&str],
    changes: &mut Vec<SchemaDiagnostic>,
) -> bool {
    let (owner, harness_name) = (kind.to_string(), harness.as_str());
    let title = match harness {
        HarnessId::Claude => "Claude",
        HarnessId::Codex => "Codex",
        HarnessId::Pi => "Pi",
        HarnessId::OpenCode => "OpenCode",
        HarnessId::Cursor => "Cursor",
    };
    let present = fm
        .keys()
        .into_iter()
        .filter(|key| keys.contains(&key.as_str()))
        .collect::<Vec<_>>();
    if present.is_empty() {
        return false;
    }
    let mut note = |severity, suffix: &str, message: String| {
        changes.push(SchemaDiagnostic::new(
            format!("{owner}.{harness_name}-keys-{suffix}"),
            severity,
            Vec::new(),
            message,
        ));
    };
    let pairs = present
        .iter()
        .map(|key| (key.as_str(), key.as_str()))
        .collect::<Vec<_>>();
    match move_keys_under(fm, harness_name, &pairs) {
        Moved::Blocked => {
            note(
                Severity::Warning,
                "unmoved",
                format!(
                    "left {title}-only keys in place because harness-overrides or harness-overrides.{harness_name} is not a mapping: {}",
                    backticked(&present)
                ),
            );
            false
        }
        Moved::Done { moved, displaced } => {
            if !moved.is_empty() {
                note(
                    Severity::Info,
                    "moved",
                    format!(
                        "moved {title}-only keys to harness-overrides.{harness_name}: {}",
                        backticked(&moved)
                    ),
                );
            }
            if !displaced.is_empty() {
                note(
                    Severity::Warning,
                    "conflict",
                    format!(
                        "dropped {title}-only keys that harness-overrides.{harness_name} already sets, whose values win: {}",
                        backticked(&displaced)
                    ),
                );
            }
            true
        }
    }
}

/// Authored markdown with every consumer-namespace entry under `harness-overrides` blanked, one
/// line for one line.
///
/// A consumer namespace (`flow`, `meridian`) is not a harness: Mars keeps its block for the
/// consumer and never writes it to a tree. A caller scanning the text for what a package ships
/// must therefore not read it, which is what this masking is for. Line numbers survive, so a
/// location the caller reports still points at the authored file.
pub fn mask_consumer_namespaces(content: &str) -> String {
    let Ok(Some(yaml)) = crate::frontmatter::yaml_text(content) else {
        return content.to_owned();
    };
    let blocks = consumer_namespace_lines(yaml);
    if blocks.is_empty() {
        return content.to_owned();
    }
    // The frontmatter opens on the document's first line, so YAML line `n` is document line
    // `n + 1` (both counted from 0).
    let yaml_lines = yaml.matches('\n').count();
    let mut lines: Vec<&str> = content.split_inclusive('\n').collect();
    for (start, end) in blocks {
        for line in lines
            .iter_mut()
            .take(end.min(yaml_lines) + 1)
            .skip(start + 1)
        {
            *line = "\n";
        }
    }
    lines.concat()
}

/// The `[start, end)` lines of `yaml`, a frontmatter block, that a consumer namespace owns.
fn consumer_namespace_lines(yaml: &str) -> Vec<(usize, usize)> {
    use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
    use yaml_rust2::scanner::Marker;

    /// One open collection, and, for a mapping, the key its next child reports.
    struct Frame {
        pending_key: Option<String>,
    }

    #[derive(Default)]
    struct NamespaceBlocks {
        frames: Vec<Frame>,
        /// Depth of the `harness-overrides` mapping, while one is open.
        overrides_depth: Option<usize>,
        /// Line the open namespace entry starts on.
        open: Option<usize>,
        ranges: Vec<(usize, usize)>,
    }

    impl NamespaceBlocks {
        /// The entry ended at the next thing Mars reads: close it there, and never before the
        /// line its own key sits on.
        fn close(&mut self, end: usize) {
            if let Some(start) = self.open.take() {
                self.ranges.push((start, end.max(start + 1)));
            }
        }
    }

    impl MarkedEventReceiver for NamespaceBlocks {
        fn on_event(&mut self, event: Event, marker: Marker) {
            let line = marker.line() - 1;
            match event {
                Event::MappingStart(..) | Event::SequenceStart(..) => {
                    let parent_depth = self.frames.len();
                    let under_key = self
                        .frames
                        .last_mut()
                        .and_then(|frame| frame.pending_key.take());
                    if parent_depth == 1 && under_key.as_deref() == Some("harness-overrides") {
                        self.overrides_depth = Some(parent_depth + 1);
                    }
                    self.frames.push(Frame { pending_key: None });
                }
                Event::MappingEnd | Event::SequenceEnd => {
                    let depth = self.frames.len();
                    self.frames.pop();
                    if self.overrides_depth == Some(depth) {
                        self.close(line);
                        self.overrides_depth = None;
                    }
                }
                // Inside a mapping, scalars alternate key and value; only keys name a namespace.
                Event::Scalar(value, ..) => {
                    let depth = self.frames.len();
                    let Some(frame) = self.frames.last_mut() else {
                        return;
                    };
                    if frame.pending_key.take().is_some() {
                        return;
                    }
                    frame.pending_key = Some(value.clone());
                    if self.overrides_depth == Some(depth) {
                        self.close(line);
                        if crate::harness::RESERVED_CONSUMER_NAMESPACES.contains(&value.as_str()) {
                            self.open = Some(line);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut blocks = NamespaceBlocks::default();
    // Frontmatter the schema reader already accepted can still leave here unseen; a document this
    // scanner cannot follow simply keeps its current reading.
    if Parser::new_from_str(yaml).load(&mut blocks, true).is_err() {
        return Vec::new();
    }
    blocks.ranges
}
