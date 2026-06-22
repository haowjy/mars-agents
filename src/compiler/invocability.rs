//! Shared `model-invocable` / `user-invocable` axis parsing for agents and skills.
//!
//! Both item kinds use the same YAML keys and collapsed defaults; presence bits
//! record whether the author explicitly set the axis (needed for later lowering
//! that must distinguish omitted from explicit `true`).

use serde_yaml::Value;

use crate::frontmatter::Frontmatter;

pub fn value_label(val: &Value) -> String {
    val.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{val:?}"))
}

/// Read an invocability axis from frontmatter (`model-invocable` or kebab/snake alias).
pub fn get_invocability_field<'a>(fm: &'a Frontmatter, kebab_key: &str) -> Option<&'a Value> {
    fm.get(kebab_key).or_else(|| {
        let snake = kebab_key.replace('-', "_");
        fm.get(&snake)
    })
}

/// Parse a single invocability axis.
///
/// Returns `(collapsed_value, field_was_explicitly_set, invalid_value_label)`.
/// Omitted fields default to `true` with `field_was_explicitly_set = false`.
/// Invalid non-boolean values default to `true`, leave the presence bit false,
/// and return a value label for the caller's diagnostic.
pub fn parse_invocability_axis(raw: Option<&Value>) -> (bool, bool, Option<String>) {
    match raw {
        Some(raw) => match raw.as_bool() {
            Some(value) => (value, true, None),
            None => (true, false, Some(value_label(raw))),
        },
        None => (true, false, None),
    }
}
