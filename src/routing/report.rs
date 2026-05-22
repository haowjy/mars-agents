use serde::Serialize;

use crate::routing::RoutingTrace;

/// Public serialization surface for routing decisions.
/// Consumers serialize this, never `RoutingTrace` directly.
#[derive(Debug, Clone, Serialize)]
pub struct RouteDecisionReport {
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

impl RouteDecisionReport {
    pub fn from_trace(trace: &RoutingTrace) -> Self {
        Self {
            source: trace.source.label().to_string(),
            selection_kind: trace.selection_kind.label().to_string(),
            match_evidence: trace.match_evidence.label().to_string(),
            harness: trace.harness.clone(),
            harness_order_position: trace.harness_order_position,
            candidates_tried: trace.candidates_tried.clone(),
            assessments: trace
                .assessments
                .iter()
                .map(|assessment| AssessmentReport {
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
            diagnostics: trace.diagnostics.clone(),
        }
    }
}
