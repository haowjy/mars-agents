use crate::config::targets::HarnessScope;
use crate::harness::host::{AuthState, ListingEvidence, ListingEvidenceSet};
use crate::harness::registry;
use crate::harness::registry::{HarnessClass, ListingAuth};
use mars_schema::HarnessId;
use std::collections::HashSet;

pub mod acceptance;
pub mod evidence;
pub mod probe_match;
pub mod report;
pub mod slug;

pub(crate) use probe_match::{SlugSelection, select_probe_slug};

use crate::models;
use crate::models::probes::CursorProbeResult;
use crate::models::probes::OpenCodeProbeResult;
use crate::models::probes::PiProbeResult;

pub use evidence::{RoutingEvidence, RoutingSettingsEvidence};

/// How the harness was selected — orthogonal to slug evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    Auto,
    Fixed,
}

impl SelectionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fixed => "fixed",
        }
    }
}

/// Slug evidence the evaluator found for this harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchEvidence {
    Confirmed,
    Constrained,
    Passthrough,
    None,
}

impl MatchEvidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Constrained => "constrained",
            Self::Passthrough => "passthrough",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExhaustionReason {
    LinkedHarnessConstraints,
}

/// How the harness was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSource {
    Cli,
    Overlay,
    OverlayModelPolicy,
    ProfileModelPolicy,
    SettingsModelPolicy,
    Profile,
    Alias,
    ConfigOrder,
    ConfigDefault,
    Provider,
}

impl RouteSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Overlay => "overlay",
            Self::OverlayModelPolicy => "overlay-model-policy",
            Self::ProfileModelPolicy => "profile-model-policy",
            Self::SettingsModelPolicy => "settings-model-policy",
            Self::Profile => "profile",
            Self::Alias => "alias",
            Self::ConfigOrder => "config-order",
            Self::ConfigDefault => "config",
            Self::Provider => "provider",
        }
    }
}

/// Assessment of one candidate harness.
#[derive(Debug, Clone)]
pub struct CandidateAssessment {
    /// None when permission, installation, or support prevented an auth check.
    pub auth: Option<crate::harness::host::AuthState>,
    pub harness: String,
    pub installed: bool,
    pub candidate_slugs: Vec<String>,
    pub filtered_slugs: Vec<String>,
    pub chosen_slug: Option<String>,
    pub chosen_model: Option<String>,
    pub match_evidence: Option<MatchEvidence>,
    pub skip_reason: Option<&'static str>,
}

/// Runtime eligibility is separate from model support evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    Eligible,
    Unverified,
    Blocked,
}

impl Eligibility {
    pub fn label(self) -> &'static str {
        match self {
            Self::Eligible => "eligible",
            Self::Unverified => "unverified",
            Self::Blocked => "blocked",
        }
    }
}

impl CandidateAssessment {
    pub fn eligibility(&self) -> Eligibility {
        if !self.installed
            || self.skip_reason.is_some()
            || self.auth == Some(crate::harness::host::AuthState::Unauthenticated)
            || matches!(self.match_evidence, None | Some(MatchEvidence::None))
        {
            Eligibility::Blocked
        } else if self.match_evidence == Some(MatchEvidence::Passthrough)
            || !self
                .auth
                .as_ref()
                .is_some_and(AuthState::is_runtime_evidence)
        {
            Eligibility::Unverified
        } else {
            Eligibility::Eligible
        }
    }

    pub fn eligibility_reason(&self) -> Option<&'static str> {
        match self.eligibility() {
            Eligibility::Eligible => None,
            Eligibility::Unverified if self.match_evidence == Some(MatchEvidence::Passthrough) => {
                Some("support_unknown")
            }
            Eligibility::Unverified => Some(match self.auth {
                Some(AuthState::ListingFailed) => "auth_listing_failed",
                Some(AuthState::Unknown { .. }) => "auth_unknown",
                _ => "auth_unchecked",
            }),
            Eligibility::Blocked => Some(match self.skip_reason {
                Some("pi_incompatible" | "unsupported_candidate") => "incompatible_harness",
                Some(reason) => reason,
                None if !self.installed => "not_installed",
                None if self.auth == Some(crate::harness::host::AuthState::Unauthenticated) => {
                    "native_auth_unavailable"
                }
                None => "no_model_match",
            }),
        }
    }
}

