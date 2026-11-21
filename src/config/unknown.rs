//! An unknown key in the project's own `mars.toml` or `mars.local.toml`: a warning, with the key
//! it is closest to. The note says what Mars did not read; this says what that costs. A key
//! that is not `targets` leaves the project with no targets, and a sync removes what it
//! installed there.

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};

use mars_schema::diagnostic::{PathSegment, SchemaDiagnostic};

use crate::diagnostic::{Diagnostic, DiagnosticLevel};

/// The note for one key the file leaves unread, as a warning, naming the closest key the table
/// reads. `T` is the file's type and `S` its `[settings]` table.
pub(super) fn warning<T: DeserializeOwned, S: DeserializeOwned>(
    note: &SchemaDiagnostic,
    file: &str,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::from_schema(note, format!("{file}: {}", note.message));
    if note.code != "field.unknown" {
        return diagnostic;
    }
    diagnostic.level = DiagnosticLevel::Warning;
    let known = match note.path.as_slice() {
        [PathSegment::Key(_)] => Some(field_names::<T>()),
        [PathSegment::Key(table), PathSegment::Key(_)] if table == "settings" => {
            Some(field_names::<S>())
        }
        _ => None,
    };
    if let (Some(known), Some(PathSegment::Key(key))) = (known, note.path.last())
        && let Some(close) = known.iter().find(|name| is_close(key, name))
    {
        diagnostic
            .message
            .push_str(&format!(". Did you mean `{close}`?"));
    }
    diagnostic
}

fn is_close(key: &str, name: &str) -> bool {
    let distance = edit_distance(key, name);
    distance <= 2 && distance * 3 <= key.len()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != *cb))
                .min(above + 1)
                .min(row[j] + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

/// The keys serde reads for a struct: what `T`'s derived `Deserialize` asks for when it is asked
/// to read a struct, so the list cannot drift from the fields.
fn field_names<T: DeserializeOwned>() -> &'static [&'static str] {
    match T::deserialize(Fields) {
        Err(Found(fields)) => fields,
        Ok(_) => &[],
    }
}

struct Fields;

#[derive(Debug)]
struct Found(&'static [&'static str]);

impl std::fmt::Display for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not a struct")
    }
}

impl std::error::Error for Found {}

impl de::Error for Found {
    fn custom<M: std::fmt::Display>(_: M) -> Self {
        Found(&[])
    }
}

impl<'de> Deserializer<'de> for Fields {
    type Error = Found;

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Found> {
        Err(Found(&[]))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, Found> {
        Err(Found(fields))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        option unit unit_struct newtype_struct seq tuple tuple_struct map enum identifier
        ignored_any
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Config, LocalConfig, LocalSettings, Settings};
    use super::*;

    #[test]
    fn closest_key_is_named_for_a_typo_and_not_for_a_stranger() {
        let note = |path: &[&str]| {
            SchemaDiagnostic::new(
                "field.unknown",
                mars_schema::diagnostic::Severity::Info,
                path.iter().map(|p| PathSegment::Key((*p).into())).collect(),
                "unknown key",
            )
        };
        let text = |path: &[&str]| warning::<Config, Settings>(&note(path), "mars.toml").message;
        assert!(text(&["settings", "target"]).ends_with("Did you mean `targets`?"));
        assert!(text(&["setting"]).ends_with("Did you mean `settings`?"));
        assert!(!text(&["settings", "colour"]).contains("Did you mean"));
        assert!(!text(&["agents", "coder", "targets"]).contains("Did you mean"));
        assert!(
            warning::<LocalConfig, LocalSettings>(
                &note(&["settings", "target"]),
                "mars.local.toml"
            )
            .message
            .contains("`targets`")
        );
    }
}
