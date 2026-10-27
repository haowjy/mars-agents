//! `harness-overrides`: the per-harness passthrough blocks of an Agent or skill.
//!
//! Mars validates only shape and serializability. Nested keys belong to the target harness and
//! are never interpreted as Mars fields.

use indexmap::IndexMap;
use serde_yaml::Value;
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::Marker;

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
        if let Some((_, new)) = crate::harness::RENAMED_CONSUMER_NAMESPACES
            .iter()
            .find(|(old, _)| *old == harness_name)
        {
            diags.push(SchemaDiagnostic::new(
                "field.removed",
                Severity::Error,
                harness_path,
                format!(
                    "`harness-overrides.{harness_name}` was renamed; use `harness-overrides.{new}`"
                ),
            ));
            continue;
        }
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
pub(crate) enum Moved<'a> {
    /// `harness-overrides` or its `<harness>` entry is not a mapping; nothing moved.
    Blocked,
    /// Keys are under the override now, or were dropped because the override already sets that
    /// key and an override already written wins.
    Done {
        /// The keys that moved, under the names they have at the top level.
        moved: Vec<&'a str>,
        /// Each displaced key with the name the block knows it by.
        displaced: Vec<(&'a str, &'a str)>,
    },
}

/// Move the top-level `keys` of `fm`, in the order given and unchanged, under
/// `harness-overrides.<harness>`; each `(from, to)` pair renames the key as it moves.
pub(crate) fn move_keys_under<'a>(
    fm: &mut Frontmatter,
    harness: &str,
    keys: &[(&'a str, &'a str)],
) -> Moved<'a> {
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
    for &(from, to) in keys {
        let Some(value) = fm.get(from).cloned() else {
            continue;
        };
        match destination(&overrides, to) {
            // An override already written wins, and its top-level twin is dropped.
            Some(existing) if existing != value => displaced.push((from, to)),
            Some(_) => moved.push(from),
            None => {
                moved.push(from);
                carried.push((to.to_owned(), value));
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
    for key in moved.iter().chain(displaced.iter().map(|(from, _)| from)) {
        fm.remove(key);
    }
    Moved::Done { moved, displaced }
}

fn backticked(keys: impl IntoIterator<Item = impl std::fmt::Display>) -> String {
    keys.into_iter()
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
                        backticked(displaced.iter().map(|(from, _)| from))
                    ),
                );
            }
            true
        }
    }
}

/// Authored markdown with every consumer-namespace entry under `harness-overrides` blanked.
///
/// A consumer namespace (`meridian-flow`, `meridian-cli`) is not a harness: Mars keeps its block for the
/// consumer and never writes it to a tree. A caller scanning the text for what a package ships
/// must therefore not read it, which is what this masking is for. Only the entry's own bytes
/// become spaces and every newline stays, so a location the caller reports still points at the
/// authored file, and what the package ships around the entry is still read.
pub fn mask_consumer_namespaces(content: &str) -> String {
    let Ok(Some((yaml, yaml_start))) = crate::frontmatter::yaml_text(content) else {
        return content.to_owned();
    };
    let spans = consumer_namespace_spans(yaml);
    if spans.is_empty() {
        return content.to_owned();
    }
    let mut masked = content.as_bytes().to_vec();
    for (start, end) in spans {
        for byte in &mut masked[yaml_start + start..yaml_start + end] {
            if !matches!(*byte, b'\n' | b'\r') {
                *byte = b' ';
            }
        }
    }
    // Replacing whole characters with spaces leaves the document's UTF-8 valid.
    String::from_utf8(masked).expect("masking a UTF-8 document keeps it UTF-8")
}

/// One open collection of a frontmatter block, and where its next child sits.
struct Frame {
    /// A mapping's children alternate key and value; a sequence's are all values.
    mapping: bool,
    /// A mapping's next child is a key.
    expect_key: bool,
    /// This collection is the value of the mapping under it, so only its end consumes that
    /// value: a nested key is still a key.
    parent_value: bool,
    /// The last key read from this mapping, for finding the block a namespace entry sits in.
    key: Option<String>,
}

impl Frame {
    fn new(mapping: bool, parent_value: bool) -> Self {
        Self {
            mapping,
            expect_key: mapping,
            parent_value,
            key: None,
        }
    }
}

/// Collects the byte spans of the entries a consumer namespace owns under `harness-overrides`.
///
/// The spans are the scanner's own: an entry starts at its key and ends where the next thing Mars
/// reads begins, so a block mapping, a flow mapping and an alias's value are one shape here.
#[derive(Default)]
struct NamespaceSpans<'a> {
    yaml: &'a str,
    frames: Vec<Frame>,
    /// Depth of the `harness-overrides` mapping, while one is open.
    overrides_depth: Option<usize>,
    /// Byte offset where the entry being read starts, and the column of its key.
    open: Option<(usize, usize)>,
    spans: Vec<(usize, usize)>,
    /// Scanner positions are character indices; the spans are byte offsets.
    char_to_byte: Vec<usize>,
}

