//! `mars validate` — dry-run compiler that reports diagnostics without writing.
//!
//! Runs the same reader/compiler pipeline as `mars sync --diff` but stops
//! before any writes and reports diagnostics with structured output options.
//!
//! Output modes:
//! - Normal: print diagnostics, exit 0 if clean, exit 1 if errors present.
//! - `--strict`: escalate warnings to errors (missing env vars, etc.).
//! - `--json`: emit diagnostics as structured JSON for tooling.

use std::borrow::Cow;

use serde::Serialize;

use crate::cli::MarsContext;
use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticLevel};
use crate::error::ConfigError;
use crate::error::MarsError;
use crate::sync::{ResolutionMode, SyncOptions, SyncRequest};

/// Arguments for `mars validate`.
#[derive(Debug, clap::Args)]
pub struct ValidateArgs {
    /// Escalate warnings to errors (e.g., missing env vars become errors).
    #[arg(long)]
    pub strict: bool,
}

/// JSON output envelope for `mars validate --json`.
#[derive(Debug, Serialize)]
pub struct ValidateReport {
    /// Whether the validate run is clean (no errors after applying strictness rules).
    pub clean: bool,
    /// All diagnostics collected during the dry-run pipeline.
    pub diagnostics: Vec<ValidateDiagnostic>,
    /// Number of errors (after strictness escalation).
    pub error_count: usize,
    /// Number of warnings (after strictness escalation, pre-escalated warnings
    /// that became errors are NOT counted here).
    pub warning_count: usize,
}

/// A single diagnostic in JSON output.
#[derive(Debug, Serialize)]
pub struct ValidateDiagnostic {
    pub level: &'static str,
    pub code: Cow<'static, str>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<&'static str>,
}

impl ValidateDiagnostic {
    fn from_diagnostic(d: &Diagnostic, strict: bool) -> Self {
        let level = effective_level(d.level, strict);
        ValidateDiagnostic {
            level: level_str(level),
            code: d.code.clone(),
            message: d.message.clone(),
            context: d.context.clone(),
            category: d.category.map(category_str),
        }
    }
}

pub fn run(args: &ValidateArgs, ctx: &MarsContext, json: bool) -> Result<i32, MarsError> {
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: None,
        options: SyncOptions {
            dry_run: true,
            ..SyncOptions::default()
        },
        recovery: Default::default(),
        lossiness_mode: crate::diagnostic::LossinessMode::Hidden,
    };

    // Load config for the notes that only
    // validate reports (tool rules set aside or about to change). This is a lightweight read that doesn't
    // acquire the sync lock.
    let config_notes = match crate::config::load_project_config_layers(&ctx.project_root) {
        Ok(loaded) => crate::compiler::tool_notices::tool_rule_notices(&ctx.project_root, &loaded),
        Err(MarsError::Config(ConfigError::NotFound { .. })) => Vec::new(),
        Err(err) => return Err(err),
    };

    let report = crate::sync::execute_validation(ctx, &request)?;

    let mut all_diagnostics: Vec<Diagnostic> = report.diagnostics.clone();

    all_diagnostics.extend(config_notes);

    let error_count = all_diagnostics
        .iter()
        .filter(|d| effective_level(d.level, args.strict) == DiagnosticLevel::Error)
        .count();
    let warning_count = all_diagnostics
        .iter()
        .filter(|d| effective_level(d.level, args.strict) == DiagnosticLevel::Warning)
        .count();
    let clean = error_count == 0;

    if json {
        let validate_diags: Vec<ValidateDiagnostic> = all_diagnostics
            .iter()
            .map(|d| ValidateDiagnostic::from_diagnostic(d, args.strict))
            .collect();
        let validate_report = ValidateReport {
            clean,
            diagnostics: validate_diags,
            error_count,
            warning_count,
        };
        super::output::print_json(&validate_report);
    } else {
        let shown: Vec<Diagnostic> = all_diagnostics
            .iter()
            .map(|d| Diagnostic {
                level: effective_level(d.level, args.strict),
                ..d.clone()
            })
            .collect();
        super::output::print_tagged_diagnostics(&shown);
        println!();
        if clean && warning_count == 0 {
            super::output::print_success("validate: clean");
        } else if clean {
            super::output::print_warn(&format!("validate: no errors, {warning_count} warning(s)"));
        } else {
            super::output::print_error(&format!(
                "validate: {error_count} error(s){}",
                if warning_count > 0 {
                    format!(", {warning_count} warning(s)")
                } else {
                    String::new()
                }
            ));
        }
    }

    if clean { Ok(0) } else { Ok(1) }
}

