use std::borrow::Cow;

use serde::Serialize;

use crate::compiler::lossiness::{LossRecord, LossyField};

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    /// Machine-readable code, e.g. "shadow-collision", "manifest-path-dep". A schema
    /// diagnostic's code passes through unchanged.
    pub code: Cow<'static, str>,
    /// Human-readable message.
    pub message: String,
    /// Optional context (source name, item path, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Diagnostic category for tooling and structured output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<DiagnosticCategory>,
}

impl Diagnostic {
    /// Whether this diagnostic means the sync may have fallen short of the state it wanted
    /// (a failed write, an edit left in place), so `already up to date` is not printed.
    /// Every warning counts unless it is advisory or lossiness.
    pub fn blocks_up_to_date(&self) -> bool {
        self.level == DiagnosticLevel::Warning
            && !matches!(
                self.category,
                Some(DiagnosticCategory::Advisory | DiagnosticCategory::Lossiness)
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Info,
}

/// Whether lossiness diagnostics are collected and surfaced to the user.
///
/// Shared by the sync pipeline (`SyncRequest`) and the check/init preview path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LossinessMode {
    /// Suppress lossiness warnings (validate, export, repair, …).
    #[default]
    Hidden,
    /// Default for sync, check, init and plugin builds: one line for the project covering the
    /// restrictions an agent loses (`Lossiness::Dropped`).
    Surface,
    /// Every lossy field per item and target: quiet, approximate and launch-time ones too.
    Verbose,
}

impl LossinessMode {
    pub fn surfaces_lossiness(self) -> bool {
        self != LossinessMode::Hidden
    }

    /// Whether every lossy field prints per item, not only the project line for dropped ones.
    pub fn verbose(self) -> bool {
        self == LossinessMode::Verbose
    }
}

/// Broad category for a diagnostic — used in structured output and validation gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticCategory {
    /// Compatibility and version requirement issues.
    Compatibility,
    /// Lossiness during lowering to a target (dropped/approximate fields).
    Lossiness,
    /// Schema validation and structural checks.
    Validation,
    /// A warning that says nothing about whether the sync reached the state it wanted
    /// (an unset env var, a skipped unknown event). It does not stop `already up to date`.
    Advisory,
}

/// Collects diagnostics during pipeline execution.
pub struct DiagnosticCollector {
    diagnostics: Vec<Diagnostic>,
    engine_fallbacks: Vec<crate::resolve::EngineFallback>,
    lossiness_mode: LossinessMode,
    /// The command that re-runs this one with `--verbose`, named in the default lossiness line.
    rerun_verbose: &'static str,
    /// Every lossy field recorded so far; `drain` renders them for `lossiness_mode`.
    lossy_records: Vec<LossRecord>,
    /// Files that carry each repeated `field.unknown` warning, by the warning's own text.
    unknown_field_sites: std::collections::HashMap<String, Vec<String>>,
}

impl DiagnosticCollector {
    pub fn new() -> Self {
        Self::with_lossiness_mode(LossinessMode::default())
    }

    /// Name the command whose `--verbose` shows the detail behind the default lossiness line,
    /// for a caller that is not `mars sync`.
    pub fn rerun_verbose_with(mut self, command: &'static str) -> Self {
        self.rerun_verbose = command;
        self
    }

    pub fn with_lossiness_mode(lossiness_mode: LossinessMode) -> Self {
        Self {
            diagnostics: Vec::new(),
            engine_fallbacks: Vec::new(),
            lossiness_mode,
            rerun_verbose: "mars sync --verbose",
            lossy_records: Vec::new(),
            unknown_field_sites: std::collections::HashMap::new(),
        }
    }

    pub(crate) fn schema(
        &mut self,
        diagnostic: &mars_schema::diagnostic::SchemaDiagnostic,
        context: &str,
        errors_as_warnings: bool,
    ) {
        use mars_schema::diagnostic::Severity;

        let code = Cow::Owned(diagnostic.code.clone());
        let message = format!("{context}: {}", diagnostic.message);
        if self
            .diagnostics
            .iter()
            .any(|existing| existing.code == code && existing.message == message)
        {
            return;
        }
        if diagnostic.severity == Severity::Error && !errors_as_warnings {
            self.error_with_category(code, message, DiagnosticCategory::Validation);
        } else if diagnostic.severity == Severity::Info {
            self.info(code, message);
        } else if diagnostic.code == "field.unknown" {
            self.warn_unknown_field(code, &diagnostic.message, context);
        } else {
            self.warn_advisory(code, message);
        }
    }

