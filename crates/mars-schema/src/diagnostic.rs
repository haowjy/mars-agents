//! Stable diagnostics returned by schema operations.

use serde::{Deserialize, Serialize};

use crate::keys::{self, Rewrite, Surface};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

/// Byte offsets into the file text. Only fence and YAML-syntax errors may carry one
/// (`serde_yaml` has no spans; divergence V3), and no reader sets it yet. Every other
/// diagnostic is located by `path`, which consumers map to a range with their own parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaDiagnostic {
    pub code: String,
    pub severity: Severity,
    pub path: Vec<PathSegment>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<ByteRange>,
}

impl SchemaDiagnostic {
    pub fn new(
        code: impl Into<String>,
        severity: Severity,
        path: Vec<PathSegment>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            severity,
            path,
            message: message.into(),
            range: None,
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// What an Agent or skill reader reports for a key it has no field for: `field.removed` (an
/// error) when the key table removes it on `surface`, else `field.unknown` (a warning). An unknown key stays in the
/// authored file and reaches no native file.
pub(crate) fn unrecognized_field(surface: Surface, key: &str) -> SchemaDiagnostic {
    match keys::removed_key(surface, key) {
        Some((_, removal)) => SchemaDiagnostic::new(
            "field.removed",
            Severity::Error,
            key_path(key),
            format!("`{key}` was removed; {}", removal.advice),
        ),
        None => SchemaDiagnostic::new(
            "field.unknown",
            Severity::Warning,
            key_path(key),
            format!(
                "unknown field `{key}` is kept in the file but written to no native file; put harness-specific keys under `harness-overrides.<harness>`"
            ),
        ),
    }
}

/// The one warning for a key that a project, package or user config file carries and this Mars
/// does not read. `table` is the dotted table the key sits in, empty at the top of the file.
/// The key stays in the file: an older Mars cannot tell a typo from a key a newer one reads.
pub fn unknown_key_message(table: &str, key: &str) -> String {
    let table = if table.is_empty() {
        String::new()
    } else {
        format!("[{table}] ")
    };
    format!(
        "{table}unknown key `{key}` is not read by Mars v{} (a typo, or a key from a newer Mars); it stays in the file",
        crate::VERSION
    )
}

/// What the reader reports for a value the key table removed from a field that remains. The legacy
/// upgrade rewrites it to the row's replacement.
pub(crate) fn removed_value(surface: Surface, key: &str, value: &str) -> Option<SchemaDiagnostic> {
    let (_, removal) = keys::removed_value_of(surface, key, value)?;
    let Rewrite::Value(replacement) = removal.rewrite else {
        return None;
    };
    Some(SchemaDiagnostic::new(
        "field.removed",
        Severity::Error,
        key_path(key),
        format!("{key} value `{value}` was removed; use `{replacement}`"),
    ))
}

/// A bad value for a field the item kind defines (`kind` is the noun in the message).
pub(crate) fn push_invalid(
    kind: &str,
    diags: &mut Vec<SchemaDiagnostic>,
    path: Vec<PathSegment>,
    value: impl Into<String>,
    allowed: &'static str,
) {
    let value = value.into();
    let severity = if matches!(path.first(), Some(PathSegment::Key(key)) if key == "harness-overrides")
    {
        Severity::Warning
    } else {
        Severity::Error
    };
    let display_field = if matches!(path.first(), Some(PathSegment::Key(key)) if key == "model-policies")
    {
        path.iter()
            .map(|segment| match segment {
                PathSegment::Key(key) => key.clone(),
                PathSegment::Index(index) => format!("[{}]", index),
            })
            .fold(String::new(), |mut display, segment| {
                if !display.is_empty() && !segment.starts_with('[') {
                    display.push('.');
                }
                display.push_str(&segment);
                display
            })
    } else {
        display_path(&path)
    };
    diags.push(SchemaDiagnostic::new(
        "field.value",
        severity,
        path,
        format!("{kind} field `{display_field}` has invalid value `{value}`; allowed: {allowed}"),
    ));
}

pub(crate) fn key_path(field: impl Into<String>) -> Vec<PathSegment> {
    vec![PathSegment::Key(field.into())]
}

pub(crate) fn child_path(parent: impl Into<String>, child: impl Into<String>) -> Vec<PathSegment> {
    vec![
        PathSegment::Key(parent.into()),
        PathSegment::Key(child.into()),
    ]
}

pub fn display_path(path: &[PathSegment]) -> String {
    let mut display = String::new();
    for segment in path {
        match segment {
            PathSegment::Key(key) => {
                if !display.is_empty() {
                    display.push('.');
                }
                display.push_str(key);
            }
            PathSegment::Index(index) => {
                use std::fmt::Write;
                let _ = write!(display, "[{index}]");
            }
        }
    }
    display
}

pub(crate) fn frontmatter_error(error: &crate::frontmatter::FrontmatterError) -> SchemaDiagnostic {
    let code = match error {
        crate::frontmatter::FrontmatterError::TooLarge(_) => "frontmatter.too-large",
        crate::frontmatter::FrontmatterError::TooDeep => "frontmatter.too-deep",
        crate::frontmatter::FrontmatterError::Unclosed => "frontmatter.unclosed",
        crate::frontmatter::FrontmatterError::Anchor => "yaml.anchor",
        crate::frontmatter::FrontmatterError::DocumentEnd => "yaml.document-end",
        crate::frontmatter::FrontmatterError::UnsupportedSyntax(_) => "yaml.unsupported-syntax",
        crate::frontmatter::FrontmatterError::NotAMapping => "frontmatter.not-mapping",
        crate::frontmatter::FrontmatterError::MalformedYaml(inner) => {
            let message = inner.to_string();
            if message.contains("duplicate entry") || message.contains("duplicate key") {
                "yaml.duplicate-key"
            } else {
                "yaml.invalid"
            }
        }
    };
    SchemaDiagnostic::new(code, Severity::Error, Vec::new(), error.to_string())
}