/// Full routing trace for diagnostics/provenance.
#[derive(Debug, Clone)]
pub struct RoutingTrace {
    pub source: RouteSource,
    pub selection_kind: SelectionKind,
    pub selected_by_preference: bool,
    pub match_evidence: MatchEvidence,
    pub harness: String,
    pub harness_order_position: Option<usize>,
    pub candidates_tried: Vec<String>,
    pub assessments: Vec<CandidateAssessment>,
    pub diagnostics: Vec<String>,
    pub exhaustion_reason: Option<ExhaustionReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedChosenSlugEvidence {
    pub slug: String,
    pub match_evidence: Option<MatchEvidence>,
}

impl RoutingTrace {
    pub fn selected_harness(&self) -> &str {
        &self.harness
    }

    pub fn selected_selection_kind(&self) -> SelectionKind {
        self.selection_kind
    }

    pub fn selected_match_evidence(&self) -> MatchEvidence {
        self.match_evidence
    }

    pub fn selected_diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    pub fn selected_harness_order_position(&self) -> Option<usize> {
        self.harness_order_position
    }
}

/// Input to the routing engine.
pub struct RoutingInput<'a> {
    pub preferred_harness: Option<(&'a str, RouteSource)>,
    pub model_id: &'a str,
    pub provider_for_order: Option<&'a str>,
    pub provider_constraint: Option<&'a str>,
    pub settings_provider_order: Option<&'a [String]>,
    pub settings_harness_order: Option<&'a [String]>,
    pub config_default_harness: Option<&'a str>,
    pub installed_harnesses: &'a HashSet<String>,
    pub harness_scope: HarnessScope,
    pub excluded_harnesses: &'a [HarnessId],
    pub opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    pub pi_probe_result: Option<&'a PiProbeResult>,
    pub cursor_probe_result: Option<&'a CursorProbeResult>,
    /// Cached catalog slugs (`provider/model`) for full model-id matching on native harnesses.
    pub catalog_model_slugs: Option<&'a [String]>,
}

/// Permission is checked before installation, authentication, or support evidence.
/// Keep caller exclusions separate from configured scope for diagnostics.
pub fn permission_denial(
    scope: &HarnessScope,
    excluded: &[HarnessId],
    harness: &str,
) -> Option<&'static str> {
    if !scope.permits(harness) {
        Some("disabled_target")
    } else if mars_schema::harness::parse(harness).is_some_and(|id| excluded.contains(&id)) {
        Some("excluded_by_caller")
    } else {
        None
    }
}

pub trait ProbeResolver {
    fn opencode_probe_result(&mut self) -> Option<OpenCodeProbeResult>;
    fn pi_probe_result(&mut self) -> Option<PiProbeResult>;
    fn cursor_probe_result(&mut self) -> Option<CursorProbeResult>;
    fn listing_evidence(&mut self, harness: HarnessId) -> ListingEvidence;
}

#[derive(Debug, Default)]
struct StaticProbeResolver {
    opencode_probe_result: Option<OpenCodeProbeResult>,
    pi_probe_result: Option<PiProbeResult>,
    cursor_probe_result: Option<CursorProbeResult>,
}

impl StaticProbeResolver {
    fn from_input(input: &RoutingInput<'_>) -> Self {
        Self {
            opencode_probe_result: input.opencode_probe_result.cloned(),
            pi_probe_result: input.pi_probe_result.cloned(),
            cursor_probe_result: input.cursor_probe_result.cloned(),
        }
    }
}

impl ProbeResolver for StaticProbeResolver {
    fn opencode_probe_result(&mut self) -> Option<OpenCodeProbeResult> {
        self.opencode_probe_result.clone()
    }

    fn pi_probe_result(&mut self) -> Option<PiProbeResult> {
        self.pi_probe_result.clone()
    }

    fn cursor_probe_result(&mut self) -> Option<CursorProbeResult> {
        self.cursor_probe_result.clone()
    }

    fn listing_evidence(&mut self, harness: HarnessId) -> ListingEvidence {
        ListingEvidenceSet::from_results_assuming_latest_ok(
            self.opencode_probe_result.as_ref(),
            self.pi_probe_result.as_ref(),
            self.cursor_probe_result.as_ref(),
        )
        .get(harness)
    }
}