    /// One warning per unknown field name, listing every file that carries it: the first file
    /// is reported as an ordinary warning, and each further one extends the same warning.
    fn warn_unknown_field(&mut self, code: Cow<'static, str>, text: &str, context: &str) {
        let sites = self.unknown_field_sites.entry(text.to_owned()).or_default();
        if sites.iter().any(|site| site == context) {
            return;
        }
        sites.push(context.to_owned());
        if sites.len() == 1 {
            self.warn_advisory(code.clone(), format!("{context}: {text}"));
            return;
        }
        let message = format!("{text} (in {} files: {})", sites.len(), sites.join(", "));
        let first = format!("{}: {text}", sites[0]);
        if let Some(existing) = self.diagnostics.iter_mut().find(|existing| {
            existing.code == code
                && (existing.message == first || existing.message.starts_with(text))
        }) {
            existing.message = message;
        }
    }

    pub(crate) fn record_engine_fallback(&mut self, fallback: crate::resolve::EngineFallback) {
        if let Some(existing) = self
            .engine_fallbacks
            .iter_mut()
            .find(|existing| existing.source == fallback.source)
        {
            for skipped in fallback.skipped {
                if let Some(recorded) = existing
                    .skipped
                    .iter_mut()
                    .find(|recorded| recorded.version == skipped.version)
                {
                    *recorded = skipped;
                } else {
                    existing.skipped.push(skipped);
                }
            }
            for engine in fallback.engines {
                if !existing.engines.contains(&engine) {
                    existing.engines.push(engine);
                }
            }
        } else {
            self.engine_fallbacks.push(fallback);
        }
    }

    pub(crate) fn reconcile_engine_fallbacks(&mut self, graph: &crate::resolve::ResolvedGraph) {
        self.engine_fallbacks.retain_mut(|fallback| {
            let Some(node) = graph.nodes.get(fallback.source.as_str()) else {
                return false;
            };
            fallback.selected_version = node
                .resolved_ref
                .version
                .as_ref()
                .map(ToString::to_string)
                .or_else(|| node.resolved_ref.version_tag.clone())
                .unwrap_or_else(|| "HEAD/path".to_string());
            true
        });
    }

    pub(crate) fn take_engine_fallbacks(&mut self) -> Vec<crate::resolve::EngineFallback> {
        std::mem::take(&mut self.engine_fallbacks)
    }

    /// Buffer one lowered item's lossy fields; `drain` renders them by mode.
    pub fn record_lossy_fields(
        &mut self,
        item_kind: &'static str,
        item_name: &str,
        fields: &[LossyField],
    ) {
        if !self.lossiness_mode.surfaces_lossiness() {
            return;
        }
        self.lossy_records
            .extend(fields.iter().map(|field| LossRecord {
                item_kind,
                item_name: item_name.to_string(),
                field: field.clone(),
            }));
    }

    pub fn error(&mut self, code: impl Into<Cow<'static, str>>, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Error,
            code: code.into(),
            message: message.into(),
            context: None,
            category: None,
        });
    }

    pub fn error_with_category(
        &mut self,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
        category: DiagnosticCategory,
    ) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Error,
            code: code.into(),
            message: message.into(),
            context: None,
            category: Some(category),
        });
    }

    pub fn warn(&mut self, code: impl Into<Cow<'static, str>>, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            code: code.into(),
            message: message.into(),
            context: None,
            category: None,
        });
    }

    /// A warning that does not imply the sync fell short of the state it wanted. Every other
    /// warning does: `already up to date` is withheld while one is present, so an unmarked
    /// site costs a reassuring line, never a false one.
    pub fn warn_advisory(
        &mut self,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
    ) {
        self.warn_with_category(code, message, DiagnosticCategory::Advisory);
    }

    pub(crate) fn warn_once(&mut self, code: &'static str, message: impl Into<String>) {
        let message = message.into();
        if !self.has(code, &message) {
            self.warn(code, message);
        }
    }

    pub(crate) fn warn_advisory_once(&mut self, code: &'static str, message: impl Into<String>) {
        let message = message.into();
        if !self.has(code, &message) {
            self.warn_advisory(code, message);
        }
    }

    fn has(&self, code: &str, message: &str) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code && diagnostic.message == message)
    }

    pub fn warn_with_category(
        &mut self,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
        category: DiagnosticCategory,
    ) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            code: code.into(),
            message: message.into(),
            context: None,
            category: Some(category),
        });
    }

    pub fn info(&mut self, code: impl Into<Cow<'static, str>>, message: impl Into<String>) {
        let message = message.into();
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Info,
            code: code.into(),
            message,
            context: None,
            category: None,
        });
    }

    pub(crate) fn info_once(&mut self, code: &'static str, message: impl Into<String>) {
        let message = message.into();
        if self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code && diagnostic.message == message)
        {
            return;
        }
        self.info(code, message);
    }

    pub fn warn_with_context(
        &mut self,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
        context: impl Into<String>,
    ) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            code: code.into(),
            message: message.into(),
            context: Some(context.into()),
            category: None,
        });
    }

    pub fn extend(&mut self, diagnostics: Vec<Diagnostic>) {
        self.diagnostics.extend(diagnostics);
    }

    pub(crate) fn fail_on_errors(&self) -> Result<(), crate::error::MarsError> {
        let errors = self
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
            .map(|diagnostic| format!("[{}] {}", diagnostic.code, diagnostic.message))
            .collect::<Vec<_>>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(crate::error::ConfigError::Invalid {
                message: errors.join("\n"),
            }
            .into())
        }
    }

    pub fn drain(&mut self) -> Vec<Diagnostic> {
        let records = std::mem::take(&mut self.lossy_records);
        self.diagnostics.extend(crate::compiler::lossiness::render(
            &records,
            self.lossiness_mode,
            self.rerun_verbose,
        ));
        std::mem::take(&mut self.diagnostics)
    }
}

