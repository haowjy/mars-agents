use std::collections::HashSet;

pub mod acceptance;
pub mod evidence;
pub mod probe_match;
pub mod report;
pub mod slug;

pub(crate) use probe_match::{SlugSelection, select_probe_slug};

use crate::models;
use crate::models::harness::HarnessOrderFailure;
use crate::models::probes::CursorProbeResult;
use crate::models::probes::OpenCodeProbeResult;
use crate::models::probes::PiProbeResult;

pub use evidence::{RoutingEvidence, RoutingSettingsEvidence};

/// How the harness was selected — orthogonal to slug evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    Auto,
    Fixed,
    ConfigDefault,
    LinkedFallback,
    HardcodedDefault,
}

impl SelectionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fixed => "fixed",
            Self::ConfigDefault => "config_default",
            Self::LinkedFallback => "linked_fallback",
            Self::HardcodedDefault => "hardcoded_default",
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

/// How the harness was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSource {
    Cli,
    Profile,
    Alias,
    ConfigOrder,
    ConfigDefault,
    Provider,
    HardcodedDefault,
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
            Self::HardcodedDefault => "default",
        }
    }
}

/// Assessment of one candidate harness.
#[derive(Debug, Clone)]
pub struct CandidateAssessment {
    pub harness: String,
    pub installed: bool,
    pub candidate_slugs: Vec<String>,
    pub filtered_slugs: Vec<String>,
    pub chosen_slug: Option<String>,
    pub chosen_model: Option<String>,
    pub match_evidence: Option<MatchEvidence>,
    pub skip_reason: Option<&'static str>,
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

