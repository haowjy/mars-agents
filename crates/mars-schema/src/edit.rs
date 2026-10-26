//! Canonical YAML emission used by editors and the WASM adapter.

use serde_json::Value;

/// Emit one JSON-compatible value as canonical YAML at `indent` spaces.
/// Strings are deliberately double quoted: this avoids YAML 1.1/1.2 scalar
/// disagreements (`012`, `yes`, `on`, and friends) in downstream readers.
pub fn emit_value(value: &Value, indent: usize) -> String {
    emit(value, indent)
}

fn emit(value: &Value, indent: usize) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) if v.contains('\n') => {
            let mut out = String::from("|-\n");
            for line in v.lines() {
                out.push_str(&format!("{}{}\n", " ".repeat(indent + 2), line));
            }
            out
        }
        Value::String(v) => serde_json::to_string(v).expect("JSON strings are serializable"),
        Value::Array(values) if values.is_empty() => "[]".into(),
        Value::Object(values) if values.is_empty() => "{}".into(),
        Value::Array(values) => values
            .iter()
            .map(|v| format!("\n{}- {}", " ".repeat(indent), emit(v, indent + 2)))
            .collect(),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| {
                let rendered = emit(value, indent + 2);
                if rendered.starts_with('\n') {
                    format!("\n{}{}:", " ".repeat(indent), key) + &rendered
                } else {
                    format!("\n{}{}: {}", " ".repeat(indent), key, rendered)
                }
            })
            .collect(),
    }
}

/// Emit a complete frontmatter mapping, without the `---` fences.
pub fn rewrite_block(values: &[(String, Value)]) -> String {
    let mut out = String::new();
    for (key, value) in values {
        let rendered = emit(value, 2);
        if rendered.starts_with('\n') {
            out.push_str(key);
            out.push(':');
            out.push_str(&rendered);
        } else {
            out.push_str(&format!("{key}: {rendered}"));
        }
        out.push('\n');
    }
    out
}
