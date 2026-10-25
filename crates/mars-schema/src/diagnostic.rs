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
}

pub fn key_path(field: &str) -> Vec<PathSegment> {
    let mut path = Vec::new();
    for part in field.split('.') {
        if let Some(open) = part.find('[') {
            if open > 0 {
                path.push(PathSegment::Key(part[..open].to_owned()));
            }
            if let Some(index) = part[open + 1..]
                .strip_suffix(']')
                .and_then(|value| value.parse().ok())
            {
                path.push(PathSegment::Index(index));
            }
        } else if !part.is_empty() {
            path.push(PathSegment::Key(part.to_owned()));
        }
    }
    path
}
