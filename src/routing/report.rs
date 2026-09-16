use serde::Serialize;

use crate::config::targets::{HarnessScope, LinkSource, TargetOrigin, TargetSource};
use crate::harness::registry::{self, HarnessId};
use crate::routing::RoutingTrace;

pub const ROUTE_DECISION_REPORT_VERSION: u32 = 2;

/// Public serialization surface for routing decisions.
/// Consumers serialize this, never `RoutingTrace` directly.
#[derive(Debug, Clone, Serialize)]
pub struct RouteDecisionReport {
    pub version: u32,
    pub scope: ScopeReport,
    pub model_attempts: Vec<ModelAttemptReport>,
    pub selected: Option<SelectedAssessment>,
    pub outcome: SelectionOutcome,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectionOutcome {
    Selected,
    Exhausted,
    ExplicitConstraintError,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectedAssessment {
    pub attempt_index: usize,
    pub assessment_index: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScopeReport {
    pub mode: &'static str,
    pub enabled_harnesses: Vec<String>,
    pub target_source: TargetSourceReport,
    pub excluded_harnesses: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetSourceReport {
    pub field: &'static str,
    pub origin: &'static str,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelAttemptReport {
    pub model_token: String,
    pub canonical_model: String,
    pub model_source: String,
    pub source: String,
    pub selection_kind: String,
    pub match_evidence: String,
    pub harness: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness_order_position: Option<usize>,
    pub candidates_tried: Vec<String>,
    pub assessments: Vec<AssessmentReport>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssessmentReport {
    pub verdict: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub harness: String,
    pub installed: bool,
    pub candidate_slugs: Vec<String>,
    pub filtered_slugs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen_slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_evidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

/// Compact route summary for CLI JSON.
#[derive(Debug, Clone, Serialize)]
pub struct RouteSummaryReport {
    pub harness: String,
    pub source: String,
    pub selection_kind: String,
    pub match_evidence: String,
}

impl ModelAttemptReport {
    pub fn from_trace(
        model_token: &str,
        canonical_model: &str,
        model_source: &str,
        trace: &RoutingTrace,
    ) -> Self {
        Self {
            model_token: model_token.into(),
            canonical_model: canonical_model.into(),
            model_source: model_source.into(),
            source: trace.source.label().to_string(),
            selection_kind: trace.selected_selection_kind().label().to_string(),
            match_evidence: trace.selected_match_evidence().label().to_string(),
            harness: trace.selected_harness().to_string(),
            harness_order_position: trace.selected_harness_order_position(),
            candidates_tried: trace.candidates_tried.clone(),
            assessments: trace
                .assessments
                .iter()
                .map(|assessment| AssessmentReport {
                    verdict: assessment.eligibility().label().to_string(),
                    reason: assessment.eligibility_reason().map(str::to_string),
                    harness: assessment.harness.clone(),
                    installed: assessment.installed,
                    candidate_slugs: assessment.candidate_slugs.clone(),
                    filtered_slugs: assessment.filtered_slugs.clone(),
                    chosen_slug: assessment.chosen_slug.clone(),
                    chosen_model: assessment.chosen_model.clone(),
                    match_evidence: assessment
                        .match_evidence
                        .map(|evidence| evidence.label().to_string()),
                    skip_reason: assessment.skip_reason.map(str::to_string),
                })
                .collect(),
            diagnostics: trace.selected_diagnostics().to_vec(),
        }
    }

    pub fn compact_summary(&self) -> RouteSummaryReport {
        RouteSummaryReport {
            harness: self.harness.clone(),
            source: self.source.clone(),
            selection_kind: self.selection_kind.clone(),
            match_evidence: self.match_evidence.clone(),
        }
    }
}

impl RouteDecisionReport {
    pub fn new(scope: &HarnessScope, source: &TargetSource, excluded: &[HarnessId]) -> Self {
        Self {
            version: ROUTE_DECISION_REPORT_VERSION,
            scope: ScopeReport {
                mode: if matches!(scope, HarnessScope::Unrestricted) {
                    "unrestricted"
                } else {
                    "only"
                },
                enabled_harnesses: registry::names()
                    .iter()
                    .filter(|name| scope.permits(name))
                    .map(|name| name.to_string())
                    .collect(),
                target_source: TargetSourceReport {
                    field: match source.field {
                        LinkSource::Targets => "targets",
                        LinkSource::ManagedRoot => "managed_root",
                        LinkSource::None => "none",
                    },
                    origin: match source.origin {
                        TargetOrigin::Project => "project",
                        TargetOrigin::Local => "local",
                        TargetOrigin::Unset => "unset",
                    },
                    path: source
                        .path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                },
                excluded_harnesses: registry::names()
                    .iter()
                    .filter(|name| excluded.iter().any(|id| id.as_str() == **name))
                    .map(|name| name.to_string())
                    .collect(),
            },
            model_attempts: Vec::new(),
            selected: None,
            outcome: SelectionOutcome::Exhausted,
        }
    }

    pub fn push(&mut self, token: &str, model: &str, source: &str, trace: &RoutingTrace) {
        self.model_attempts
            .push(ModelAttemptReport::from_trace(token, model, source, trace));
    }

    pub fn push_unassessed(&mut self, token: &str, model: &str, source: &str) {
        self.model_attempts.push(ModelAttemptReport {
            model_token: token.into(),
            canonical_model: model.into(),
            model_source: source.into(),
            source: String::new(),
            selection_kind: String::new(),
            match_evidence: "none".into(),
            harness: String::new(),
            harness_order_position: None,
            candidates_tried: Vec::new(),
            assessments: Vec::new(),
            diagnostics: Vec::new(),
        });
    }

    pub fn select(&mut self, attempt_index: usize) {
        let attempt = &self.model_attempts[attempt_index];
        self.selected = attempt
            .assessments
            .iter()
            .position(|assessment| {
                assessment.harness == attempt.harness && assessment.verdict != "blocked"
            })
            .map(|assessment_index| SelectedAssessment {
                attempt_index,
                assessment_index,
            });
        self.outcome = if self.selected.is_some() {
            SelectionOutcome::Selected
        } else {
            SelectionOutcome::Exhausted
        };
    }

    pub fn selected_attempt(&self) -> Option<&ModelAttemptReport> {
        self.selected
            .as_ref()
            .map(|selection| &self.model_attempts[selection.attempt_index])
    }
}

impl std::fmt::Display for RouteDecisionReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for attempt in &self.model_attempts {
            write!(
                f,
                "\nModel: {} ({})\nTried: {}",
                attempt.model_token,
                attempt.canonical_model,
                attempt.candidates_tried.join(", ")
            )?;
            for assessment in &attempt.assessments {
                if let Some(reason) = &assessment.reason {
                    let label = if assessment.verdict == "blocked" {
                        "Skip"
                    } else {
                        "Unverified"
                    };
                    write!(f, "\n{label}: {} ({reason})", assessment.harness)?;
                }
            }
            for diagnostic in &attempt.diagnostics {
                write!(f, "\n{diagnostic}")?;
            }
        }
        Ok(())
    }
}