fn effective_level(level: DiagnosticLevel, strict: bool) -> DiagnosticLevel {
    if strict && level == DiagnosticLevel::Warning {
        DiagnosticLevel::Error
    } else {
        level
    }
}

fn level_str(level: DiagnosticLevel) -> &'static str {
    match level {
        DiagnosticLevel::Error => "error",
        DiagnosticLevel::Warning => "warning",
        DiagnosticLevel::Info => "info",
    }
}

fn category_str(cat: DiagnosticCategory) -> &'static str {
    match cat {
        DiagnosticCategory::Compatibility => "compatibility",
        DiagnosticCategory::Lossiness => "lossiness",
        DiagnosticCategory::Validation => "validation",
        DiagnosticCategory::Advisory => "advisory",
    }
}

#[cfg(test)]
fn validation_warning_to_diagnostic(vw: &crate::validate::ValidationWarning) -> Diagnostic {
    Diagnostic {
        level: DiagnosticLevel::Warning,
        code: vw.code().into(),
        message: vw.message(),
        context: None,
        category: Some(DiagnosticCategory::Validation),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticLevel;

    fn make_diag(level: DiagnosticLevel) -> Diagnostic {
        Diagnostic {
            level,
            code: "test".into(),
            message: "test message".to_string(),
            context: None,
            category: None,
        }
    }

    #[test]
    fn strict_mode_escalates_warning_to_error() {
        let diag = make_diag(DiagnosticLevel::Warning);
        assert_eq!(effective_level(diag.level, true), DiagnosticLevel::Error);
    }

    #[test]
    fn strict_mode_leaves_error_as_error() {
        let diag = make_diag(DiagnosticLevel::Error);
        assert_eq!(effective_level(diag.level, true), DiagnosticLevel::Error);
    }

    #[test]
    fn non_strict_leaves_warning_as_warning() {
        let diag = make_diag(DiagnosticLevel::Warning);
        assert_eq!(effective_level(diag.level, false), DiagnosticLevel::Warning);
    }

    #[test]
    fn strict_mode_leaves_info_as_info() {
        let diag = make_diag(DiagnosticLevel::Info);
        assert_eq!(effective_level(diag.level, true), DiagnosticLevel::Info);
    }

    #[test]
    fn validate_diag_from_diagnostic_maps_category() {
        let diag = Diagnostic {
            level: DiagnosticLevel::Warning,
            code: "compat-version".into(),
            message: "test".to_string(),
            context: None,
            category: Some(DiagnosticCategory::Compatibility),
        };
        let vd = ValidateDiagnostic::from_diagnostic(&diag, false);
        assert_eq!(vd.level, "warning");
        assert_eq!(vd.category, Some("compatibility"));
    }

    #[test]
    fn validate_diag_strict_escalation_in_json() {
        let diag = Diagnostic {
            level: DiagnosticLevel::Warning,
            code: "missing-skill".into(),
            message: "test".to_string(),
            context: None,
            category: Some(DiagnosticCategory::Validation),
        };
        let vd = ValidateDiagnostic::from_diagnostic(&diag, true);
        assert_eq!(
            vd.level, "error",
            "warning should be escalated in strict mode"
        );
    }

    #[test]
    fn validation_warning_missing_skill_no_suggestion() {
        use crate::lock::ItemId;
        use mars_schema::item::ItemKind;
        use mars_schema::names::ItemName;
        let vw = crate::validate::ValidationWarning::MissingSkill {
            agent: ItemId {
                kind: ItemKind::Agent,
                name: ItemName::from("coder".to_string()),
            },
            skill_name: "planning".to_string(),
            suggestion: None,
        };
        let diag = validation_warning_to_diagnostic(&vw);
        assert_eq!(diag.level, DiagnosticLevel::Warning);
        assert!(diag.message.contains("coder"));
        assert!(diag.message.contains("planning"));
        assert_eq!(diag.category, Some(DiagnosticCategory::Validation));
    }

    #[test]
    fn validation_warning_missing_skill_with_suggestion() {
        use crate::lock::ItemId;
        use mars_schema::item::ItemKind;
        use mars_schema::names::ItemName;
        let vw = crate::validate::ValidationWarning::MissingSkill {
            agent: ItemId {
                kind: ItemKind::Agent,
                name: ItemName::from("coder".to_string()),
            },
            skill_name: "plan".to_string(),
            suggestion: Some("planning".to_string()),
        };
        let diag = validation_warning_to_diagnostic(&vw);
        assert!(diag.message.contains("did you mean `planning`"));
    }
}
