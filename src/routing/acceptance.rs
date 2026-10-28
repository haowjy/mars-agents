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

/// Check whether a routing trace meets the given acceptance policy.
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

/// Check whether a single candidate assessment is acceptable.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::{RouteSource, SelectionKind};

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn trace(harness: &str, match_evidence: MatchEvidence) -> RoutingTrace {
        RoutingTrace {
            source: RouteSource::Provider,
            selection_kind: SelectionKind::Auto,
            selected_by_preference: false,
            match_evidence,
            harness: harness.to_string(),
            harness_order_position: None,
            candidates_tried: vec![harness.to_string()],
            assessments: Vec::new(),
            diagnostics: Vec::new(),
            exhaustion_reason: None,
        }
    }

    fn assessment(
        harness: &str,
        installed: bool,
        match_evidence: Option<MatchEvidence>,
        skip_reason: Option<&'static str>,
    ) -> CandidateAssessment {
        CandidateAssessment {
            auth: None,
            harness: harness.to_string(),
            installed,
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            match_evidence,
            skip_reason,
        }
    }

    #[test]
    fn any_policy_rejects_when_harness_not_installed() {
        let installed = installed(&["codex"]);
        for policy in [
            MatchPolicy::AllowPassthrough,
            MatchPolicy::RequireSlugEvidence,
        ] {
            assert_eq!(
                accept_route(&trace("pi", MatchEvidence::Confirmed), &installed, policy),
                Err(RejectionReason::HarnessNotInstalled {
                    harness: "pi".to_string()
                })
            );
        }
    }

    #[test]
    fn match_policy_decides_which_evidence_is_accepted() {
        let installed = installed(&["pi"]);
        for (policy, accepted, rejected) in [
            (
                MatchPolicy::AllowPassthrough,
                &[
                    MatchEvidence::Confirmed,
                    MatchEvidence::Constrained,
                    MatchEvidence::Passthrough,
                ][..],
                &[MatchEvidence::None][..],
            ),
            (
                MatchPolicy::RequireSlugEvidence,
                &[MatchEvidence::Confirmed, MatchEvidence::Constrained][..],
                &[MatchEvidence::Passthrough, MatchEvidence::None][..],
            ),
        ] {
            for match_evidence in accepted {
                assert_eq!(
                    accept_route(&trace("pi", *match_evidence), &installed, policy),
                    Ok(()),
                    "{policy:?} with {match_evidence:?}"
                );
            }
            for match_evidence in rejected {
                assert_eq!(
                    accept_route(&trace("pi", *match_evidence), &installed, policy),
                    Err(RejectionReason::NoSlugEvidence {
                        harness: "pi".to_string()
                    }),
                    "{policy:?} with {match_evidence:?}"
                );
            }
        }
    }

    #[test]
    fn accept_assessment_rejects_missing_evidence_with_skip_reason() {
        assert_eq!(
            accept_assessment(&assessment(
                "codex",
                true,
                None,
                Some("provider_constraint_unsatisfied")
            )),
            Err(RejectionReason::AssessmentFailed {
                harness: "codex".to_string(),
                skip_reason: Some("provider_constraint_unsatisfied".to_string())
            })
        );
    }
}
