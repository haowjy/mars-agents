//! Canonical YAML emission for editors that splice bytes into authored frontmatter.
//!
//! One rule decides how every string, key or value, is spelled: plain when it is a
//! safe identifier-like word, JSON double-quoted otherwise. Quoted strings escape
//! every character YAML parsers disagree about, so the output reads back as the
//! same value in `serde_yaml` and in other YAML libraries.

use serde_json::{Number, Value};

/// Largest integer magnitude JavaScript represents unambiguously (`Number.MAX_SAFE_INTEGER`):
/// 2^53 is also what 2^53 + 1 rounds to.
pub(crate) const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// A value that cannot be written as YAML without changing it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EmitError {
    #[error("integer {0} is beyond ±(2^53 − 1) and cannot cross to JavaScript exactly")]
    IntegerTooLarge(String),
    #[error("duplicate frontmatter key `{0}`")]
    DuplicateKey(String),
}

/// Emit one JSON-compatible value for the mapping key at column `indent`.
///
/// A scalar is one line with no leading newline. A collection starts with `\n`, and
/// its children sit at column `indent + 2`. Output never ends in a newline. Splice
/// the result directly after `key:` (a scalar after one space).
pub fn emit_value(value: &Value, indent: usize) -> Result<String, EmitError> {
    check(value)?;
    Ok(emit(value, indent))
}

/// Emit a whole frontmatter mapping: the block between the `---` fences, keys in
/// order, ending in exactly one newline (empty for no keys).
pub fn rewrite_block(values: &[(String, Value)]) -> Result<String, EmitError> {
    let mut seen = std::collections::HashSet::new();
    let mut out = String::new();
    for (key, value) in values {
        if !seen.insert(key.as_str()) {
            return Err(EmitError::DuplicateKey(key.clone()));
        }
        check(value)?;
        push_pair(&mut out, key, value, 0);
        out.push('\n');
    }
    Ok(out)
}

fn check(value: &Value) -> Result<(), EmitError> {
    match value {
        Value::Number(n) => check_number(n),
        Value::Array(values) => values.iter().try_for_each(check),
        Value::Object(map) => map.values().try_for_each(check),
        _ => Ok(()),
    }
}

fn check_number(n: &Number) -> Result<(), EmitError> {
    let too_large = if let Some(v) = n.as_i64() {
        v.unsigned_abs() > MAX_SAFE_INTEGER
    } else if let Some(v) = n.as_u64() {
        v > MAX_SAFE_INTEGER
    } else {
        // JavaScript cannot tell an integer from a float; a whole float this large was
        // an integer that already lost digits.
        n.as_f64()
            .is_some_and(|v| v.fract() == 0.0 && v.abs() > MAX_SAFE_INTEGER as f64)
    };
    if too_large {
        Err(EmitError::IntegerTooLarge(n.to_string()))
    } else {
        Ok(())
    }
}

/// A whole float is written as an integer: JavaScript cannot tell `1.0` from `1`, and the
/// range check has already bounded it to ±2^53.
fn number(n: &Number) -> String {
    match n.as_f64() {
        Some(v) if n.is_f64() && v.fract() == 0.0 => (v as i64).to_string(),
        _ => n.to_string(),
    }
}

fn push_pair(out: &mut String, key: &str, value: &Value, column: usize) {
    out.push_str(&scalar(key));
    out.push(':');
    let rendered = emit(value, column);
    if !rendered.starts_with('\n') {
        out.push(' ');
    }
    out.push_str(&rendered);
}

fn emit(value: &Value, indent: usize) -> String {
    let child = indent + 2;
    match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => number(v),
        Value::String(v) => scalar(v),
        Value::Array(values) if values.is_empty() => "[]".into(),
        Value::Object(values) if values.is_empty() => "{}".into(),
        Value::Array(values) => {
            let mut out = String::new();
            for item in values {
                out.push('\n');
                out.push_str(&" ".repeat(child));
                out.push('-');
                let rendered = emit(item, child);
                match rendered.strip_prefix('\n') {
                    // A collection item continues on the dash line (`- a: 1`).
                    Some(block) => {
                        out.push(' ');
                        out.push_str(&block[child + 2..]);
                    }
                    None => {
                        out.push(' ');
                        out.push_str(&rendered);
                    }
                }
            }
            out
        }
        Value::Object(values) => {
            let mut out = String::new();
            for (key, value) in values {
                out.push('\n');
                out.push_str(&" ".repeat(child));
                push_pair(&mut out, key, value, child);
            }
            out
        }
    }
}

/// Spell a string as one YAML scalar on one line.
fn scalar(s: &str) -> String {
    if is_plain_safe(s) {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if !is_printable(c) => {
                let n = c as u32;
                if n <= 0xff {
                    out.push_str(&format!("\\x{n:02x}"));
                } else {
                    out.push_str(&format!("\\u{n:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// YAML's printable set, minus line separators and the byte-order mark that some
/// parsers treat as structure.
fn is_printable(c: char) -> bool {
    matches!(c, ' '..='~' | '\u{a0}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
        && !matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}')
}

/// Plain scalars are limited to `[A-Za-z_][A-Za-z0-9_./-]*` that no YAML 1.1 or 1.2
/// reader turns into a boolean, null or number.
fn is_plain_safe(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'))
        && !matches!(
            s.to_ascii_lowercase().as_str(),
            "true"
                | "false"
                | "yes"
                | "no"
                | "on"
                | "off"
                | "y"
                | "n"
                | "null"
                | "inf"
                | "nan"
                | "infinity"
        )
}
