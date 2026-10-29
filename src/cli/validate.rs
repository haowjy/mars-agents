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

/// Run `mars validate`.
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

    // Load config to get min_mars_version for compatibility preflight.
    // This is a lightweight read that doesn't acquire the sync lock.
    let min_required: Option<String> =
        match crate::config::load_effective_project_config(&ctx.project_root) {
            Ok(effective) => effective.settings.min_mars_version,
            Err(MarsError::Config(ConfigError::NotFound { .. })) => None,
            Err(err) => return Err(err),
        };

    // Run the pipeline in dry-run mode (no writes).
    // ValidationWarnings are included in report.diagnostics by finalize().
    let report = crate::sync::execute_validation(ctx, &request)?;

    // Run compatibility preflight against the binary version and project setting.
    let binary_version = env!("CARGO_PKG_VERSION");
    let mut all_diagnostics: Vec<Diagnostic> = report.diagnostics.clone();
    if let Some(compat_diag) =
        crate::diagnostic::compatibility_preflight(binary_version, min_required.as_deref())
    {
        // Compatibility errors are prepended so they appear first.
        all_diagnostics.insert(0, compat_diag);
    }

    // Compute effective counts (--strict escalates warnings to errors).
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
        print_text_report(&all_diagnostics, args.strict);
        println!();
        if clean {
            super::output::print_success("validate: clean");
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

fn print_text_report(diagnostics: &[Diagnostic], strict: bool) {
    for diag in diagnostics {
        let level = effective_level(diag.level, strict);
        let prefix = level_str(level);
        if let Some(ctx) = &diag.context {
            eprintln!("  {prefix}[{}]: {} ({})", diag.code, diag.message, ctx);
        } else {
            eprintln!("  {prefix}[{}]: {}", diag.code, diag.message);
        }
    }
}

/// When `--strict` is active, escalate Warning to Error.
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticLevel;

    #[test]
    fn strict_mode_escalates_warning_to_error() {
        use DiagnosticLevel::{Error, Info, Warning};
        // (level, strict, effective level)
        for (level, strict, expected) in [
            (Warning, true, Error),
            (Error, true, Error),
            (Warning, false, Warning),
            (Info, true, Info),
        ] {
            assert_eq!(
                effective_level(level, strict),
                expected,
                "{level:?} strict={strict}"
            );
        }
    }
}
