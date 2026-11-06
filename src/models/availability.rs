use serde::Serialize;

use crate::models::harness_model::resolve_harness_model;
use crate::routing::{Eligibility, RoutingTrace};
use mars_schema::HarnessId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityStatus {
    Runnable,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilitySource {
    RouteRejected,
    RouteUnverified,
    HarnessInstalled,
    #[serde(rename = "pi_probe")]
    PiProbe,
    #[serde(rename = "opencode_probe")]
    OpenCodeProbe,
    #[serde(rename = "cursor_probe")]
    CursorProbe,
}

/// One selected, executable model path. Never reconstructed from a second support check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunnablePath {
    pub harness: String,
    pub mars_provider: String,
    pub harness_model_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelAvailability {
    pub status: AvailabilityStatus,
    pub source: AvailabilitySource,
    pub runnable_paths: Vec<RunnablePath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnablePathSource {
    CachedProbe,
    ProviderMatch,
    Passthrough,
}

impl RunnablePathSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::CachedProbe => "cached-probe",
            Self::ProviderMatch => "provider-match",
            Self::Passthrough => "passthrough",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnableConfidence {
    Confirmed,
    Likely,
    Unknown,
}

impl RunnableConfidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Likely => "likely",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRunnablePath {
    pub harness_model_id: String,
    /// Provider selected by the same evidence that chose the launch ID.
    pub mars_provider: Option<String>,
    pub source: RunnablePathSource,
    pub confidence: RunnableConfidence,
}

/// Project availability from the route the evaluator actually selected.
pub fn from_routing_trace(
    model_id: &str,
    provider: &str,
    trace: &RoutingTrace,
) -> ModelAvailability {
    let Some(assessment) = trace
        .assessments
        .iter()
        .find(|item| item.harness == trace.harness)
    else {
        return ModelAvailability {
            status: AvailabilityStatus::Unavailable,
            source: AvailabilitySource::RouteRejected,
            runnable_paths: Vec::new(),
        };
    };
    match assessment.eligibility() {
        Eligibility::Blocked => ModelAvailability {
            status: AvailabilityStatus::Unavailable,
            source: AvailabilitySource::RouteRejected,
            runnable_paths: Vec::new(),
        },
        Eligibility::Unverified => ModelAvailability {
            status: AvailabilityStatus::Unknown,
            source: AvailabilitySource::RouteUnverified,
            runnable_paths: Vec::new(),
        },
        Eligibility::Eligible => {
            let Some(harness) = mars_schema::harness::parse(&trace.harness) else {
                return ModelAvailability {
                    status: AvailabilityStatus::Unavailable,
                    source: AvailabilitySource::RouteRejected,
                    runnable_paths: Vec::new(),
                };
            };
            let runnable = resolve_harness_model(
                harness,
                model_id,
                assessment.chosen_slug.as_deref(),
                assessment.chosen_model.as_deref(),
                None,
                Some(provider),
            );
            let mars_provider = runnable
                .mars_provider
                .unwrap_or_else(|| provider.to_string());
            let source = match harness {
                HarnessId::Pi => AvailabilitySource::PiProbe,
                HarnessId::OpenCode => AvailabilitySource::OpenCodeProbe,
                HarnessId::Cursor => AvailabilitySource::CursorProbe,
                _ => AvailabilitySource::HarnessInstalled,
            };
            ModelAvailability {
                status: AvailabilityStatus::Runnable,
                source,
                runnable_paths: vec![RunnablePath {
                    harness: trace.harness.clone(),
                    mars_provider,
                    harness_model_id: runnable.harness_model_id,
                }],
            }
        }
    }
}
