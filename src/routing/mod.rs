use crate::config::targets::HarnessScope;
use crate::harness::registry::HarnessId;
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
            || self.auth != Some(crate::harness::host::AuthState::Authenticated)
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
            Eligibility::Unverified => Some("auth_unknown"),
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
    pub fn to_report(&self) -> report::RouteDecisionReport {
        report::RouteDecisionReport::from_trace(self)
    }
}

/// Input to the routing engine.
pub struct RoutingInput<'a> {
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
    } else if crate::harness::registry::parse(harness).is_some_and(|id| excluded.contains(&id)) {
        Some("excluded_by_caller")
    } else {
        None
    }
}

pub trait ProbeResolver {
    fn opencode_probe_result(&mut self) -> Option<OpenCodeProbeResult>;
    fn pi_probe_result(&mut self) -> Option<PiProbeResult>;
    fn cursor_probe_result(&mut self) -> Option<CursorProbeResult>;
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
    let provider_order =
        parse_settings_provider_order(input.settings_provider_order, &mut diagnostics);
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
        crate::harness::registry::all()
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
                    models::harness::VALID_HARNESSES.join(", ")
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
    let mut assessment = candidate_support_evidence(input, harness, provider_order, probe_resolver);
    if assessment.match_evidence.is_some() && assessment.skip_reason.is_none() {
        let auth = if is_native_harness(harness) {
            auth_check(harness)
        } else {
            crate::harness::host::AuthState::NotApplicable
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

    if is_native_harness(harness)
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
            match_evidence: Some(if is_native_harness(harness) {
                MatchEvidence::Confirmed
            } else {
                MatchEvidence::Passthrough
            }),
            skip_reason: None,
        };
    }

