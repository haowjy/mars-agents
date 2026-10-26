//! `harness-overrides`: the per-harness passthrough blocks of an Agent or skill.
//!
//! Mars validates only shape and serializability. Nested keys belong to the target harness and
//! are never interpreted as Mars fields.

use indexmap::IndexMap;
use serde_yaml::Value;

use crate::HarnessId;
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity, key_path, push_invalid};
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
