use std::collections::HashSet;

use super::{CandidateAssessment, MatchEvidence, RoutingTrace};

/// What evidence level to require for acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchPolicy {
    /// Require Confirmed or Constrained slug evidence.
    RequireSlugEvidence,
    /// Accept Passthrough (harness may or may not support the model).
    AllowPassthrough,
}

/// Why a route was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectionReason {
    HarnessNotInstalled {
        harness: String,
    },
    NoSlugEvidence {
        harness: String,
    },
    AssessmentFailed {
        harness: String,
        skip_reason: Option<String>,
    },
}

impl RejectionReason {
    pub fn is_not_installed(&self) -> bool {
        matches!(self, Self::HarnessNotInstalled { .. })
    }

    pub fn skip_reason(&self) -> Option<&str> {
        match self {
            Self::AssessmentFailed { skip_reason, .. } => skip_reason.as_deref(),
            _ => None,
        }
    }

    /// Whether the rejection is due to a provider constraint that makes the
    /// harness fundamentally unable to run the requested model (e.g. codex
    /// cannot run Anthropic models). Distinguished from `no_model_match` which
    /// means the harness could potentially run the provider but doesn't
    /// recognize the specific model slug.
    pub fn is_provider_constraint(&self) -> bool {
        self.skip_reason() == Some("provider_constraint_unsatisfied")
    }
}

pub fn accept_route(
    trace: &RoutingTrace,
    installed: &HashSet<String>,
    policy: MatchPolicy,
) -> Result<(), RejectionReason> {
    if !installed.contains(&trace.harness) {
        return Err(RejectionReason::HarnessNotInstalled {
            harness: trace.harness.clone(),
        });
    }

    if let Some(assessment) = trace
        .assessments
        .iter()
        .find(|assessment| assessment.harness == trace.harness)
    {
        accept_assessment(assessment)?;
    }

    match policy {
        MatchPolicy::AllowPassthrough => match trace.match_evidence {
            MatchEvidence::Confirmed | MatchEvidence::Constrained | MatchEvidence::Passthrough => {
                Ok(())
            }
            MatchEvidence::None => Err(RejectionReason::NoSlugEvidence {
                harness: trace.harness.clone(),
            }),
        },
        MatchPolicy::RequireSlugEvidence => match trace.match_evidence {
            MatchEvidence::Confirmed | MatchEvidence::Constrained => Ok(()),
            MatchEvidence::Passthrough | MatchEvidence::None => {
                Err(RejectionReason::NoSlugEvidence {
                    harness: trace.harness.clone(),
                })
            }
        },
    }
}

pub fn accept_assessment(assessment: &CandidateAssessment) -> Result<(), RejectionReason> {
    if !assessment.installed {
        return Err(RejectionReason::HarnessNotInstalled {
            harness: assessment.harness.clone(),
        });
    }

    match assessment.eligibility() {
        super::Eligibility::Eligible | super::Eligibility::Unverified => Ok(()),
        super::Eligibility::Blocked => Err(RejectionReason::AssessmentFailed {
            harness: assessment.harness.clone(),
            skip_reason: assessment.eligibility_reason().map(str::to_string),
        }),
    }
}