    pub fn selected_chosen_slug_evidence(&self) -> Option<SelectedChosenSlugEvidence> {
        self.assessments
            .iter()
            .find(|assessment| assessment.harness == self.harness)
            .and_then(|assessment| {
                assessment
                    .chosen_slug
                    .as_ref()
                    .map(|slug| SelectedChosenSlugEvidence {
                        slug: slug.clone(),
                        match_evidence: assessment.match_evidence,
                    })
            })
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
    pub linked_harnesses: Option<&'a [String]>,
    pub opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    pub pi_probe_result: Option<&'a PiProbeResult>,
    pub cursor_probe_result: Option<&'a CursorProbeResult>,
    /// Cached catalog slugs (`provider/model`) for full model-id matching on native harnesses.
    pub catalog_model_slugs: Option<&'a [String]>,
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

/// Evaluate all candidates and return a routing trace.
/// This is the ONLY candidate evaluator. Both `mars models` and `mars build` call this.
pub fn evaluate_candidates(input: &RoutingInput<'_>) -> RoutingTrace {
    let mut probe_resolver = StaticProbeResolver::from_input(input);
    evaluate_candidates_with_auth_and_probes(
        input,
        &mut probe_resolver,
        models::harness::native_harness_authenticated,
    )
}

/// Evaluate one fixed harness choice without fallback.
/// Used by fixed-selection precedence paths (CLI/profile/alias).
pub fn evaluate_fixed_harness(input: &RoutingInput<'_>, harness: &str) -> CandidateAssessment {
    let mut probe_resolver = StaticProbeResolver::from_input(input);
    evaluate_fixed_harness_with_auth_and_probes(
        input,
        harness,
        &mut probe_resolver,
        models::harness::native_harness_authenticated,
    )
}

pub fn evaluate_fixed_harness_with_auth<F>(
    input: &RoutingInput<'_>,
    harness: &str,
    auth_check: F,
) -> CandidateAssessment
where
    F: Fn(&str) -> bool,
{
    let mut probe_resolver = StaticProbeResolver::from_input(input);
    evaluate_fixed_harness_with_auth_and_probes(input, harness, &mut probe_resolver, auth_check)
}

pub fn evaluate_fixed_harness_with_auth_and_probes<F, P>(
    input: &RoutingInput<'_>,
    harness: &str,
    probe_resolver: &mut P,
    auth_check: F,
) -> CandidateAssessment
where
    F: Fn(&str) -> bool,
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
    F: Fn(&str) -> bool,
{
    let mut probe_resolver = StaticProbeResolver::from_input(input);
    evaluate_candidates_with_auth_and_probes(input, &mut probe_resolver, auth_check)
}

pub fn evaluate_candidates_with_auth_and_probes<F, P>(
    input: &RoutingInput<'_>,
    probe_resolver: &mut P,
    auth_check: F,
) -> RoutingTrace
where
    F: Fn(&str) -> bool,
    P: ProbeResolver + ?Sized,
{
    let mut diagnostics = Vec::new();
    let parsed_provider_order =
        parse_settings_provider_order(input.settings_provider_order, &mut diagnostics);
    let config_default_harness =
        normalize_config_default_harness(input.config_default_harness, &mut diagnostics);
    let linked_harnesses = input
        .linked_harnesses
        .filter(|harnesses| !harnesses.is_empty());
    let linked_harnesses_set = linked_harnesses
        .map(|harnesses| harnesses.iter().map(String::as_str).collect::<HashSet<_>>());
    let has_link_constraints = linked_harnesses_set.is_some();
    let effective_config_default_harness = config_default_harness
        .as_ref()
        .filter(|harness| {
            linked_harnesses_set
                .as_ref()
                .is_none_or(|known| known.contains(harness.as_str()))
        })
        .cloned();
    if has_link_constraints
        && config_default_harness.is_some()
        && effective_config_default_harness.is_none()
    {
        diagnostics.push(
            "settings.default_harness is excluded by known linked harness constraints; ignoring fallback"
                .to_string(),
        );
    }

    let mut harness_order_failure = None;

    let mut candidate_source = RouteSource::Provider;

    let candidates = if let Some(order) = input.settings_harness_order {
        let parsed_order = models::harness::parse_settings_harness_order(order);
        diagnostics.extend(parsed_order.warnings);

        if parsed_order.failure == Some(HarnessOrderFailure::Empty) {
            diagnostics.push(
                "settings.harness_order is empty; falling through to provider candidate order"
                    .to_string(),
            );
            let provider_for_order = input.provider_for_order.unwrap_or("unknown");
            filter_candidates_by_links(
                models::harness::harness_candidates_for_provider(provider_for_order),
                linked_harnesses_set.as_ref(),
            )
            .into_iter()
            .map(|harness| (harness, None))
            .collect::<Vec<_>>()
        } else {
            candidate_source = RouteSource::ConfigOrder;
            let mut candidate_pairs = parsed_order
                .valid_candidates
                .into_iter()
                .enumerate()
                .map(|(index, harness)| (harness, Some(index)))
                .collect::<Vec<_>>();

            filter_candidate_pairs_by_links(&mut candidate_pairs, linked_harnesses_set.as_ref());

            let valid_candidates = candidate_pairs
                .iter()
                .map(|(harness, _)| harness.clone())
                .collect::<Vec<_>>();

            if !valid_candidates.is_empty()
                && valid_candidates
                    .iter()
                    .all(|candidate| !input.installed_harnesses.contains(candidate))
            {
                harness_order_failure = Some(HarnessOrderFailure::NoneInstalled {
                    valid_candidates: valid_candidates.clone(),
                });
            }

            candidate_pairs
        }
    } else if input.model_id.trim().is_empty() {
        filter_candidates_by_links(
            models::harness::VALID_HARNESSES
                .iter()
                .map(|harness| (*harness).to_string())
                .collect(),
            linked_harnesses_set.as_ref(),
        )
        .into_iter()
        .map(|harness| (harness, None))
        .collect::<Vec<_>>()
    } else {
        let provider_for_order = input.provider_for_order.unwrap_or("unknown");
        filter_candidates_by_links(
            models::harness::harness_candidates_for_provider(provider_for_order),
            linked_harnesses_set.as_ref(),
        )
        .into_iter()
        .map(|harness| (harness, None))
        .collect::<Vec<_>>()
    };

    let mut candidates_tried = Vec::new();
    let mut assessments = Vec::new();
    let mut passthrough_selection: Option<(String, Option<usize>, MatchEvidence)> = None;

    for (harness, harness_order_position) in candidates {
        let assessment = candidate_match_evidence_with_auth(
            input,
            &harness,
            Some(parsed_provider_order.as_slice()),
            probe_resolver,
            &auth_check,
        );

        candidates_tried.push(harness.clone());
        let match_evidence = assessment.match_evidence;
        assessments.push(assessment);

        if let Some(match_evidence) = match_evidence {
            match match_evidence {
                MatchEvidence::Confirmed | MatchEvidence::Constrained => {
                    return RoutingTrace {
                        source: candidate_source,
                        selection_kind: SelectionKind::Auto,
                        match_evidence,
                        harness,
                        harness_order_position,
                        candidates_tried,
                        assessments,
                        diagnostics,
                    };
                }
                MatchEvidence::Passthrough => {
                    if passthrough_selection.is_none() {
                        passthrough_selection =
                            Some((harness, harness_order_position, match_evidence));
                    }
                }
                MatchEvidence::None => {}
            }
        }
    }

    if let Some((harness, harness_order_position, match_evidence)) = passthrough_selection {
        return RoutingTrace {
            source: candidate_source,
            selection_kind: SelectionKind::Auto,
            match_evidence,
            harness,
            harness_order_position,
            candidates_tried,
            assessments,
            diagnostics,
        };
    }

    if input.settings_harness_order.is_some()
        && let Some(warning) = format_harness_order_fallback_warning(
            harness_order_failure.as_ref(),
            effective_config_default_harness.is_some(),
            has_link_constraints,
        )
    {
        diagnostics.push(warning);
    }

    if let Some(harness) = effective_config_default_harness {
        return RoutingTrace {
            source: RouteSource::ConfigDefault,
            selection_kind: SelectionKind::ConfigDefault,
            match_evidence: MatchEvidence::Passthrough,
            harness,
            harness_order_position: None,
            candidates_tried,
            assessments,
            diagnostics,
        };
    }

    if let Some(known_links) = linked_harnesses {
        if let Some(harness) = select_linked_fallback_harness(input, known_links, &assessments) {
            diagnostics.push(format!(
                "known linked harness constraints left no eligible auto-routing candidates; selecting linked harness `{harness}` in harness order (skipped incompatible candidates)"
            ));
            candidates_tried.push(harness.clone());

            return RoutingTrace {
                source: candidate_source,
                selection_kind: SelectionKind::LinkedFallback,
                match_evidence: MatchEvidence::Passthrough,
                harness,
                harness_order_position: None,
                candidates_tried,
                assessments,
                diagnostics,
            };
        }

        diagnostics.push(
            "known linked harness constraints left no linked harness eligible for this model after routing assessments"
                .to_string(),
        );
    }

    diagnostics
        .push("harness not set by CLI/profile/alias/provider/config; defaulting to `pi`".into());

    RoutingTrace {
        source: RouteSource::HardcodedDefault,
        selection_kind: SelectionKind::HardcodedDefault,
        match_evidence: MatchEvidence::Passthrough,
        harness: "pi".to_string(),
        harness_order_position: None,
        candidates_tried,
        assessments,
        diagnostics,
    }
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

fn filter_candidate_pairs_by_links(
    candidates: &mut Vec<(String, Option<usize>)>,
    linked_harnesses: Option<&HashSet<&str>>,
) {
    if let Some(linked_harnesses) = linked_harnesses {
        candidates.retain(|(harness, _)| linked_harnesses.contains(harness.as_str()));
    }
}

fn filter_candidates_by_links(
    candidates: Vec<String>,
    linked_harnesses: Option<&HashSet<&str>>,
) -> Vec<String> {
    let Some(linked_harnesses) = linked_harnesses else {
        return candidates;
    };

    candidates
        .into_iter()
        .filter(|harness| linked_harnesses.contains(harness.as_str()))
        .collect()
}

fn candidate_match_evidence_with_auth<F, P>(
    input: &RoutingInput<'_>,
    harness: &str,
    provider_order: Option<&[String]>,
    probe_resolver: &mut P,
    auth_check: &F,
) -> CandidateAssessment
where
    F: Fn(&str) -> bool,
    P: ProbeResolver + ?Sized,
{
    if !input.installed_harnesses.contains(harness) {
        return CandidateAssessment {
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
            return assessment_from_slug_selection(
                harness,
                selection,
                input.provider_constraint,
                true,
                &auth_check,
            );
        }

        if is_native_match(effective_provider_for_order(input).as_deref(), harness) {
            if auth_check(harness) {
                return CandidateAssessment {
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
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: Vec::new(),
                filtered_slugs: Vec::new(),
                chosen_slug: None,
                chosen_model: None,
                match_evidence: None,
                skip_reason: Some("native_auth_unavailable"),
            };
        }

        return CandidateAssessment {
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

fn assessment_from_slug_selection<F>(
    harness: &str,
    selection: SlugSelection,
    provider_constraint: Option<&str>,
    require_auth: bool,
    auth_check: &F,
) -> CandidateAssessment
where
    F: Fn(&str) -> bool,
{
    if let Some(chosen_slug) = selection.chosen_slug.clone() {
        if require_auth && !auth_check(harness) {
            return CandidateAssessment {
                harness: harness.to_string(),
                installed: true,
                candidate_slugs: selection.candidate_slugs,
                filtered_slugs: selection.filtered_slugs,
                chosen_slug: None,
                chosen_model: None,
                match_evidence: None,
                skip_reason: Some("native_auth_unavailable"),
            };
        }
        return CandidateAssessment {
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

fn is_hard_assessment_skip(skip_reason: Option<&str>) -> bool {
    matches!(
        skip_reason,
        Some(
            "pi_incompatible"
                | "no_model_match"
                | "unsupported_candidate"
                | "not_installed"
                | "provider_constraint_unsatisfied"
        )
    )
}

fn select_linked_fallback_harness(
    input: &RoutingInput<'_>,
    linked_harnesses: &[String],
    assessments: &[CandidateAssessment],
) -> Option<String> {
    let linked_set: HashSet<&str> = linked_harnesses.iter().map(String::as_str).collect();

    let walk_order: Vec<String> = input
        .settings_harness_order
        .map(|order| {
            order
                .iter()
                .filter(|harness| linked_set.contains(harness.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_else(|| linked_harnesses.to_vec());

    for harness in walk_order {
        let rejected = assessments
            .iter()
            .find(|assessment| assessment.harness == harness)
            .and_then(|assessment| assessment.skip_reason)
            .is_some_and(|reason| is_hard_assessment_skip(Some(reason)));
        if !rejected {
            return Some(harness);
        }
    }

    None
}

fn format_harness_order_fallback_warning(
    harness_order_failure: Option<&HarnessOrderFailure>,
    has_config_default_harness: bool,
    has_link_constraints: bool,
) -> Option<String> {
    let mut warning = match harness_order_failure {
        Some(HarnessOrderFailure::Empty) => "settings.harness_order is empty".to_string(),
        Some(HarnessOrderFailure::NoneInstalled { valid_candidates }) => format!(
            "settings.harness_order is set but none of [{}] are installed",
            valid_candidates.join(", ")
        ),
        None => return None,
    };

    if has_config_default_harness {
        warning.push_str("; falling through to settings.default_harness");
    } else if has_link_constraints {
        warning.push_str("; linked harness constraints prevent unrelated fallback");
    } else {
        warning.push_str("; settings.default_harness is unset, falling through to hardcoded `pi`");
    }

    Some(warning)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn always_authed(_: &str) -> bool {
        true
    }

    fn never_authed(_: &str) -> bool {
        false
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
            linked_harnesses,
            opencode_probe_result,
            pi_probe_result,
            cursor_probe_result,
            catalog_model_slugs,
        }
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
    fn linked_fallback_skips_pi_incompatible() {
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

        assert_eq!(trace.harness, "claude");
        assert_eq!(trace.selection_kind, SelectionKind::LinkedFallback);
        assert!(
            trace
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("skipped incompatible candidates"))
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
        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.selection_kind, SelectionKind::HardcodedDefault);
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
            linked_harnesses: None,
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
            linked_harnesses: None,
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
            linked_harnesses: None,
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
    fn empty_harness_order_falls_through_to_provider() {
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
                .any(|diagnostic| diagnostic.contains("settings.harness_order is empty"))
        );
    }

    #[test]
    fn uses_config_default_fallback() {
        let installed = installed(&[]);
        let input = routing_input(
            "gpt-5",
            Some("openai"),
            None,
            Some("Pi"),
            &installed,
            None,
            (None, None, None),
        );

        let trace = evaluate_candidates_with_auth(&input, never_authed);

        assert_eq!(trace.source, RouteSource::ConfigDefault);
        assert_eq!(trace.selection_kind, SelectionKind::ConfigDefault);
        assert_eq!(trace.harness, "pi");
        assert_eq!(trace.match_evidence, MatchEvidence::Passthrough);
    }

    #[test]
    fn uses_hardcoded_pi_fallback_with_warning() {
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

        assert_eq!(trace.source, RouteSource::HardcodedDefault);
        assert_eq!(trace.selection_kind, SelectionKind::HardcodedDefault);
        assert_eq!(trace.harness, "pi");
        assert!(
            trace
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.contains("defaulting to `pi`") })
        );
    }

    #[test]
    fn linked_constraints_apply_to_default_and_hardcoded_fallbacks() {
        let installed = installed(&["claude"]);
        let linked_harnesses = vec!["claude".to_string()];

        let with_config_default = routing_input(
            "claude-opus-4-7",
            Some("anthropic"),
            None,
            Some("pi"),
            &installed,
            Some(&linked_harnesses),
            (None, None, None),
        );
        let with_default_trace = evaluate_candidates_with_auth(&with_config_default, never_authed);
        assert_eq!(with_default_trace.source, RouteSource::Provider);
        assert_eq!(
            with_default_trace.selection_kind,
            SelectionKind::LinkedFallback
        );
        assert_eq!(with_default_trace.harness, "claude");
        assert_eq!(
            with_default_trace.candidates_tried,
            vec!["claude", "claude"]
        );
        assert!(with_default_trace.diagnostics.iter().any(|diagnostic| {
            diagnostic.contains(
                "settings.default_harness is excluded by known linked harness constraints",
            )
        }));

        let without_config_default = routing_input(
            "claude-opus-4-7",
            Some("anthropic"),
            None,
            None,
            &installed,
            Some(&linked_harnesses),
            (None, None, None),
        );
        let hardcoded_trace = evaluate_candidates_with_auth(&without_config_default, never_authed);
        assert_eq!(hardcoded_trace.source, RouteSource::Provider);
        assert_eq!(
            hardcoded_trace.selection_kind,
            SelectionKind::LinkedFallback
        );
        assert_eq!(hardcoded_trace.harness, "claude");
        assert!(
            hardcoded_trace
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.contains("selecting linked harness `claude`") })
        );
    }

    #[test]
    fn linked_default_harness_is_allowed_when_linked() {
        let installed = installed(&[]);
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

        assert_eq!(trace.source, RouteSource::ConfigDefault);
        assert_eq!(trace.harness, "pi");
    }

    #[test]
    fn fixed_harness_evaluation_has_no_fallback() {
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
        let assessment = evaluate_fixed_harness_with_auth(&input, "codex", never_authed);

        assert_eq!(assessment.harness, "codex");
        assert!(!assessment.installed);
        assert_eq!(assessment.match_evidence, None);
        assert_eq!(assessment.skip_reason, Some("not_installed"));
    }

    #[test]
    fn fixed_native_harness_enforces_provider_constraint() {
        let installed = installed(&["codex"]);
        let input = RoutingInput {
            model_id: "gpt-5",
            provider_for_order: Some("openai"),
            provider_constraint: Some("anthropic"),
            settings_provider_order: None,
            settings_harness_order: None,
            config_default_harness: None,
            installed_harnesses: &installed,
            linked_harnesses: None,
            opencode_probe_result: None,
            pi_probe_result: None,
            cursor_probe_result: None,
            catalog_model_slugs: None,
        };

        let assessment = evaluate_fixed_harness_with_auth(&input, "codex", always_authed);

        assert_eq!(assessment.harness, "codex");
        assert!(assessment.installed);
        assert_eq!(assessment.match_evidence, None);
        assert_eq!(
            assessment.skip_reason,
            Some("provider_constraint_unsatisfied")
        );
    }

    #[test]
    fn fixed_native_codex_accepts_openai_codex_provider_variant() {
        let installed = installed(&["codex"]);
        let input = RoutingInput {
            model_id: "gpt-5",
            provider_for_order: Some("openai-codex"),
            provider_constraint: Some("openai-codex"),
            settings_provider_order: None,
            settings_harness_order: None,
            config_default_harness: None,
            installed_harnesses: &installed,
            linked_harnesses: None,
            opencode_probe_result: None,
            pi_probe_result: None,
            cursor_probe_result: None,
            catalog_model_slugs: None,
        };

        let assessment = evaluate_fixed_harness_with_auth(&input, "codex", always_authed);

        assert_eq!(assessment.harness, "codex");
        assert!(assessment.installed);
        assert_eq!(assessment.match_evidence, Some(MatchEvidence::Constrained));
        assert_eq!(assessment.skip_reason, None);
    }

    #[test]
    fn fixed_native_claude_accepts_anthropic_claude_provider_variant() {
        let installed = installed(&["claude"]);
        let input = RoutingInput {
            model_id: "claude-opus-4-7",
            provider_for_order: Some("anthropic-claude"),
            provider_constraint: Some("anthropic-claude"),
            settings_provider_order: None,
            settings_harness_order: None,
            config_default_harness: None,
            installed_harnesses: &installed,
            linked_harnesses: None,
            opencode_probe_result: None,
            pi_probe_result: None,
            cursor_probe_result: None,
            catalog_model_slugs: None,
        };

        let assessment = evaluate_fixed_harness_with_auth(&input, "claude", always_authed);

        assert_eq!(assessment.harness, "claude");
        assert!(assessment.installed);
        assert_eq!(assessment.match_evidence, Some(MatchEvidence::Constrained));
        assert_eq!(assessment.skip_reason, None);
    }

    #[test]
    fn selected_chosen_slug_evidence_prefers_selected_harness_assessment() {
        let trace = RoutingTrace {
            source: RouteSource::Provider,
            selection_kind: SelectionKind::Auto,
            match_evidence: MatchEvidence::Confirmed,
            harness: "pi".to_string(),
            harness_order_position: None,
            candidates_tried: vec!["pi".to_string()],
            assessments: vec![
                CandidateAssessment {
                    harness: "opencode".to_string(),
                    installed: true,
                    candidate_slugs: vec!["openai/gpt-5.4-mini".to_string()],
                    filtered_slugs: vec!["openai/gpt-5.4-mini".to_string()],
                    chosen_slug: Some("openai/gpt-5.4-mini".to_string()),
                    chosen_model: Some("gpt-5.4-mini".to_string()),
                    match_evidence: Some(MatchEvidence::Confirmed),
                    skip_reason: None,
                },
                CandidateAssessment {
                    harness: "pi".to_string(),
                    installed: true,
                    candidate_slugs: vec!["openai/gpt-5.4-mini".to_string()],
                    filtered_slugs: vec!["openai/gpt-5.4-mini".to_string()],
                    chosen_slug: Some("openai/gpt-5.4-mini".to_string()),
                    chosen_model: Some("gpt-5.4-mini".to_string()),
                    match_evidence: Some(MatchEvidence::Constrained),
                    skip_reason: None,
                },
            ],
            diagnostics: vec!["diag".to_string()],
        };

        let selected = trace
            .selected_chosen_slug_evidence()
            .expect("selected slug evidence should be present");
        assert_eq!(selected.slug, "openai/gpt-5.4-mini");
        assert_eq!(selected.match_evidence, Some(MatchEvidence::Constrained));
        assert_eq!(trace.selected_harness(), "pi");
        assert_eq!(trace.selected_selection_kind(), SelectionKind::Auto);
        assert_eq!(trace.selected_match_evidence(), MatchEvidence::Confirmed);
        assert_eq!(trace.selected_diagnostics(), vec!["diag".to_string()]);
    }

    #[test]
    fn constrained_slug_selection_prefers_exact_provider_over_variant() {
        let installed = installed(&["pi"]);
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs: HashSet::from([
                "openai-codex/gpt-5.4-mini".to_string(),
                "openai/gpt-5.4-mini".to_string(),
            ]),
            ..PiProbeResult::default()
        };
        let input = RoutingInput {
            model_id: "gpt-5.4-mini",
            provider_for_order: Some("openai"),
            provider_constraint: Some("openai"),
            settings_provider_order: None,
            settings_harness_order: None,
            config_default_harness: None,
            installed_harnesses: &installed,
            linked_harnesses: None,
            opencode_probe_result: None,
            pi_probe_result: Some(&pi_probe),
            cursor_probe_result: None,
            catalog_model_slugs: None,
        };

        let trace = evaluate_candidates_with_auth(&input, always_authed);
        assert_eq!(trace.harness, "pi");
        assert_eq!(
            trace
                .selected_chosen_slug_evidence()
                .expect("selected chosen slug evidence")
                .slug,
            "openai/gpt-5.4-mini"
        );
    }

    #[test]
    fn unconstrained_slug_selection_prefers_openai_codex_variant_for_pi() {
        let installed = installed(&["pi"]);
        let pi_probe = PiProbeResult {
            compatible: true,
            model_slugs: HashSet::from([
                "openai-codex/gpt-5.4-mini".to_string(),
                "openai/gpt-5.4-mini".to_string(),
            ]),
            ..PiProbeResult::default()
        };
        let input = RoutingInput {
            model_id: "gpt-5.4-mini",
            provider_for_order: Some("openai"),
            provider_constraint: None,
            settings_provider_order: None,
            settings_harness_order: None,
            config_default_harness: None,
            installed_harnesses: &installed,
            linked_harnesses: None,
            opencode_probe_result: None,
            pi_probe_result: Some(&pi_probe),
            cursor_probe_result: None,
            catalog_model_slugs: None,
        };

        let trace = evaluate_candidates_with_auth(&input, always_authed);
        assert_eq!(trace.harness, "pi");
        assert_eq!(
            trace
                .selected_chosen_slug_evidence()
                .expect("selected chosen slug evidence")
                .slug,
            "openai-codex/gpt-5.4-mini"
        );
    }
}