/// Assess one fixed harness using a supplied auth observer and the probe snapshot.
pub fn evaluate_fixed_harness_with_auth<F>(
    input: &RoutingInput<'_>,
    harness: &str,
    auth_check: F,
) -> CandidateAssessment
where
    F: Fn(&str) -> crate::harness::host::AuthState,
{
    let mut probes = StaticProbeResolver::from_input(input);
    evaluate_fixed_harness_with_auth_and_probes(input, harness, &mut probes, auth_check)
}

pub fn evaluate_fixed_harness_with_auth_and_probes<F, P>(
    input: &RoutingInput<'_>,
    harness: &str,
    probe_resolver: &mut P,
    auth_check: F,
) -> CandidateAssessment
where
    F: Fn(&str) -> crate::harness::host::AuthState,
    P: ProbeResolver + ?Sized,
{
    candidate_match_evidence_with_auth(
        input,
        harness,
        input.settings_provider_order,
        probe_resolver,
        &auth_check,
    )
}

/// Build a fixed-selection routing trace from one fixed harness assessment.
pub fn trace_for_fixed_harness(
    source: RouteSource,
    harness: &str,
    assessment: CandidateAssessment,
    diagnostics: Vec<String>,
) -> RoutingTrace {
    let match_evidence = assessment.match_evidence.unwrap_or(MatchEvidence::None);

    RoutingTrace {
        source,
        selection_kind: SelectionKind::Fixed,
        selected_by_preference: false,
        match_evidence,
        harness: harness.to_string(),
        harness_order_position: None,
        candidates_tried: vec![harness.to_string()],
        assessments: vec![assessment],
        diagnostics,
        exhaustion_reason: None,
    }
}

pub fn provider_for_order_for_fixed_harness<'a>(
    provider_for_order: Option<&'a str>,
    harness: &str,
) -> Option<&'a str> {
    let has_explicit_provider = provider_for_order.is_some_and(|provider| {
        let normalized = provider.trim();
        !normalized.is_empty() && !normalized.eq_ignore_ascii_case("unknown")
    });
    if has_explicit_provider {
        return provider_for_order;
    }

    native_provider_for_harness(harness).or(provider_for_order)
}