impl Default for DiagnosticCollector {
    fn default() -> Self {
        Self::with_lossiness_mode(LossinessMode::default())
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let prefix = match self.level {
            DiagnosticLevel::Error => "error",
            DiagnosticLevel::Warning => "warning",
            DiagnosticLevel::Info => "info",
        };
        write!(f, "{prefix}: {}", self.message)
    }
}

/// Compatibility preflight: check whether the current binary version satisfies a
/// `min_mars_version` requirement declared by the project's mars.toml.
///
/// Returns `None` if compatible (or no requirement is declared).
/// Returns `Some(Diagnostic)` with `Error` level if the binary is too old.
///
/// Rule:
/// - `min_required` is `None` → always compatible (old package without requirement)
/// - `binary_version >= min_required` → compatible
/// - `binary_version < min_required` → error: binary too old
pub fn compatibility_preflight(
    binary_version: &str,
    min_required: Option<&str>,
) -> Option<Diagnostic> {
    let min = min_required?;

    // Parse as semver. If either fails to parse, accept and emit a warning instead.
    let bin_ver = parse_semver(binary_version);
    let req_ver = parse_semver(min);

    match (bin_ver, req_ver) {
        (Some(bin), Some(req)) => {
            if bin < req {
                Some(Diagnostic {
                    level: DiagnosticLevel::Error,
                    code: "compat-version".into(),
                    message: format!(
                        "this project requires mars >= {min} but the installed binary is {binary_version}; \
                         upgrade with: cargo install mars-agents"
                    ),
                    context: None,
                    category: Some(DiagnosticCategory::Compatibility),
                })
            } else {
                None
            }
        }
        _ => {
            // Unparseable version strings — warn and continue (forward compat: new package,
            // unknown version scheme → don't hard-block the consumer).
            Some(Diagnostic {
                level: DiagnosticLevel::Warning,
                code: "compat-version-parse".into(),
                message: format!(
                    "could not compare mars version `{binary_version}` against requirement `{min}`; \
                     proceeding with defaults"
                ),
                context: None,
                category: Some(DiagnosticCategory::Compatibility),
            })
        }
    }
}

