//! Stable reader result shared by native and WASM callers.

use serde::Serialize;
use serde_yaml::Value;

use crate::diagnostic::SchemaDiagnostic;

#[derive(Debug, Clone, Serialize)]
pub struct DefinitionRead<T> {
    pub fields: Option<T>,
    pub body: Option<String>,
    pub unknown: Vec<(String, Value)>,
    pub diagnostics: Vec<SchemaDiagnostic>,
}