pub fn evaluate_candidates_with_auth<F>(input: &RoutingInput<'_>, auth_check: F) -> RoutingTrace
where
    F: Fn(&str) -> crate::harness::host::AuthState,
{
    let mut probe_resolver = StaticProbeResolver::from_input(input);
    evaluate_candidates(input, &mut probe_resolver, auth_check)
}

/// The single candidate evaluator for runtime routing and native materialization.
pub fn evaluate_candidates<F, P>(
    input: &RoutingInput<'_>,
    probe_resolver: &mut P,
    auth_check: F,
) -> RoutingTrace
where
    F: Fn(&str) -> crate::harness::host::AuthState,
    P: ProbeResolver + ?Sized,
{
    let mut diagnostics = Vec::new();
    let provider_order = parse_settings_provider_order(input.settings_provider_order);
    let default_harness =
        normalize_config_default_harness(input.config_default_harness, &mut diagnostics);
    let constrained = !matches!(input.harness_scope, HarnessScope::Unrestricted)
        || !input.excluded_harnesses.is_empty();
    if default_harness.as_ref().is_some_and(|harness| {
        permission_denial(&input.harness_scope, input.excluded_harnesses, harness).is_some()
    }) {
        diagnostics.push("settings.default_harness is excluded by known linked harness constraints; ignoring fallback".to_string());
    }

    // Ordering is preference, never permission. Every route is assessed once.
    let mut candidates = Vec::new();
    if let Some((harness, source)) = input.preferred_harness {
        candidates.push((harness.to_string(), None, source));
    }
    if let Some(order) = input.settings_harness_order {
        let parsed = models::harness::parse_settings_harness_order(order);
        diagnostics.extend(parsed.warnings);
        if parsed.valid_candidates.is_empty() {
            diagnostics.push("settings.harness_order has no candidates; trying default and remaining permitted harnesses".to_string());
        }
        candidates.extend(
            parsed
                .valid_candidates
                .into_iter()
                .enumerate()
                .map(|(position, harness)| (harness, Some(position), RouteSource::ConfigOrder)),
        );
    } else {
        candidates.extend(
            crate::harness::registry::default_harness_order_names()
                .into_iter()
                .map(|harness| (harness, None, RouteSource::Provider)),
        );
    }
    if let Some(harness) = default_harness {
        candidates.push((harness, None, RouteSource::ConfigDefault));
    }
    candidates.extend(
        mars_schema::harness::ALL
            .iter()
            .map(|id| (id.as_str().to_string(), None, RouteSource::Provider)),
    );
    let mut seen = HashSet::new();
    candidates.retain(|(harness, _, _)| {
        permission_denial(&input.harness_scope, input.excluded_harnesses, harness).is_none()
            && seen.insert(harness.clone())
    });

    let mut trace = RoutingTrace {
        source: if input.settings_harness_order.is_some() {
            RouteSource::ConfigOrder
        } else {
            RouteSource::Provider
        },
        selection_kind: SelectionKind::Auto,
        selected_by_preference: false,
        match_evidence: MatchEvidence::None,
        harness: String::new(),
        harness_order_position: None,
        candidates_tried: Vec::new(),
        assessments: Vec::new(),
        diagnostics,
        exhaustion_reason: None,
    };
    let mut unverified = None;
    for (harness, position, source) in candidates {
        let assessment = candidate_match_evidence_with_auth(
            input,
            &harness,
            Some(&provider_order),
            probe_resolver,
            &auth_check,
        );
        let evidence = assessment.match_evidence.unwrap_or(MatchEvidence::None);
        let eligibility = assessment.eligibility();
        trace.candidates_tried.push(harness.clone());
        trace.assessments.push(assessment);
        let preferred_supported = input.preferred_harness.is_some_and(|(preferred, source)| {
            source != RouteSource::Cli
                && preferred == harness
                && eligibility != Eligibility::Blocked
                && matches!(
                    evidence,
                    MatchEvidence::Confirmed | MatchEvidence::Constrained
                )
                && trace
                    .assessments
                    .last()
                    .is_some_and(|assessment| assessment.auth != Some(AuthState::ListingFailed))
        });
        if preferred_supported {
            trace.harness = harness;
            trace.harness_order_position = position;
            trace.source = source;
            trace.match_evidence = evidence;
            trace.selected_by_preference = true;
            return trace;
        }
        match eligibility {
            Eligibility::Eligible => {
                trace.harness = harness;
                trace.harness_order_position = position;
                trace.source = source;
                trace.match_evidence = evidence;
                return trace;
            }
            Eligibility::Unverified if unverified.is_none() => {
                unverified = Some((harness, position, source, evidence));
            }
            _ => {}
        }
    }
    if let Some((harness, position, source, evidence)) = unverified {
        trace.harness = harness;
        trace.harness_order_position = position;
        trace.source = source;
        trace.match_evidence = evidence;
    } else {
        if constrained {
            trace.exhaustion_reason = Some(ExhaustionReason::LinkedHarnessConstraints);
            trace.diagnostics.push("known linked harness constraints left no linked harness eligible for this model after routing assessments".to_string());
        } else {
            trace
                .diagnostics
                .push("no fallback harness is available after routing assessments".to_string());
        }
    }
    trace
}

/// Normalize and validate config default_harness. Returns normalized name or None with warning.
pub fn normalize_config_default_harness(
    config_default_harness: Option<&str>,
    warnings: &mut Vec<String>,
) -> Option<String> {
    match config_default_harness {
        Some(value) => match models::harness::normalize_harness_name(value) {
            Some(valid) => Some(valid),
            None => {
                warnings.push(format!(
                    "settings.default_harness `{value}` is invalid; expected one of: {}",
                    mars_schema::harness::names()
                ));
                None
            }
        },
        None => None,
    }
}

fn candidate_match_evidence_with_auth<F, P>(
    input: &RoutingInput<'_>,
    harness: &str,
    provider_order: Option<&[String]>,
    probe_resolver: &mut P,
    auth_check: &F,
) -> CandidateAssessment
where
    F: Fn(&str) -> crate::harness::host::AuthState,
    P: ProbeResolver + ?Sized,
{
    let harness_id = mars_schema::harness::parse(harness);
    let mut assessment =
        candidate_support_evidence(input, harness, harness_id, provider_order, probe_resolver);
    if assessment.match_evidence.is_some() && assessment.skip_reason.is_none() {
        let auth = match harness_id.map(|id| registry::descriptor(id).class) {
            Some(HarnessClass::Native { .. }) => auth_check(harness),
            Some(HarnessClass::ProbeBacked {
                listing: ListingAuth::Gated,
            }) => {
                let listing =
                    probe_resolver.listing_evidence(harness_id.expect("registry harness"));
                if listing.succeeded && !listing.latest_attempt_ok {
                    AuthState::ListingFailed
                } else if listing.succeeded {
                    AuthState::ImpliedByListing
                } else {
                    AuthState::Unchecked
                }
            }
            _ => AuthState::Unchecked,
        };
        if auth == crate::harness::host::AuthState::Unauthenticated {
            assessment.skip_reason = Some("native_auth_unavailable");
        }
        assessment.auth = Some(auth);
    }
    assessment
}

fn candidate_support_evidence<P>(
    input: &RoutingInput<'_>,
    harness: &str,
    harness_id: Option<HarnessId>,
    provider_order: Option<&[String]>,
    probe_resolver: &mut P,
) -> CandidateAssessment
where
    P: ProbeResolver + ?Sized,
{
    if let Some(reason) = permission_denial(&input.harness_scope, input.excluded_harnesses, harness)
    {
        return CandidateAssessment {
            auth: None,
            harness: harness.to_string(),
            installed: input.installed_harnesses.contains(harness),
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            match_evidence: None,
            skip_reason: Some(reason),
        };
    }

    if !input.installed_harnesses.contains(harness) {
        return CandidateAssessment {
            auth: None,
            harness: harness.to_string(),
            installed: false,
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            match_evidence: None,
            skip_reason: Some("not_installed"),
        };
    }

    if harness_id.is_some_and(|id| registry::native_provider(id).is_some())
        && provider_constraint_excludes_native_harness(input.provider_constraint, harness)
    {
        return CandidateAssessment {
            auth: None,
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            match_evidence: None,
            skip_reason: Some("provider_constraint_unsatisfied"),
        };
    }

    if input.model_id.trim().is_empty() {
        return CandidateAssessment {
            auth: None,
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            match_evidence: Some(
                if harness_id.is_some_and(|id| registry::native_provider(id).is_some()) {
                    MatchEvidence::Confirmed
                } else {
                    MatchEvidence::Passthrough
                },
            ),
            skip_reason: None,
        };
    }

    if harness_id.is_some_and(|id| registry::native_provider(id).is_some()) {
        let native_slugs = catalog_slugs_for_native_harness(harness, input.catalog_model_slugs);
        if !native_slugs.is_empty() {
            let selection = select_probe_slug(
                input.model_id,
                input.provider_constraint,
                effective_provider_for_order(input).as_deref(),
                provider_order,
                native_slugs,
            );
            return assessment_from_slug_selection(harness, selection, input.provider_constraint);
        }

        if is_native_match(effective_provider_for_order(input).as_deref(), harness) {
            return CandidateAssessment {
                auth: None,
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: Vec::new(),
                filtered_slugs: Vec::new(),
                chosen_slug: None,
                chosen_model: Some(input.model_id.to_string()),
                match_evidence: Some(match_evidence_for_match(input.provider_constraint)),
                skip_reason: None,
            };
        }

        return CandidateAssessment {
            auth: None,
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            match_evidence: None,
            skip_reason: Some("no_model_match"),
        };
    }

    if harness_id == Some(HarnessId::OpenCode) {
        let Some(opencode_probe) = probe_resolver.opencode_probe_result() else {
            return CandidateAssessment {
                auth: None,
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: Vec::new(),
                filtered_slugs: Vec::new(),
                chosen_slug: None,
                chosen_model: None,
                match_evidence: Some(MatchEvidence::Passthrough),
                skip_reason: None,
            };
        };
        if !opencode_probe.model_probe_success {
            return CandidateAssessment {
                auth: None,
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: Vec::new(),
                filtered_slugs: Vec::new(),
                chosen_slug: None,
                chosen_model: None,
                match_evidence: Some(MatchEvidence::Passthrough),
                skip_reason: None,
            };
        }

        let selection = select_probe_slug(
            input.model_id,
            input.provider_constraint,
            input.provider_for_order,
            provider_order,
            opencode_probe.model_slugs.iter().map(String::as_str),
        );

        if let Some(chosen_slug) = selection.chosen_slug.clone() {
            return CandidateAssessment {
                auth: None,
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: selection.candidate_slugs,
                filtered_slugs: selection.filtered_slugs,
                chosen_model: slug::parse(&chosen_slug).map(|parts| parts.model_id.to_string()),
                chosen_slug: Some(chosen_slug),
                match_evidence: Some(match_evidence_for_match(input.provider_constraint)),
                skip_reason: None,
            };
        }

        if !selection.candidate_slugs.is_empty() {
            return CandidateAssessment {
                auth: None,
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: selection.candidate_slugs,
                filtered_slugs: selection.filtered_slugs,
                chosen_slug: None,
                chosen_model: None,
                match_evidence: None,
                skip_reason: Some("provider_constraint_unsatisfied"),
            };
        }

        return CandidateAssessment {
            auth: None,
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: selection.candidate_slugs,
            filtered_slugs: selection.filtered_slugs,
            chosen_slug: None,
            chosen_model: None,
            match_evidence: None,
            skip_reason: Some("no_model_match"),
        };
    }

    if harness_id == Some(HarnessId::Pi) {
        if let Some(pi_probe) = probe_resolver.pi_probe_result() {
            if pi_probe.compatible {
                if !pi_probe.model_probe_success {
                    return passthrough_assessment(harness);
                }
                let selection = select_probe_slug(
                    input.model_id,
                    input.provider_constraint,
                    input.provider_for_order,
                    provider_order,
                    pi_probe.model_slugs.iter().map(String::as_str),
                );

                if let Some(chosen_slug) = selection.chosen_slug.clone() {
                    return CandidateAssessment {
                        auth: None,
                        harness: harness.to_string(),
                        installed: true,
                        candidate_slugs: selection.candidate_slugs,
                        filtered_slugs: selection.filtered_slugs,
                        chosen_model: slug::parse(&chosen_slug)
                            .map(|parts| parts.model_id.to_string()),
                        chosen_slug: Some(chosen_slug),
                        match_evidence: Some(match_evidence_for_match(input.provider_constraint)),
                        skip_reason: None,
                    };
                }

                if !selection.candidate_slugs.is_empty() {
                    return CandidateAssessment {
                        auth: None,
                        harness: harness.to_string(),
                        installed: true,
                        candidate_slugs: selection.candidate_slugs,
                        filtered_slugs: selection.filtered_slugs,
                        chosen_slug: None,
                        chosen_model: None,
                        match_evidence: None,
                        skip_reason: Some("provider_constraint_unsatisfied"),
                    };
                }

                return CandidateAssessment {
                    auth: None,
                    harness: harness.to_string(),
                    installed: true,
                    candidate_slugs: selection.candidate_slugs,
                    filtered_slugs: selection.filtered_slugs,
                    chosen_slug: None,
                    chosen_model: None,
                    match_evidence: None,
                    skip_reason: Some("no_model_match"),
                };
            }
            return CandidateAssessment {
                auth: None,
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: Vec::new(),
                filtered_slugs: Vec::new(),
                chosen_slug: None,
                chosen_model: None,
                match_evidence: None,
                skip_reason: Some("pi_incompatible"),
            };
        }

        return CandidateAssessment {
            auth: None,
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            match_evidence: Some(MatchEvidence::Passthrough),
            skip_reason: None,
        };
    }

    if harness_id == Some(HarnessId::Cursor) {
        let Some(cursor_probe) = probe_resolver.cursor_probe_result() else {
            return passthrough_assessment(harness);
        };
        if !cursor_probe.model_probe_success {
            return passthrough_assessment(harness);
        }
        if cursor_probe.slugs.is_empty() {
            return passthrough_assessment(harness);
        }

        let normalized_model = crate::models::probes::cursor::normalize_slug(input.model_id);
        if cursor_probe
            .slugs
            .iter()
            .any(|slug| crate::models::probes::cursor::normalize_slug(slug) == normalized_model)
        {
            return CandidateAssessment {
                auth: None,
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: vec![input.model_id.to_string()],
                filtered_slugs: vec![input.model_id.to_string()],
                chosen_slug: Some(input.model_id.to_string()),
                chosen_model: Some(input.model_id.to_string()),
                match_evidence: Some(MatchEvidence::Confirmed),
                skip_reason: None,
            };
        }

        let matches = crate::models::probes::cursor::find_cursor_prefix_matches(
            input.model_id,
            &cursor_probe.slugs,
        );
        if !matches.is_empty() {
            let candidate_slugs: Vec<String> =
                matches.iter().map(|slug| (*slug).to_string()).collect();
            return CandidateAssessment {
                auth: None,
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: candidate_slugs.clone(),
                filtered_slugs: candidate_slugs,
                chosen_slug: Some(input.model_id.to_string()),
                chosen_model: Some(input.model_id.to_string()),
                match_evidence: Some(MatchEvidence::Confirmed),
                skip_reason: None,
            };
        }

        // Probe slugs didn't match, but if the alias declares provider=cursor,
        // trust the constraint over possibly-stale probe cache.
        if input
            .provider_constraint
            .is_some_and(|p| p.eq_ignore_ascii_case("cursor"))
        {
            return CandidateAssessment {
                auth: None,
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: Vec::new(),
                filtered_slugs: Vec::new(),
                chosen_slug: None,
                chosen_model: None,
                match_evidence: Some(MatchEvidence::Constrained),
                skip_reason: None,
            };
        }

        return CandidateAssessment {
            auth: None,
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: Vec::new(),
            filtered_slugs: Vec::new(),
            chosen_slug: None,
            chosen_model: None,
            match_evidence: None,
            skip_reason: Some("no_model_match"),
        };
    }

    CandidateAssessment {
        auth: None,
        harness: harness.to_string(),
        installed: true,
        candidate_slugs: Vec::new(),
        filtered_slugs: Vec::new(),
        chosen_slug: None,
        chosen_model: None,
        match_evidence: None,
        skip_reason: Some("unsupported_candidate"),
    }
}

fn passthrough_assessment(harness: &str) -> CandidateAssessment {
    CandidateAssessment {
        auth: None,
        harness: harness.to_string(),
        installed: true,
        candidate_slugs: Vec::new(),
        filtered_slugs: Vec::new(),
        chosen_slug: None,
        chosen_model: None,
        match_evidence: Some(MatchEvidence::Passthrough),
        skip_reason: None,
    }
}

fn native_provider_for_harness(harness: &str) -> Option<&'static str> {
    mars_schema::harness::parse(harness).and_then(registry::native_provider)
}

