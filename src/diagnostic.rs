use serde::Serialize;

/// A diagnostic message from library code.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    /// Machine-readable code, e.g. "shadow-collision", "manifest-path-dep".
    pub code: &'static str,
    /// Human-readable message.
    pub message: String,
    /// Optional context (source name, item path, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Warning,
    Info,
}

/// Collects diagnostics during pipeline execution.
pub struct DiagnosticCollector {
    diagnostics: Vec<Diagnostic>,
}

impl DiagnosticCollector {
    pub fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
        }
    }

    pub fn warn(&mut self, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            code,
            message: message.into(),
            context: None,
        });
    }

    pub fn info(&mut self, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Info,
            code,
            message: message.into(),
            context: None,
        });
    }

    pub fn warn_with_context(
        &mut self,
        code: &'static str,
        message: impl Into<String>,
        context: impl Into<String>,
    ) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            code,
            message: message.into(),
            context: Some(context.into()),
        });
    }

    pub fn extend(&mut self, diagnostics: Vec<Diagnostic>) {
        self.diagnostics.extend(diagnostics);
    }

    pub fn drain(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

impl Default for DiagnosticCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefix = match self.level {
            DiagnosticLevel::Warning => "warning",
            DiagnosticLevel::Info => "info",
        };
        write!(f, "{prefix}: {}", self.message)
    }
}
