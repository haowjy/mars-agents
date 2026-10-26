//! Stable diagnostics returned by schema operations.

use serde::{Deserialize, Serialize};

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

/// A frontmatter key a schema version removed, as the reader and the legacy upgrade share it.
/// `advice` is what the reader tells the author. A `discarded` key has no replacement, so the
/// upgrade and the foreign-dialect import drop it with a warning; every other removed key is
/// renamed or moved by the upgrade, or left for the author.
pub(crate) struct RemovedField {
    pub key: &'static str,
    pub advice: &'static str,
    pub discarded: bool,
}

impl RemovedField {
    pub(crate) const fn replaced(key: &'static str, advice: &'static str) -> Self {
        Self {
            key,
            advice,
            discarded: false,
        }
    }

    pub(crate) const fn discarded(key: &'static str, advice: &'static str) -> Self {
        Self {
            key,
            advice,
            discarded: true,
        }
    }
}

/// What an Agent or skill reader reports for a key it has no field for: `field.removed` (an
/// error) when `removed` lists it, else `field.unknown` (a warning). An unknown key stays in the
/// authored file and reaches no native file.
pub(crate) fn unrecognized_field(removed: &[RemovedField], key: &str) -> SchemaDiagnostic {
    match removed.iter().find(|field| field.key == key) {
        Some(field) => SchemaDiagnostic::new(
            "field.removed",
            Severity::Error,
            key_path(key),
            format!("`{key}` was removed; {}", field.advice),
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