fn is_native_match(provider: Option<&str>, harness: &str) -> bool {
    provider
        .map(|provider| slug::provider_matches_native_harness(provider, harness))
        .unwrap_or(false)
}

fn provider_constraint_excludes_native_harness(
    provider_constraint: Option<&str>,
    harness: &str,
) -> bool {
    let Some(provider_constraint) = provider_constraint else {
        return false;
    };

    !slug::provider_matches_native_harness(provider_constraint, harness)
}

fn match_evidence_for_match(provider_constraint: Option<&str>) -> MatchEvidence {
    if provider_constraint.is_some() {
        MatchEvidence::Constrained
    } else {
        MatchEvidence::Confirmed
    }
}

fn parse_settings_provider_order(provider_order: Option<&[String]>) -> Vec<String> {
    provider_order
        .unwrap_or_default()
        .iter()
        .map(|provider| provider.trim().to_ascii_lowercase())
        .filter(|provider| !provider.is_empty())
        .collect()
}

fn effective_provider_for_order(input: &RoutingInput<'_>) -> Option<String> {
    input
        .provider_for_order
        .map(str::trim)
        .filter(|provider| !provider.is_empty() && !provider.eq_ignore_ascii_case("unknown"))
        .map(str::to_string)
        .or_else(|| models::infer_provider_from_model_id(input.model_id).map(str::to_string))
}