impl NamespaceSpans<'_> {
    fn byte_of(&self, marker: Marker) -> usize {
        self.char_to_byte
            .get(marker.index())
            .copied()
            .unwrap_or(self.yaml.len())
    }

    /// Whether the node starting now is the next key of the `harness-overrides` mapping.
    fn entry_key(&self) -> bool {
        self.overrides_depth == Some(self.frames.len())
            && self.frames.last().is_some_and(|frame| frame.expect_key)
    }

    /// The byte offset at which the line holding `byte` starts.
    fn line_start(&self, byte: usize) -> usize {
        self.yaml[..byte]
            .rfind('\n')
            .map_or(0, |newline| newline + 1)
    }

    /// The open entry ends where the next thing Mars reads begins. The spacing and the comments a
    /// later key owns are not its own: a comment indented at or left of the entry's key belongs to
    /// that key, while one indented further in is the entry's own text.
    fn close(&mut self, end: usize) {
        let Some((start, column)) = self.open.take() else {
            return;
        };
        self.spans
            .push((start, self.trimmed_end(start, column, end).max(start)));
    }

    fn trimmed_end(&self, start: usize, column: usize, end: usize) -> usize {
        let bytes = self.yaml.as_bytes();
        let mut stop = end;
        loop {
            while stop > start && bytes[stop - 1].is_ascii_whitespace() {
                stop -= 1;
            }
            let line_start = self.line_start(stop);
            let line = &self.yaml[line_start..stop];
            let indent = line.len() - line.trim_start_matches(' ').len();
            if !line[indent..].starts_with('#') || indent > column {
                return stop;
            }
            stop = line_start;
        }
    }

    /// A scalar or an alias starts: in a mapping, either the key its frame waits for or the value
    /// of the key just read.
    fn node(&mut self, key: Option<String>) {
        if let Some(parent) = self.frames.last_mut().filter(|parent| parent.mapping) {
            let was_key = parent.expect_key;
            parent.expect_key = !was_key;
            if was_key {
                parent.key = key;
            }
        }
    }

    /// A collection starts. A value collection consumes its mapping's value only when it ends.
    fn start(&mut self, mapping: bool) {
        let mut parent_value = false;
        if let Some(parent) = self.frames.last_mut().filter(|parent| parent.mapping) {
            parent_value = !parent.expect_key;
            if parent.expect_key {
                // A complex key starts here, and it names no harness.
                parent.expect_key = false;
                parent.key = None;
            }
        }
        self.frames.push(Frame::new(mapping, parent_value));
    }

    fn pop(&mut self) {
        let Some(frame) = self.frames.pop() else {
            return;
        };
        if frame.parent_value
            && let Some(parent) = self.frames.last_mut()
        {
            parent.expect_key = true;
        }
    }
}

impl MarkedEventReceiver for NamespaceSpans<'_> {
    fn on_event(&mut self, event: Event, marker: Marker) {
        let byte = self.byte_of(marker);
        match event {
            Event::Scalar(value, ..) => {
                let entry = self.entry_key();
                let namespace =
                    entry && crate::harness::RESERVED_CONSUMER_NAMESPACES.contains(&value.as_str());
                if entry {
                    self.close(byte);
                }
                self.node(Some(value));
                if namespace {
                    self.open = Some((byte, byte - self.line_start(byte)));
                }
            }
            Event::Alias(_) => {
                if self.entry_key() {
                    self.close(byte);
                }
                self.node(None);
            }
            start @ (Event::MappingStart(..) | Event::SequenceStart(..)) => {
                let mapping = matches!(start, Event::MappingStart(..));
                // The mapping `harness-overrides` holds, whose entries name the namespaces.
                let overrides = mapping
                    && self.frames.len() == 1
                    && self.frames.last().is_some_and(|root| {
                        !root.expect_key && root.key.as_deref() == Some("harness-overrides")
                    });
                if self.entry_key() {
                    self.close(byte);
                }
                self.start(mapping);
                if overrides {
                    self.overrides_depth = Some(self.frames.len());
                }
            }
            Event::MappingEnd | Event::SequenceEnd => {
                if self.overrides_depth == Some(self.frames.len()) {
                    self.close(byte);
                    self.overrides_depth = None;
                }
                self.pop();
            }
            _ => {}
        }
    }
}

/// The `[start, end)` byte spans of `yaml`, a frontmatter block's YAML, that a consumer namespace
/// owns. An anchor defined inside an entry and aliased from a shipped one still hides that text;
/// the reader rejects anchors (`yaml.anchor`) before what it hides is reported.
fn consumer_namespace_spans(yaml: &str) -> Vec<(usize, usize)> {
    let mut char_to_byte: Vec<usize> = yaml.char_indices().map(|(byte, _)| byte).collect();
    char_to_byte.push(yaml.len());
    let mut spans = NamespaceSpans {
        yaml,
        char_to_byte,
        ..NamespaceSpans::default()
    };
    // Frontmatter the schema reader already accepted can still leave here unseen; a document this
    // scanner cannot follow simply keeps its current reading.
    if Parser::new_from_str(yaml).load(&mut spans, true).is_err() {
        return Vec::new();
    }
    spans.spans
}
