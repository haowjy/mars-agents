use std::collections::HashSet;

use super::{CandidateAssessment, MatchEvidence, RoutingTrace};

/// What evidence level to require for acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchPolicy {
    /// Require Confirmed or Constrained slug evidence.
    RequireSlugEvidence,
    /// Accept Passthrough (harness may or may not support the model).
    AllowPassthrough,
    /// Accept anything — only check harness is installed.
    InstalledOnly,
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

    match policy {
        MatchPolicy::InstalledOnly => Ok(()),
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

    match assessment.match_evidence {
        Some(_) => Ok(()),
        None => Err(RejectionReason::AssessmentFailed {
            harness: assessment.harness.clone(),
            skip_reason: assessment.skip_reason.map(str::to_string),
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
            match_evidence,
            harness: harness.to_string(),
            harness_order_position: None,
            candidates_tried: vec![harness.to_string()],
            assessments: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn assessment(
        harness: &str,
        installed: bool,
        match_evidence: Option<MatchEvidence>,
        skip_reason: Option<&'static str>,
    ) -> CandidateAssessment {
        CandidateAssessment {
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
    fn installed_only_accepts_any_evidence_when_installed() {
        let installed = installed(&["pi"]);
        for match_evidence in [
            MatchEvidence::Confirmed,
            MatchEvidence::Constrained,
            MatchEvidence::Passthrough,
            MatchEvidence::None,
        ] {
            assert_eq!(
                accept_route(
                    &trace("pi", match_evidence),
                    &installed,
                    MatchPolicy::InstalledOnly
                ),
                Ok(())
            );
        }
    }

    #[test]
    fn any_policy_rejects_when_harness_not_installed() {
        let installed = installed(&["codex"]);
        for policy in [
            MatchPolicy::InstalledOnly,
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
    fn allow_passthrough_rejects_only_none_evidence() {
        let installed = installed(&["pi"]);
        for match_evidence in [
            MatchEvidence::Confirmed,
            MatchEvidence::Constrained,
            MatchEvidence::Passthrough,
        ] {
            assert_eq!(
                accept_route(
                    &trace("pi", match_evidence),
                    &installed,
                    MatchPolicy::AllowPassthrough
                ),
                Ok(())
            );
        }
        assert_eq!(
            accept_route(
                &trace("pi", MatchEvidence::None),
                &installed,
                MatchPolicy::AllowPassthrough
            ),
            Err(RejectionReason::NoSlugEvidence {
                harness: "pi".to_string()
            })
        );
    }

    #[test]
    fn require_slug_evidence_rejects_passthrough_and_none() {
        let installed = installed(&["pi"]);
        for match_evidence in [MatchEvidence::Confirmed, MatchEvidence::Constrained] {
            assert_eq!(
                accept_route(
                    &trace("pi", match_evidence),
                    &installed,
                    MatchPolicy::RequireSlugEvidence
                ),
                Ok(())
            );
        }
        for match_evidence in [MatchEvidence::Passthrough, MatchEvidence::None] {
            assert_eq!(
                accept_route(
                    &trace("pi", match_evidence),
                    &installed,
                    MatchPolicy::RequireSlugEvidence
                ),
                Err(RejectionReason::NoSlugEvidence {
                    harness: "pi".to_string()
                })
            );
        }
    }

    #[test]
    fn accept_assessment_rejects_not_installed() {
        let rejection =
            accept_assessment(&assessment("claude", false, None, Some("not_installed")))
                .expect_err("assessment should reject when harness is not installed");
        assert!(rejection.is_not_installed());
        assert_eq!(
            rejection,
            RejectionReason::HarnessNotInstalled {
                harness: "claude".to_string()
            }
        );
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

    #[test]
    fn accept_assessment_accepts_any_present_evidence() {
        for match_evidence in [
            MatchEvidence::Confirmed,
            MatchEvidence::Constrained,
            MatchEvidence::Passthrough,
            MatchEvidence::None,
        ] {
            assert_eq!(
                accept_assessment(&assessment("pi", true, Some(match_evidence), None)),
                Ok(())
            );
        }
    }
}
