use std::collections::HashSet;

use crate::config::routing_settings::ResolvedRoutingSettings;
use crate::models::probes::{CursorProbeResult, OpenCodeProbeResult, PiProbeResult};

use super::RoutingInput;

pub struct RoutingEvidence<'a> {
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
    pub catalog_model_slugs: Option<&'a [String]>,
}

impl<'a> RoutingEvidence<'a> {
    pub fn routing_input(&self) -> RoutingInput<'_> {
        self.routing_input_with_config_default_harness(self.config_default_harness)
    }

    pub fn routing_input_with_config_default_harness(
        &'a self,
        config_default_harness: Option<&'a str>,
    ) -> RoutingInput<'a> {
        RoutingInput {
            model_id: self.model_id,
            provider_for_order: self.provider_for_order,
            provider_constraint: self.provider_constraint,
            settings_provider_order: self.settings_provider_order,
            settings_harness_order: self.settings_harness_order,
            config_default_harness,
            installed_harnesses: self.installed_harnesses,
            linked_harnesses: self.linked_harnesses,
            opencode_probe_result: self.opencode_probe_result,
            pi_probe_result: self.pi_probe_result,
            cursor_probe_result: self.cursor_probe_result,
            catalog_model_slugs: self.catalog_model_slugs,
        }
    }
}

pub struct RoutingSettingsEvidence<'a> {
    pub model_id: &'a str,
    pub provider_for_order: Option<&'a str>,
    pub provider_constraint: Option<&'a str>,
    pub installed_harnesses: &'a HashSet<String>,
    pub opencode_probe_result: Option<&'a OpenCodeProbeResult>,
    pub pi_probe_result: Option<&'a PiProbeResult>,
    pub cursor_probe_result: Option<&'a CursorProbeResult>,
    pub catalog_model_slugs: Option<&'a [String]>,
    provider_order: Option<Vec<String>>,
    harness_order: Option<Vec<String>>,
    default_harness: Option<String>,
    linked_harnesses: Vec<String>,
}

impl<'a> RoutingSettingsEvidence<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model_id: &'a str,
        provider_for_order: Option<&'a str>,
        provider_constraint: Option<&'a str>,
        installed_harnesses: &'a HashSet<String>,
        opencode_probe_result: Option<&'a OpenCodeProbeResult>,
        pi_probe_result: Option<&'a PiProbeResult>,
        cursor_probe_result: Option<&'a CursorProbeResult>,
        catalog_model_slugs: Option<&'a [String]>,
        routing_settings: &ResolvedRoutingSettings,
    ) -> Self {
        Self {
            model_id,
            provider_for_order,
            provider_constraint,
            installed_harnesses,
            opencode_probe_result,
            pi_probe_result,
            cursor_probe_result,
            catalog_model_slugs,
            provider_order: routing_settings.provider_order_names(),
            harness_order: routing_settings.harness_order_names(),
            default_harness: routing_settings.default_harness_name(),
            linked_harnesses: routing_settings.linked_harness_names(),
        }
    }

    pub fn routing_input(&self) -> RoutingInput<'_> {
        RoutingInput {
            model_id: self.model_id,
            provider_for_order: self.provider_for_order,
            provider_constraint: self.provider_constraint,
            settings_provider_order: self.provider_order.as_deref(),
            settings_harness_order: self.harness_order.as_deref(),
            config_default_harness: self.default_harness.as_deref(),
            installed_harnesses: self.installed_harnesses,
            linked_harnesses: (!self.linked_harnesses.is_empty())
                .then_some(self.linked_harnesses.as_slice()),
            opencode_probe_result: self.opencode_probe_result,
            pi_probe_result: self.pi_probe_result,
            cursor_probe_result: self.cursor_probe_result,
            catalog_model_slugs: self.catalog_model_slugs,
        }
    }
}
