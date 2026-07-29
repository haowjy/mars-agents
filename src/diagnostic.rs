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
    /// Diagnostic category for tooling and structured output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<DiagnosticCategory>,
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
    /// Suppress lossiness warnings (validate, export, add, repair, …).
    #[default]
    Hidden,
    /// Surface consequential lossiness warnings plus one meridian-only summary line.
    Surface,
    /// Surface all lossiness warnings, including per-item meridian-only detail.
    Verbose,
}

impl LossinessMode {
    pub fn surfaces_lossiness(self) -> bool {
        matches!(self, LossinessMode::Surface | LossinessMode::Verbose)
    }
}

/// One launch-time field omitted from a native artifact but enforced by Meridian at spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MeridianOnlyRecord {
    item_kind: String,
    item_name: String,
    target: String,
    field: String,
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
}

/// Collects diagnostics during pipeline execution.
pub struct DiagnosticCollector {
    diagnostics: Vec<Diagnostic>,
    engine_fallbacks: Vec<crate::resolve::EngineFallback>,
    lossiness_mode: LossinessMode,
    /// Launch-time / meridian-enforced field mappings accumulated for summary or verbose detail.
    meridian_only_records: Vec<MeridianOnlyRecord>,
}

impl DiagnosticCollector {
    pub fn new() -> Self {
        Self::with_lossiness_mode(LossinessMode::default())
    }

    pub fn with_lossiness_mode(lossiness_mode: LossinessMode) -> Self {
        Self {
            diagnostics: Vec::new(),
            engine_fallbacks: Vec::new(),
            lossiness_mode,
            meridian_only_records: Vec::new(),
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

    /// Record a launch-time field mapping for meridian-only summary or verbose detail.
    pub fn record_meridian_only_field(
        &mut self,
        item_kind: &str,
        item_name: &str,
        target: &str,
        field: &str,
    ) {
        if !self.lossiness_mode.surfaces_lossiness() {
            return;
        }
        self.meridian_only_records.push(MeridianOnlyRecord {
            item_kind: item_kind.to_string(),
            item_name: item_name.to_string(),
            target: target.to_string(),
            field: field.to_string(),
        });
    }

    fn should_emit_lossiness(&self) -> bool {
        self.lossiness_mode.surfaces_lossiness()
    }

    pub fn error(&mut self, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Error,
            code,
            message: message.into(),
            context: None,
            category: None,
        });
    }