fn catalog_slugs_for_native_harness<'a>(
    harness: &str,
    catalog_model_slugs: Option<&'a [String]>,
) -> Vec<&'a str> {
    let Some(slugs) = catalog_model_slugs else {
        return Vec::new();
    };
    slugs
        .iter()
        .filter(|slug| {
            slug::parse(slug)
                .is_some_and(|parts| slug::provider_matches_native_harness(parts.provider, harness))
        })
        .map(String::as_str)
        .collect()
}

fn assessment_from_slug_selection(
    harness: &str,
    selection: SlugSelection,
    provider_constraint: Option<&str>,
) -> CandidateAssessment {
    if let Some(chosen_slug) = selection.chosen_slug.clone() {
        return CandidateAssessment {
            auth: None,
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: selection.candidate_slugs,
            filtered_slugs: selection.filtered_slugs,
            chosen_model: slug::parse(&chosen_slug).map(|parts| parts.model_id.to_string()),
            chosen_slug: Some(chosen_slug),
            match_evidence: Some(match_evidence_for_match(provider_constraint)),
            skip_reason: None,
        };
    }

    if !selection.candidate_slugs.is_empty() {
        return CandidateAssessment {
            auth: None,
            harness: harness.to_string(),
            installed: true,
            candidate_slugs: selection.candidate_slugs,
            filtered_slugs: selection.filtered_slugs,
            chosen_slug: None,
            chosen_model: None,
            match_evidence: None,
            skip_reason: Some("provider_constraint_unsatisfied"),
        };
    }

    CandidateAssessment {
        auth: None,
        harness: harness.to_string(),
        installed: true,
        candidate_slugs: selection.candidate_slugs,
        filtered_slugs: selection.filtered_slugs,
        chosen_slug: None,
        chosen_model: None,
        match_evidence: None,
        skip_reason: Some("no_model_match"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    type ProbeInputs<'a> = (
        Option<&'a OpenCodeProbeResult>,
        Option<&'a PiProbeResult>,
        Option<&'a CursorProbeResult>,
    );

    fn routing_input<'a>(
        model_id: &'a str,
        provider_for_order: Option<&'a str>,
        settings_harness_order: Option<&'a [String]>,
        config_default_harness: Option<&'a str>,
        installed_harnesses: &'a HashSet<String>,
        linked_harnesses: Option<&'a [String]>,
        probe_inputs: ProbeInputs<'a>,
    ) -> RoutingInput<'a> {
        routing_input_with_catalog(
            model_id,
            provider_for_order,
            settings_harness_order,
            config_default_harness,
            installed_harnesses,
            linked_harnesses,
            None,
            probe_inputs,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn routing_input_with_catalog<'a>(
        model_id: &'a str,
        provider_for_order: Option<&'a str>,
        settings_harness_order: Option<&'a [String]>,
        config_default_harness: Option<&'a str>,
        installed_harnesses: &'a HashSet<String>,
        linked_harnesses: Option<&'a [String]>,
        catalog_model_slugs: Option<&'a [String]>,
        probe_inputs: ProbeInputs<'a>,
    ) -> RoutingInput<'a> {
        let (opencode_probe_result, pi_probe_result, cursor_probe_result) = probe_inputs;
        RoutingInput {
            preferred_harness: None,
            model_id,
            provider_for_order,
            provider_constraint: None,
            settings_provider_order: None,
            settings_harness_order,
            config_default_harness,
            installed_harnesses,
            excluded_harnesses: &[],
            harness_scope: match linked_harnesses {
                None => HarnessScope::Unrestricted,
                Some(names) => HarnessScope::Only(
                    names
                        .iter()
                        .map(|name| mars_schema::harness::parse(name).unwrap())
                        .collect(),
                ),
            },
            opencode_probe_result,
            pi_probe_result,
            cursor_probe_result,
            catalog_model_slugs,
        }
    }

    #[test]
    fn supported_model_does_not_turn_unknown_auth_into_success_or_rejection() {
        use crate::harness::host::AuthState;
        let installed = installed(&["claude"]);
        let input = routing_input(
            "claude-opus-4-6",
            Some("anthropic"),
            None,
            None,
            &installed,
            None,
            (None, None, None),
        );
        for (auth, verdict, reason) in [
            (AuthState::Authenticated, Eligibility::Eligible, None),
            (
                AuthState::Unauthenticated,
                Eligibility::Blocked,
                Some("native_auth_unavailable"),
            ),
            (
                AuthState::Unknown {
                    reason: "PRIVATE_AUTH_DETAIL".into(),
                },
                Eligibility::Unverified,
                Some("auth_unknown"),
            ),
        ] {
            let assessment = evaluate_fixed_harness_with_auth(&input, "claude", |_| auth.clone());
            assert_eq!(assessment.match_evidence, Some(MatchEvidence::Confirmed));
            assert_eq!(assessment.auth, Some(auth));
            assert_eq!(assessment.eligibility(), verdict);
            assert_eq!(assessment.eligibility_reason(), reason);
            let trace = trace_for_fixed_harness(RouteSource::Cli, "claude", assessment, Vec::new());
            let accepted = acceptance::accept_route(
                &trace,
                &installed,
                acceptance::MatchPolicy::AllowPassthrough,
            );
            assert_eq!(accepted.is_ok(), verdict != Eligibility::Blocked);
            assert!(
                !serde_json::to_string(&report::ModelAttemptReport::from_trace(
                    "model", "model", "cli", &trace
                ))
                .unwrap()
                .contains("PRIVATE_AUTH_DETAIL")
            );
        }
    }
}
