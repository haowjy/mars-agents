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

/// A frontmatter invocability axis and the key that supplied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocabilityField {
    pub value: Value,
    /// Source key present in frontmatter (`model-invocable` or `model_invocable`, etc.).
    pub consumed_key: String,
}

/// Read an invocability axis from frontmatter (`model-invocable` or kebab/snake alias).
///
/// Prefers the kebab-case key when both aliases are present.
pub fn find_invocability_field(fm: &Frontmatter, kebab_key: &str) -> Option<InvocabilityField> {
    if let Some(value) = fm.get(kebab_key) {
        return Some(InvocabilityField {
            value: value.clone(),
            consumed_key: kebab_key.to_string(),
        });
    }
    let snake = kebab_key.replace('-', "_");
    fm.get(&snake).map(|value| InvocabilityField {
        value: value.clone(),
        consumed_key: snake,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter::Frontmatter;

    #[test]
    fn find_invocability_field_prefers_kebab_when_both_present() {
        let fm = Frontmatter::parse("---\nmodel-invocable: false\nmodel_invocable: true\n---\n")
            .unwrap();
        let field = find_invocability_field(&fm, "model-invocable").unwrap();
        assert_eq!(field.consumed_key, "model-invocable");
        assert_eq!(field.value, Value::Bool(false));
    }

    #[test]
    fn find_invocability_field_reports_snake_alias() {
        let fm = Frontmatter::parse("---\nmodel_invocable: false\n---\n").unwrap();
        let field = find_invocability_field(&fm, "model-invocable").unwrap();
        assert_eq!(field.consumed_key, "model_invocable");
        assert_eq!(field.value, Value::Bool(false));
    }
}