/// Minimal semver parser: returns `(major, minor, patch)` tuple from "X.Y.Z" or "vX.Y.Z".
pub(crate) fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim_start_matches('v');
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() < 3 {
        return None;
    }
    let major = parts[0].parse::<u64>().ok()?;
    let minor = parts[1].parse::<u64>().ok()?;
    // Allow patch to have pre-release suffix like "1-beta.1"
    let patch_str = parts[2].split('-').next().unwrap_or(parts[2]);
    let patch = patch_str.parse::<u64>().ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_requirement_always_compatible() {
        let diag = compatibility_preflight("0.5.0", None);
        assert!(diag.is_none());
    }

    #[test]
    fn binary_meets_requirement() {
        let diag = compatibility_preflight("1.2.0", Some("1.0.0"));
        assert!(diag.is_none());
    }

    #[test]
    fn binary_exactly_meets_requirement() {
        let diag = compatibility_preflight("1.0.0", Some("1.0.0"));
        assert!(diag.is_none());
    }

    #[test]
    fn binary_too_old_produces_error() {
        let diag = compatibility_preflight("0.5.0", Some("1.0.0")).unwrap();
        assert_eq!(diag.level, DiagnosticLevel::Error);
        assert_eq!(diag.code, "compat-version");
        assert_eq!(diag.category, Some(DiagnosticCategory::Compatibility));
        assert!(diag.message.contains("0.5.0"));
        assert!(diag.message.contains("1.0.0"));
    }

    #[test]
    fn binary_v_prefix_handled() {
        let diag = compatibility_preflight("v1.2.0", Some("v1.0.0"));
        assert!(diag.is_none());
    }

    #[test]
    fn binary_v_prefix_too_old() {
        let diag = compatibility_preflight("v0.9.0", Some("v1.0.0")).unwrap();
        assert_eq!(diag.level, DiagnosticLevel::Error);
    }

    #[test]
    fn unparseable_version_produces_warning_not_error() {
        let diag = compatibility_preflight("dev", Some("1.0.0")).unwrap();
        assert_eq!(diag.level, DiagnosticLevel::Warning);
        assert_eq!(diag.code, "compat-version-parse");
    }

    #[test]
    fn unparseable_requirement_produces_warning() {
        let diag = compatibility_preflight("1.0.0", Some("latest")).unwrap();
        assert_eq!(diag.level, DiagnosticLevel::Warning);
    }

    #[test]
    fn collector_merges_engine_fallbacks_by_source_and_deduplicates_details() {
        use crate::resolve::{
            EngineFallback, EngineFallbackRequirement, EngineFallbackSkippedVersion,
        };

        let mut coll = DiagnosticCollector::new();
        coll.record_engine_fallback(EngineFallback {
            source: "pkg".to_string(),
            skipped: vec![EngineFallbackSkippedVersion {
                version: "3.0.0".to_string(),
                requirements: vec![EngineFallbackRequirement {
                    engine: "mars".to_string(),
                    requirement: ">=3".to_string(),
                }],
            }],
            selected_version: "2.0.0".to_string(),
            engines: vec!["mars".to_string()],
        });
        coll.record_engine_fallback(EngineFallback {
            source: "pkg".to_string(),
            skipped: vec![
                EngineFallbackSkippedVersion {
                    version: "3.0.0".to_string(),
                    requirements: vec![EngineFallbackRequirement {
                        engine: "mars".to_string(),
                        requirement: ">=4".to_string(),
                    }],
                },
                EngineFallbackSkippedVersion {
                    version: "2.0.0".to_string(),
                    requirements: vec![EngineFallbackRequirement {
                        engine: "meridian".to_string(),
                        requirement: ">=2".to_string(),
                    }],
                },
            ],
            selected_version: "1.0.0".to_string(),
            engines: vec!["mars".to_string(), "meridian".to_string()],
        });

        let fallbacks = coll.take_engine_fallbacks();
        assert_eq!(fallbacks.len(), 1);
        assert_eq!(
            fallbacks[0]
                .skipped
                .iter()
                .map(|skipped| skipped.version.as_str())
                .collect::<Vec<_>>(),
            vec!["3.0.0", "2.0.0"]
        );
        assert_eq!(fallbacks[0].skipped[0].requirements[0].requirement, ">=4");
        assert_eq!(fallbacks[0].engines, vec!["mars", "meridian"]);
    }

    fn meridian_only(field: &str) -> LossyField {
        LossyField {
            field: field.into(),
            target: "Claude".into(),
            classification: crate::compiler::lossiness::Lossiness::MeridianOnly,
        }
    }

    #[test]
    fn collector_verbose_surfaces_meridian_only_detail() {
        let mut coll = DiagnosticCollector::with_lossiness_mode(LossinessMode::Verbose);
        coll.record_lossy_fields("agent", "coder", &[meridian_only("approval")]);
        let diags = coll.drain();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "agent-field-meridian-only");
    }

    #[test]
    fn collector_surface_is_silent_about_meridian_only_fields() {
        let mut coll = DiagnosticCollector::with_lossiness_mode(LossinessMode::Surface);
        coll.record_lossy_fields("agent", "coder", &[meridian_only("approval")]);
        let diags = coll.drain();
        assert!(diags.is_empty());
    }

    #[test]
    fn collector_error_with_category() {
        let mut coll = DiagnosticCollector::new();
        coll.error_with_category(
            "compat-version",
            "too old",
            DiagnosticCategory::Compatibility,
        );
        let diags = coll.drain();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagnosticLevel::Error);
        assert_eq!(diags[0].category, Some(DiagnosticCategory::Compatibility));
    }

    #[test]
    fn display_shows_error_prefix() {
        let d = Diagnostic {
            level: DiagnosticLevel::Error,
            code: "test".into(),
            message: "something broke".to_string(),
            context: None,
            category: None,
        };
        assert_eq!(d.to_string(), "error: something broke");
    }

    #[test]
    fn display_shows_warning_prefix() {
        let d = Diagnostic {
            level: DiagnosticLevel::Warning,
            code: "test".into(),
            message: "heads up".to_string(),
            context: None,
            category: None,
        };
        assert_eq!(d.to_string(), "warning: heads up");
    }

    #[test]
    fn patch_with_prerelease_suffix_parsed() {
        // "1.2.3-beta.1" → (1, 2, 3)
        let v = parse_semver("1.2.3-beta.1").unwrap();
        assert_eq!(v, (1, 2, 3));
    }
}