    if is_native_harness(harness) {
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

    if harness == "opencode" {
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

    if harness == "pi" {
        if let Some(pi_probe) = probe_resolver.pi_probe_result() {
            if pi_probe.compatible {
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

    if harness == "cursor" {
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
    match harness {
        "claude" => Some("anthropic"),
        "codex" => Some("openai"),
        _ => None,
    }
}

fn is_native_match(provider: Option<&str>, harness: &str) -> bool {
    provider
        .map(|provider| slug::provider_matches_native_harness(provider, harness))
        .unwrap_or(false)
}

fn is_native_harness(harness: &str) -> bool {
    matches!(harness, "claude" | "codex")
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

fn parse_settings_provider_order(
    provider_order: Option<&[String]>,
    diagnostics: &mut Vec<String>,
) -> Vec<String> {
    let Some(provider_order) = provider_order else {
        return Vec::new();
    };

    provider_order
        .iter()
        .filter_map(|provider| {
            let normalized = provider.trim().to_ascii_lowercase();
            if normalized.is_empty() {
                return None;
            }
            if !is_known_provider_or_variant(&normalized) {
                diagnostics.push(format!(
                    "settings.provider_order contains unknown provider `{provider}`; keeping it for forward-compat routing preferences"
                ));
            }
            Some(normalized)
        })
        .collect()
}

fn is_known_provider_or_variant(provider: &str) -> bool {
    matches!(
        provider,
        "anthropic"
            | "openai"
            | "google"
            | "meta"
            | "mistral"
            | "deepseek"
            | "cohere"
            | "openrouter"
            | "openai-codex"
            | "anthropic-claude"
    )
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

    fn always_authed(_: &str) -> crate::harness::host::AuthState {
        crate::harness::host::AuthState::Authenticated
    }

    fn never_authed(_: &str) -> crate::harness::host::AuthState {
        crate::harness::host::AuthState::Unauthenticated
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
                        .map(|name| crate::harness::registry::parse(name).unwrap())
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
                !serde_json::to_string(&trace.to_report())
                    .unwrap()
                    .contains("PRIVATE_AUTH_DETAIL")
            );
        }
    }

    #[test]
    fn authenticated_native_outranks_probe_support_without_auth_evidence() {
        let installed = installed(&["pi", "codex"]);
        let order = vec!["pi".to_string(), "codex".to_string()];
        let pi = PiProbeResult {
            compatible: true,
            model_slugs: HashSet::from(["openai/gpt-5".to_string()]),
            ..PiProbeResult::default()
        };
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            Some(&order),
            None,
            &installed,
            None,
            (None, Some(&pi), None),
        );
        let trace = evaluate_candidates_with_auth(&input, always_authed);
        assert_eq!(trace.harness, "codex");
        assert_eq!(
            trace.assessments[0].match_evidence,
            Some(MatchEvidence::Confirmed)
        );
        assert_eq!(trace.assessments[0].eligibility(), Eligibility::Unverified);
        assert_eq!(
            trace.assessments[0].eligibility_reason(),
            Some("auth_unknown")
        );
        assert_eq!(trace.assessments[1].eligibility(), Eligibility::Eligible);
    }

    #[test]
    fn disabled_fixed_harness_is_rejected_before_auth() {
        let installed = installed(&["claude"]);
        let enabled = vec!["codex".to_string()];
        let input = routing_input(
            "claude-opus-4-6",
            Some("anthropic"),
            None,
            None,
            &installed,
            Some(&enabled),
            (None, None, None),
        );
        let mut probes = StaticProbeResolver::from_input(&input);
        let assessment =
            evaluate_fixed_harness_with_auth_and_probes(&input, "claude", &mut probes, |_| {
                panic!("disabled harness must not be auth-probed")
            });
        assert_eq!(assessment.skip_reason, Some("disabled_target"));
        assert_eq!(assessment.match_evidence, None);
    }

    #[test]
    fn caller_excluded_fixed_harness_is_rejected_before_auth() {
        let installed = installed(&["claude"]);
        let mut input = routing_input(
            "claude-opus-4-6",
            Some("anthropic"),
            None,
            None,
            &installed,
            None,
            (None, None, None),
        );
        input.excluded_harnesses = &[HarnessId::Claude];
        let mut probes = StaticProbeResolver::from_input(&input);
        let assessment =
            evaluate_fixed_harness_with_auth_and_probes(&input, "claude", &mut probes, |_| {
                panic!("caller-excluded harness must not be auth-probed")
            });
        assert_eq!(assessment.skip_reason, Some("excluded_by_caller"));
        assert_eq!(assessment.match_evidence, None);
        assert!(assessment.installed);
    }

    #[test]
    fn empty_model_routing_prefers_default_harness_order() {
        let installed = installed(&["cursor", "opencode"]);
        let input = routing_input("", None, None, None, &installed, None, (None, None, None));

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.harness, "cursor");
        assert_eq!(trace.selection_kind, SelectionKind::Auto);
        assert_eq!(trace.match_evidence, MatchEvidence::Passthrough);
    }

    #[test]
    fn native_match_with_auth_returns_confirmed() {
        let installed = installed(&["claude"]);
        let input = routing_input(
            "claude-opus-4-7",
            Some("anthropic"),
            None,
            None,
            &installed,
            None,
            (None, None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.source, RouteSource::Provider);
        assert_eq!(trace.selection_kind, SelectionKind::Auto);
        assert_eq!(trace.harness, "claude");
        assert_eq!(trace.match_evidence, MatchEvidence::Confirmed);
        assert_eq!(trace.candidates_tried, vec!["claude".to_string()]);
    }

    #[test]
    fn catalog_native_match_without_explicit_provider() {
        let installed = installed(&["claude", "pi"]);
        let catalog = vec!["anthropic/claude-opus-4-6".to_string()];
        let harness_order = vec!["claude".to_string(), "pi".to_string()];
        let input = routing_input_with_catalog(
            "claude-opus-4-6",
            None,
            Some(&harness_order),
            None,
            &installed,
            None,
            Some(&catalog),
            (None, None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.harness, "claude");
        assert_eq!(trace.selection_kind, SelectionKind::Auto);
        assert_eq!(trace.match_evidence, MatchEvidence::Confirmed);
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "claude")
                .and_then(|assessment| assessment.chosen_slug.as_deref()),
            Some("anthropic/claude-opus-4-6")
        );
    }

    #[test]
    fn incompatible_and_unauthenticated_routes_both_exhaust() {
        let installed = installed(&["claude", "pi"]);
        let catalog = vec!["anthropic/claude-opus-4-6".to_string()];
        let harness_order = vec!["pi".to_string(), "claude".to_string()];
        let linked = vec!["pi".to_string(), "claude".to_string()];
        let pi_probe = PiProbeResult {
            compatible: false,
            model_slugs: HashSet::new(),
            ..PiProbeResult::default()
        };
        let input = routing_input_with_catalog(
            "claude-opus-4-6",
            None,
            Some(&harness_order),
            None,
            &installed,
            Some(&linked),
            Some(&catalog),
            (None, Some(&pi_probe), None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert!(trace.harness.is_empty());
        assert_eq!(trace.selection_kind, SelectionKind::Auto);
        assert_eq!(
            trace.exhaustion_reason,
            Some(ExhaustionReason::LinkedHarnessConstraints)
        );
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "claude")
                .and_then(|assessment| assessment.skip_reason),
            Some("native_auth_unavailable")
        );
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "pi")
                .and_then(|assessment| assessment.skip_reason),
            Some("pi_incompatible")
        );
    }

    #[test]
    fn native_match_without_auth_falls_through() {
        let installed = installed(&["claude", "pi"]);
        let input = routing_input(
            "claude-opus-4-7",
            Some("anthropic"),
            None,
            None,
            &installed,
            None,
            (None, None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.selection_kind, SelectionKind::Auto);
        assert_eq!(trace.match_evidence, MatchEvidence::Passthrough);
        assert_eq!(trace.candidates_tried[0], "claude");
        assert_eq!(trace.candidates_tried[1], "codex");
        assert_eq!(trace.candidates_tried[2], "pi");
        assert_eq!(
            trace
                .assessments
                .first()
                .and_then(|assessment| assessment.skip_reason),
            Some("native_auth_unavailable")
        );
    }

    #[test]
    fn pi_or_cursor_installed_returns_passthrough() {
        let installed = installed(&["cursor"]);
        let input = routing_input(
            "gemini-2.5-pro",
            Some("google"),
            None,
            None,
            &installed,
            None,
            (None, None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "cursor");
        assert_eq!(trace.match_evidence, MatchEvidence::Passthrough);
    }

    #[test]
    fn cursor_with_no_probe_falls_back_to_passthrough() {
        let installed = installed(&["cursor"]);
        let input = routing_input(
            "gpt-5.5",
            Some("openai"),
            None,
            None,
            &installed,
            None,
            (None, None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);
        assert_eq!(trace.harness, "cursor");
        assert_eq!(trace.match_evidence, MatchEvidence::Passthrough);
    }

    #[test]
    fn cursor_prefix_match_returns_confirmed_with_candidate_slugs() {
        let installed = installed(&["cursor"]);
        let cursor_probe = CursorProbeResult {
            slugs: vec!["gpt-5.5-high".to_string(), "gpt-5.5-low".to_string()],
            model_probe_success: true,
            error: None,
        };
        let input = routing_input(
            "gpt-5.5",
            Some("openai"),
            None,
            None,
            &installed,
            None,
            (None, None, Some(&cursor_probe)),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);
        assert_eq!(trace.harness, "cursor");
        assert_eq!(trace.match_evidence, MatchEvidence::Confirmed);
        let cursor_assessment = trace
            .assessments
            .iter()
            .find(|assessment| assessment.harness == "cursor")
            .expect("cursor assessment should exist");
        assert_eq!(
            cursor_assessment.candidate_slugs,
            vec!["gpt-5.5-high".to_string(), "gpt-5.5-low".to_string()]
        );
        assert_eq!(cursor_assessment.chosen_slug.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn cursor_exact_match_returns_confirmed() {
        let installed = installed(&["cursor"]);
        let cursor_probe = CursorProbeResult {
            slugs: vec!["gpt-5.5".to_string(), "gpt-5.5-high".to_string()],
            model_probe_success: true,
            error: None,
        };
        let input = routing_input(
            "gpt-5.5",
            Some("openai"),
            None,
            None,
            &installed,
            None,
            (None, None, Some(&cursor_probe)),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);
        assert_eq!(trace.harness, "cursor");
        assert_eq!(trace.match_evidence, MatchEvidence::Confirmed);
        let cursor_assessment = trace
            .assessments
            .iter()
            .find(|assessment| assessment.harness == "cursor")
            .expect("cursor assessment should exist");
        assert_eq!(
            cursor_assessment.candidate_slugs,
            vec!["gpt-5.5".to_string()]
        );
        assert_eq!(cursor_assessment.chosen_slug.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn cursor_no_match_falls_through() {
        let installed = installed(&["cursor"]);
        let cursor_probe = CursorProbeResult {
            slugs: vec!["claude-opus-4-7-high".to_string()],
            model_probe_success: true,
            error: None,
        };
        let input = routing_input(
            "gpt-5.5",
            Some("openai"),
            None,
            None,
            &installed,
            None,
            (None, None, Some(&cursor_probe)),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);
        assert_eq!(trace.harness, "");
        assert_eq!(trace.selection_kind, SelectionKind::Auto);
        assert_eq!(trace.match_evidence, MatchEvidence::None);
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "cursor")
                .and_then(|assessment| assessment.skip_reason),
            Some("no_model_match")
        );
    }

    #[test]
    fn compatible_pi_probe_returns_confirmed() {
        let installed = installed(&["pi"]);
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs: HashSet::from(["google/gemini-2.5-pro".to_string()]),
            ..PiProbeResult::default()
        };
        let input = routing_input(
            "gemini-2.5-pro",
            Some("google"),
            None,
            None,
            &installed,
            None,
            (None, Some(&pi_probe), None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.match_evidence, MatchEvidence::Confirmed);
    }

    #[test]
    fn provider_constraint_accepts_variant_provider_name() {
        let installed = installed(&["pi", "opencode"]);
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs: HashSet::from(["openai-codex/gpt-5.4-mini".to_string()]),
            ..PiProbeResult::default()
        };
        let opencode_probe = OpenCodeProbeResult {
            model_slugs: vec!["openai/gpt-5.4-mini".to_string()],
            model_probe_success: true,
            error: None,
        };
        let input = RoutingInput {
            model_id: "gpt-5.4-mini",
            provider_for_order: Some("openai"),
            provider_constraint: Some("openai"),
            settings_provider_order: None,
            settings_harness_order: None,
            config_default_harness: None,
            installed_harnesses: &installed,
            excluded_harnesses: &[],
            harness_scope: HarnessScope::Unrestricted,
            opencode_probe_result: Some(&opencode_probe),
            pi_probe_result: Some(&pi_probe),
            cursor_probe_result: None,
            catalog_model_slugs: None,
        };

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.match_evidence, MatchEvidence::Constrained);
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "pi")
                .and_then(|assessment| assessment.chosen_slug.as_deref()),
            Some("openai-codex/gpt-5.4-mini")
        );
    }

    #[test]
    fn bare_direct_model_uses_default_ladder_before_pi_probe_slug() {
        let installed = installed(&["codex", "pi", "opencode"]);
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs: HashSet::from(["openai-codex/gpt-5.4".to_string()]),
            ..PiProbeResult::default()
        };
        let input = RoutingInput {
            model_id: "gpt-5.4",
            provider_for_order: None,
            provider_constraint: None,
            settings_provider_order: None,
            settings_harness_order: None,
            config_default_harness: None,
            installed_harnesses: &installed,
            excluded_harnesses: &[],
            harness_scope: HarnessScope::Unrestricted,
            opencode_probe_result: None,
            pi_probe_result: Some(&pi_probe),
            cursor_probe_result: None,
            catalog_model_slugs: None,
        };

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.harness, "codex");
        assert_eq!(trace.match_evidence, MatchEvidence::Confirmed);
        assert_eq!(trace.candidates_tried, vec!["claude", "codex"]);
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "codex")
                .and_then(|assessment| assessment.chosen_model.as_deref()),
            Some("gpt-5.4")
        );
    }

    #[test]
    fn provider_order_ranking_is_lenient_for_known_variants() {
        let provider_order = vec!["openai".to_string(), "anthropic".to_string()];
        assert_eq!(
            probe_match::provider_order_rank("openai-codex", &provider_order),
            0
        );
        assert_eq!(
            probe_match::provider_order_rank("anthropic-claude", &provider_order),
            1
        );
        assert_eq!(
            probe_match::provider_order_rank("openrouter", &provider_order),
            usize::MAX
        );
    }

    #[test]
    fn unknown_provider_order_entries_warn_but_do_not_block_routing() {
        let installed = installed(&["opencode"]);
        let provider_order = vec!["future-provider".to_string()];
        let probe = OpenCodeProbeResult {
            model_slugs: vec!["openai/gpt-5.4-mini".to_string()],
            model_probe_success: true,
            error: None,
        };
        let input = RoutingInput {
            model_id: "gpt-5.4-mini",
            provider_for_order: Some("openai"),
            provider_constraint: None,
            settings_provider_order: Some(&provider_order),
            settings_harness_order: None,
            config_default_harness: None,
            installed_harnesses: &installed,
            excluded_harnesses: &[],
            harness_scope: HarnessScope::Unrestricted,
            opencode_probe_result: Some(&probe),
            pi_probe_result: None,
            cursor_probe_result: None,
            catalog_model_slugs: None,
        };

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "opencode");
        assert_eq!(trace.match_evidence, MatchEvidence::Confirmed);
        assert!(trace.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .contains("settings.provider_order contains unknown provider `future-provider`")
        }));
    }

    #[test]
    fn incompatible_pi_probe_skips_to_next_candidate() {
        let installed = installed(&["pi", "cursor"]);
        let pi_probe = PiProbeResult {
            compatible: false,
            ..PiProbeResult::default()
        };
        let input = routing_input(
            "gemini-2.5-pro",
            Some("google"),
            None,
            None,
            &installed,
            None,
            (None, Some(&pi_probe), None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "cursor");
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "pi")
                .and_then(|assessment| assessment.skip_reason),
            Some("pi_incompatible")
        );
    }

    #[test]
    fn opencode_positive_probe_returns_likely() {
        let installed = installed(&["opencode"]);
        let probe = OpenCodeProbeResult {
            model_slugs: vec!["openai/gpt-5".to_string()],
            model_probe_success: true,
            error: None,
        };
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            None,
            &installed,
            None,
            (Some(&probe), None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "opencode");
        assert_eq!(trace.match_evidence, MatchEvidence::Confirmed);
    }

    #[test]
    fn opencode_negative_probe_falls_through() {
        let installed = installed(&["opencode", "cursor"]);
        let probe = OpenCodeProbeResult {
            model_slugs: Vec::new(),
            model_probe_success: true,
            error: None,
        };
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            None,
            &installed,
            None,
            (Some(&probe), None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.harness, "cursor");
        assert_eq!(trace.match_evidence, MatchEvidence::Passthrough);
        assert_eq!(
            trace
                .assessments
                .iter()
                .find(|assessment| assessment.harness == "opencode")
                .and_then(|assessment| assessment.skip_reason),
            Some("no_model_match")
        );
    }

    #[test]
    fn link_filtering_reduces_candidates() {
        let installed = installed(&["codex", "pi"]);
        let linked_harnesses = vec!["pi".to_string()];
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            None,
            &installed,
            Some(&linked_harnesses),
            (None, None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.candidates_tried, vec!["pi"]);
    }

    #[test]
    fn settings_harness_order_overrides_provider_order() {
        let installed = installed(&["codex", "pi"]);
        let order = vec!["pi".to_string(), "codex".to_string()];
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            Some(&order),
            None,
            &installed,
            None,
            (None, None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.source, RouteSource::ConfigOrder);
        assert_eq!(trace.harness, "codex");
        assert_eq!(trace.harness_order_position, Some(1));
        assert_eq!(trace.match_evidence, MatchEvidence::Confirmed);
    }

    #[test]
    fn empty_harness_order_uses_remaining_permitted_harnesses() {
        let installed = installed(&["codex"]);
        let order: Vec<String> = Vec::new();
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            Some(&order),
            None,
            &installed,
            None,
            (None, None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, always_authed);

        assert_eq!(trace.source, RouteSource::Provider);
        assert_eq!(trace.harness, "codex");
        assert!(
            trace
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("settings.harness_order has no candidates"))
        );
    }

    #[test]
    fn rejected_routes_are_not_retried_or_resurrected() {
        for default in [None, Some("claude")] {
            let installed = installed(&["claude"]);
            let enabled = vec!["claude".to_string()];
            let order = vec!["claude".to_string(), "claude".to_string()];
            let input = routing_input(
                "claude-opus-4-6",
                Some("anthropic"),
                Some(&order),
                default,
                &installed,
                Some(&enabled),
                (None, None, None),
            );
            let auth_calls = std::cell::Cell::new(0);
            let trace = evaluate_candidates_with_auth(&input, |_| {
                auth_calls.set(auth_calls.get() + 1);
                crate::harness::host::AuthState::Unauthenticated
            });
            assert!(trace.harness.is_empty(), "{trace:?}");
            assert_eq!(trace.match_evidence, MatchEvidence::None);
            assert_eq!(auth_calls.get(), 1);
            assert_eq!(trace.assessments.len(), 1);
            assert_eq!(
                trace.assessments[0].skip_reason,
                Some("native_auth_unavailable")
            );
        }
    }

    #[test]
    fn configured_order_does_not_exclude_other_permitted_harnesses() {
        let installed = installed(&["codex"]);
        let order = vec!["claude".to_string()];
        let enabled = vec!["codex".to_string()];
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            Some(&order),
            None,
            &installed,
            Some(&enabled),
            (None, None, None),
        );
        let trace = evaluate_candidates_with_auth(&input, always_authed);
        assert_eq!(trace.harness, "codex");
        assert_eq!(trace.assessments[0].chosen_model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn default_harness_requires_the_same_installation_evidence() {
        let installed = installed(&[]);
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            Some("pi"),
            &installed,
            None,
            (None, None, None),
        );
        let trace = evaluate_candidates_with_auth(&input, never_authed);
        assert!(trace.harness.is_empty(), "{trace:?}");
        assert!(
            trace
                .assessments
                .iter()
                .all(|assessment| assessment.skip_reason == Some("not_installed"))
        );
    }

    #[test]
    fn default_harness_is_assessed_as_an_ordinary_candidate() {
        let installed = installed(&["codex"]);
        let order = Vec::new();
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            Some(&order),
            Some("Codex"),
            &installed,
            None,
            (None, None, None),
        );
        let trace = evaluate_candidates_with_auth(&input, always_authed);
        assert_eq!(trace.source, RouteSource::ConfigDefault);
        assert_eq!(trace.selection_kind, SelectionKind::Auto);
        assert_eq!(trace.harness, "codex");
        assert_eq!(trace.match_evidence, MatchEvidence::Confirmed);
        assert_eq!(trace.assessments.len(), 1);
    }

    #[test]
    fn returns_empty_trace_when_no_fallback_harness_available() {
        let installed = installed(&[]);
        let input = routing_input(
            "model",
            None,
            None,
            None,
            &installed,
            None,
            (None, None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.source, RouteSource::Provider);
        assert_eq!(trace.selection_kind, SelectionKind::Auto);
        assert_eq!(trace.match_evidence, MatchEvidence::None);
        assert_eq!(trace.harness, "");
        assert!(
            trace
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.contains("no fallback harness is available") })
        );
    }

    #[test]
    fn scoped_exhaustion_cannot_select_unrelated_pi() {
        use crate::routing::acceptance::{MatchPolicy, accept_route};

        let installed = installed(&["claude", "cursor", "codex", "pi"]);
        let linked_harnesses = vec![
            "claude".to_string(),
            "cursor".to_string(),
            "codex".to_string(),
        ];
        let cursor_probe = CursorProbeResult {
            model_probe_success: true,
            slugs: vec!["gpt-5.5".to_string()],
            ..CursorProbeResult::default()
        };
        let input = routing_input_with_catalog(
            "deepseekflash",
            Some("deepseek"),
            None,
            None,
            &installed,
            Some(&linked_harnesses),
            None,
            (None, None, Some(&cursor_probe)),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert!(trace.harness.is_empty());
        assert_eq!(trace.selection_kind, SelectionKind::Auto);
        assert_eq!(trace.match_evidence, MatchEvidence::None);
        assert_eq!(
            trace.exhaustion_reason,
            Some(ExhaustionReason::LinkedHarnessConstraints)
        );
        assert!(accept_route(&trace, &installed, MatchPolicy::AllowPassthrough).is_err());
    }

    #[test]
    fn linked_default_harness_is_allowed_when_linked() {
        let installed = installed(&["pi"]);
        let linked_harnesses = vec!["pi".to_string()];
        let trace = evaluate_candidates_with_auth(
            &routing_input(
                "gpt-5",
                Some("openai"),
                None,
                Some("pi"),
                &installed,
                Some(&linked_harnesses),
                (None, None, None),
            ),
            never_authed,
        );

        assert_eq!(trace.source, RouteSource::Provider);
        assert_eq!(trace.harness, "pi");
    }
}