    pub fn error_with_category(
        &mut self,
        code: &'static str,
        message: impl Into<String>,
        category: DiagnosticCategory,
    ) {
        if category == DiagnosticCategory::Lossiness && !self.should_emit_lossiness() {
            return;
        }
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Error,
            code,
            message: message.into(),
            context: None,
            category: Some(category),
        });
    }

    pub fn warn(&mut self, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            code,
            message: message.into(),
            context: None,
            category: None,
        });
    }

    pub fn warn_with_category(
        &mut self,
        code: &'static str,
        message: impl Into<String>,
        category: DiagnosticCategory,
    ) {
        if category == DiagnosticCategory::Lossiness && !self.should_emit_lossiness() {
            return;
        }
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            code,
            message: message.into(),
            context: None,
            category: Some(category),
        });
    }

    pub fn info(&mut self, code: &'static str, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Info,
            code,
            message: message.into(),
            context: None,
            category: None,
        });
    }

    pub fn info_with_category(
        &mut self,
        code: &'static str,
        message: impl Into<String>,
        category: DiagnosticCategory,
    ) {
        if category == DiagnosticCategory::Lossiness && !self.should_emit_lossiness() {
            return;
        }
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Info,
            code,
            message: message.into(),
            context: None,
            category: Some(category),
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
            category: None,
        });
    }

    pub fn extend(&mut self, diagnostics: Vec<Diagnostic>) {
        self.diagnostics.extend(diagnostics);
    }

    pub fn drain(&mut self) -> Vec<Diagnostic> {
        self.flush_meridian_only_lossiness();
        std::mem::take(&mut self.diagnostics)
    }

    fn flush_meridian_only_lossiness(&mut self) {
        if self.meridian_only_records.is_empty() || !self.lossiness_mode.surfaces_lossiness() {
            return;
        }
        match self.lossiness_mode {
            LossinessMode::Surface => {
                let count = self.meridian_only_records.len();
                let noun = if count == 1 { "mapping" } else { "mappings" };
                self.info_with_category(
                    "launch-time-field-summary",
                    format!(
                        "{count} launch-time field {noun} handled by meridian at spawn — run with --verbose for detail"
                    ),
                    DiagnosticCategory::Lossiness,
                );
            }
            LossinessMode::Verbose => {
                use std::collections::BTreeMap;
                let mut grouped: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
                for record in &self.meridian_only_records {
                    grouped
                        .entry((
                            record.item_kind.clone(),
                            record.item_name.clone(),
                            record.target.clone(),
                        ))
                        .or_default()
                        .push(record.field.clone());
                }
                for ((item_kind, item_name, target), mut fields) in grouped {
                    fields.sort();
                    fields.dedup();
                    let field_refs: Vec<&str> = fields.iter().map(String::as_str).collect();
                    let count = field_refs.len();
                    let noun = if count == 1 { "field" } else { "fields" };
                    let target_label = format!(".{}", target.to_lowercase());
                    let code = if item_kind == "skill" {
                        "skill-field-meridian-only"
                    } else {
                        "agent-field-meridian-only"
                    };
                    self.warn_with_category(
                        code,
                        format!(
                            "{item_kind} `{item_name}`: {count} {noun} not lowered (meridian-only) for {target_label} ({})",
                            field_refs.join(", ")
                        ),
                        DiagnosticCategory::Lossiness,
                    );
                }
            }
            LossinessMode::Hidden => {}
        }
        self.meridian_only_records.clear();
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
                    code: "compat-version",
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
                code: "compat-version-parse",
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
fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
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
    fn collector_suppresses_lossiness_when_hidden() {
        let mut coll = DiagnosticCollector::with_lossiness_mode(LossinessMode::Hidden);
        coll.warn_with_category(
            "agent-field-dropped",
            "agent `x`: dropped",
            DiagnosticCategory::Lossiness,
        );
        assert!(coll.drain().is_empty());

        let mut coll = DiagnosticCollector::with_lossiness_mode(LossinessMode::Surface);
        coll.warn_with_category(
            "agent-field-dropped",
            "agent `x`: dropped",
            DiagnosticCategory::Lossiness,
        );
        assert_eq!(coll.drain().len(), 1);
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

    #[test]
    fn collector_suppresses_info_lossiness_when_hidden() {
        let mut coll = DiagnosticCollector::with_lossiness_mode(LossinessMode::Hidden);
        coll.info_with_category(
            "field-approximate",
            "field `x`: approximate",
            DiagnosticCategory::Lossiness,
        );
        assert!(coll.drain().is_empty());

        let mut coll = DiagnosticCollector::with_lossiness_mode(LossinessMode::Surface);
        coll.info_with_category(
            "field-approximate",
            "field `x`: approximate",
            DiagnosticCategory::Lossiness,
        );
        let diags = coll.drain();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagnosticLevel::Info);
        assert_eq!(diags[0].category, Some(DiagnosticCategory::Lossiness));
    }

    #[test]
    fn collector_verbose_surfaces_meridian_only_detail() {
        let mut coll = DiagnosticCollector::with_lossiness_mode(LossinessMode::Verbose);
        coll.record_meridian_only_field("agent", "coder", "Claude", "approval");
        let diags = coll.drain();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "agent-field-meridian-only");
    }

    #[test]
    fn collector_surface_emits_meridian_only_summary() {
        let mut coll = DiagnosticCollector::with_lossiness_mode(LossinessMode::Surface);
        coll.record_meridian_only_field("agent", "coder", "Claude", "approval");
        let diags = coll.drain();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "launch-time-field-summary");
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
            code: "test",
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
            code: "test",
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
