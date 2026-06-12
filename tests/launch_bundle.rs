// qa-validated: launch-bundle-blocker-audit
// qa-validated: harness-order-settings-audit
// qa-validated: capability-cache-resolver-routing-gaps
#[path = "common/mod.rs"]
mod test_common;

#[path = "launch_bundle/common.rs"]
mod common;
#[path = "launch_bundle/cursor.rs"]
mod cursor;
#[path = "launch_bundle/errors.rs"]
mod errors;
#[path = "launch_bundle/execution_policy.rs"]
mod execution_policy;
#[path = "launch_bundle/native_config.rs"]
mod native_config;
#[path = "launch_bundle/prompt_surface.rs"]
mod prompt_surface;
#[path = "launch_bundle/routing.rs"]
mod routing;
#[path = "launch_bundle/schema.rs"]
mod schema;
#[path = "launch_bundle/tool_policy.rs"]
mod tool_policy;

// Keep root-level #[test] wrappers so discovered test names stay historically stable
// while test bodies remain organized in contract-focused modules.

#[test]
fn build_launch_bundle_outputs_schema_and_slot_placeholders() {
    schema::build_launch_bundle_outputs_schema_and_slot_placeholders();
}

#[test]
fn build_launch_bundle_supports_ad_hoc_mode_with_model_override() {
    schema::build_launch_bundle_supports_ad_hoc_mode_with_model_override();
}

#[test]
fn build_launch_bundle_ad_hoc_without_mars_toml() {
    schema::build_launch_bundle_ad_hoc_without_mars_toml();
}

#[test]
fn build_launch_bundle_ad_hoc_supports_skills_missing_metadata_and_execution_overrides() {
    schema::build_launch_bundle_ad_hoc_supports_skills_missing_metadata_and_execution_overrides();
}

#[test]
fn build_launch_bundle_rejects_prompt_file_flag() {
    schema::build_launch_bundle_rejects_prompt_file_flag();
}

#[test]
fn build_launch_bundle_uses_installed_harness_default_when_no_model_available() {
    schema::build_launch_bundle_uses_installed_harness_default_when_no_model_available();
}

#[test]
fn build_launch_bundle_ad_hoc_without_model_uses_installed_harness_default() {
    schema::build_launch_bundle_ad_hoc_without_model_uses_installed_harness_default();
}

#[test]
fn build_launch_bundle_resolves_model_alias_from_consumer_config() {
    schema::build_launch_bundle_resolves_model_alias_from_consumer_config();
}

#[test]
fn build_launch_bundle_includes_skill_documents_and_system_instruction() {
    prompt_surface::build_launch_bundle_includes_skill_documents_and_system_instruction();
}

#[test]
fn build_launch_bundle_splits_loaded_and_available_skills() {
    prompt_surface::build_launch_bundle_splits_loaded_and_available_skills();
}

#[test]
fn build_launch_bundle_uses_harness_variant_skill_for_codex() {
    prompt_surface::build_launch_bundle_uses_harness_variant_skill_for_codex();
}

#[test]
fn build_launch_bundle_uses_harness_override_skills_for_prompt_surface() {
    prompt_surface::build_launch_bundle_uses_harness_override_skills_for_prompt_surface();
}

#[test]
fn build_launch_bundle_loads_model_non_invocable_skills_when_explicit() {
    prompt_surface::build_launch_bundle_loads_model_non_invocable_skills_when_explicit();
}

#[test]
fn build_launch_bundle_includes_inventory_prompt_before_report_block() {
    prompt_surface::build_launch_bundle_includes_inventory_prompt_before_report_block();
}

#[test]
fn build_launch_bundle_orders_skills_by_type_and_bookends_principles() {
    prompt_surface::build_launch_bundle_orders_skills_by_type_and_bookends_principles();
}

#[test]
fn build_launch_bundle_fanout_agent_dual_lists_in_inventory() {
    prompt_surface::build_launch_bundle_fanout_agent_dual_lists_in_inventory();
}

#[test]
fn build_launch_bundle_inventory_hides_model_non_invocable_agents_and_shows_fanout() {
    prompt_surface::build_launch_bundle_inventory_hides_model_non_invocable_agents_and_shows_fanout(
    );
}

#[test]
fn build_launch_bundle_merges_extra_skills_after_profile_dedupes_and_tracks_missing() {
    prompt_surface::build_launch_bundle_merges_extra_skills_after_profile_dedupes_and_tracks_missing();
}

#[test]
fn build_launch_bundle_has_canonical_prompt_surface_for_small_fixture() {
    prompt_surface::build_launch_bundle_has_canonical_prompt_surface_for_small_fixture();
}

#[test]
fn build_launch_bundle_cli_model_alias_harness_beats_profile_harness() {
    routing::build_launch_bundle_cli_model_alias_harness_beats_profile_harness();
}

#[test]
fn build_launch_bundle_cli_model_override_uses_provider_harness_before_profile_harness() {
    routing::build_launch_bundle_cli_model_override_uses_provider_harness_before_profile_harness();
}

#[test]
fn build_launch_bundle_openai_falls_back_to_pi_when_codex_missing() {
    routing::build_launch_bundle_openai_falls_back_to_pi_when_codex_missing();
}

#[test]
fn build_launch_bundle_openai_falls_back_to_pi_when_codex_auth_fails() {
    routing::build_launch_bundle_openai_falls_back_to_pi_when_codex_auth_fails();
}

#[test]
fn build_launch_bundle_anthropic_falls_back_to_pi_when_claude_missing() {
    routing::build_launch_bundle_anthropic_falls_back_to_pi_when_claude_missing();
}

#[test]
fn build_launch_bundle_anthropic_falls_back_to_pi_when_claude_auth_fails() {
    routing::build_launch_bundle_anthropic_falls_back_to_pi_when_claude_auth_fails();
}

#[test]
fn build_launch_bundle_google_model_prefers_pi_and_never_gemini_harness() {
    routing::build_launch_bundle_google_model_prefers_pi_and_never_gemini_harness();
}

#[test]
fn build_launch_bundle_builtin_gemini_model_alias_resolves_to_google_model_and_pi_harness() {
    routing::build_launch_bundle_builtin_gemini_model_alias_resolves_to_google_model_and_pi_harness(
    );
}

#[test]
fn build_launch_bundle_openai_falls_back_to_opencode_with_cached_capability_evidence() {
    routing::build_launch_bundle_openai_falls_back_to_opencode_with_cached_capability_evidence();
}

#[test]
fn build_launch_bundle_prefers_pi_over_opencode_even_with_positive_opencode_cache() {
    routing::build_launch_bundle_prefers_pi_over_opencode_even_with_positive_opencode_cache();
}

#[test]
fn build_launch_bundle_prefers_cursor_before_opencode_when_both_installed() {
    routing::build_launch_bundle_prefers_cursor_before_opencode_when_both_installed();
}

#[test]
fn build_launch_bundle_falls_back_to_cursor_when_opencode_cache_is_negative() {
    routing::build_launch_bundle_falls_back_to_cursor_when_opencode_cache_is_negative();
}

#[test]
fn build_launch_bundle_cursor_effort_bakes_slug_into_harness_model() {
    routing::build_launch_bundle_cursor_effort_bakes_slug_into_harness_model();
}

#[test]
fn build_launch_bundle_cursor_medium_effort_uses_unsuffixed_slug() {
    routing::build_launch_bundle_cursor_medium_effort_uses_unsuffixed_slug();
}

#[test]
fn build_launch_bundle_cursor_composer_effort_falls_back_to_bare_slug() {
    routing::build_launch_bundle_cursor_composer_effort_falls_back_to_bare_slug();
}

#[test]
fn build_launch_bundle_cursor_non_composer_missing_effort_variant_errors() {
    routing::build_launch_bundle_cursor_non_composer_missing_effort_variant_errors();
}

#[test]
fn build_launch_bundle_cursor_effort_probe_unavailable_errors_with_probe_message() {
    routing::build_launch_bundle_cursor_effort_probe_unavailable_errors_with_probe_message();
}

#[test]
fn build_launch_bundle_cursor_effort_probe_failure_errors_with_probe_failure_message() {
    routing::build_launch_bundle_cursor_effort_probe_failure_errors_with_probe_failure_message();
}

#[test]
fn build_launch_bundle_cursor_effort_no_prefix_match_errors_with_catalog_message() {
    routing::build_launch_bundle_cursor_effort_no_prefix_match_errors_with_catalog_message();
}

#[test]
fn build_launch_bundle_openai_falls_back_to_cursor_when_only_cursor_installed() {
    routing::build_launch_bundle_openai_falls_back_to_cursor_when_only_cursor_installed();
}

#[test]
fn build_launch_bundle_selects_opencode_when_opencode_cache_is_stale() {
    routing::build_launch_bundle_selects_opencode_when_opencode_cache_is_stale();
}

#[test]
fn build_launch_bundle_no_refresh_uses_stale_probe_without_spawning_refresh() {
    routing::build_launch_bundle_no_refresh_uses_stale_probe_without_spawning_refresh();
}

#[test]
fn build_launch_bundle_no_refresh_uses_stale_cursor_probe_without_spawning_refresh() {
    routing::build_launch_bundle_no_refresh_uses_stale_cursor_probe_without_spawning_refresh();
}

#[test]
fn build_launch_bundle_refresh_models_sync_probe_updates_stale_routing() {
    routing::build_launch_bundle_refresh_models_sync_probe_updates_stale_routing();
}

#[test]
fn build_launch_bundle_unknown_model_without_passthrough_harness_errors() {
    routing::build_launch_bundle_unknown_model_without_passthrough_harness_errors();
}

#[test]
fn build_launch_bundle_provider_order_prefers_configured_provider_over_first_seen_slug() {
    routing::build_launch_bundle_provider_order_prefers_configured_provider_over_first_seen_slug();
}

#[test]
fn build_launch_bundle_provider_order_unknown_provider_warns_in_route_trace() {
    routing::build_launch_bundle_provider_order_unknown_provider_warns_in_route_trace();
}

#[test]
fn build_launch_bundle_nested_slug_model_id_does_not_flatten_into_bare_match() {
    routing::build_launch_bundle_nested_slug_model_id_does_not_flatten_into_bare_match();
}

#[test]
fn build_launch_bundle_uses_provider_harness_for_openai_model_when_alias_has_no_harness() {
    routing::build_launch_bundle_uses_provider_harness_for_openai_model_when_alias_has_no_harness();
}

#[test]
fn build_launch_bundle_resolves_harness_model_from_cached_opencode_probe() {
    routing::build_launch_bundle_resolves_harness_model_from_cached_opencode_probe();
}

#[test]
fn build_launch_bundle_pi_harness_resolves_qualified_harness_model() {
    routing::build_launch_bundle_pi_harness_resolves_qualified_harness_model();
}

#[test]
fn build_launch_bundle_pi_harness_order_before_codex_selects_pi_slug() {
    routing::build_launch_bundle_pi_harness_order_before_codex_selects_pi_slug();
}

#[test]
fn build_launch_bundle_pi_harness_preserves_qualified_model_token() {
    routing::build_launch_bundle_pi_harness_preserves_qualified_model_token();
}

#[test]
fn build_launch_bundle_synthesizes_opencode_model_when_cache_missing() {
    routing::build_launch_bundle_synthesizes_opencode_model_when_cache_missing();
}

#[test]
fn build_launch_bundle_explicit_unknown_harness_model_path_clears_and_warns() {
    routing::build_launch_bundle_explicit_unknown_harness_model_path_clears_and_warns();
}

#[test]
fn build_launch_bundle_alias_fixed_native_harness_rejects_mismatched_provider_constraint() {
    routing::build_launch_bundle_alias_fixed_native_harness_rejects_mismatched_provider_constraint(
    );
}

#[test]
fn build_launch_bundle_alias_fixed_native_harness_accepts_provider_variant_and_marks_provider_match()
 {
    routing::build_launch_bundle_alias_fixed_native_harness_accepts_provider_variant_and_marks_provider_match();
}

#[test]
fn build_launch_bundle_uses_alias_provider_when_auto_resolve_misses_model_cache() {
    routing::build_launch_bundle_uses_alias_provider_when_auto_resolve_misses_model_cache();
}

#[test]
fn build_launch_bundle_uses_settings_default_harness_before_hardcoded_fallback() {
    routing::build_launch_bundle_uses_settings_default_harness_before_hardcoded_fallback();
}

#[test]
fn build_launch_bundle_cli_direct_model_id_prefers_provider_harness_over_profile() {
    routing::build_launch_bundle_cli_direct_model_id_prefers_provider_harness_over_profile();
}

#[test]
fn build_launch_bundle_uses_settings_default_model_when_profile_and_cli_missing() {
    routing::build_launch_bundle_uses_settings_default_model_when_profile_and_cli_missing();
}

#[test]
fn build_launch_bundle_cli_model_override_beats_settings_default_model() {
    routing::build_launch_bundle_cli_model_override_beats_settings_default_model();
}

#[test]
fn build_launch_bundle_profile_model_beats_settings_default_model() {
    routing::build_launch_bundle_profile_model_beats_settings_default_model();
}

#[test]
fn build_launch_bundle_overlay_model_overrides_profile_model() {
    routing::build_launch_bundle_overlay_model_overrides_profile_model();
}

#[test]
fn build_launch_bundle_settings_model_policy_applies_with_provenance() {
    routing::build_launch_bundle_settings_model_policy_applies_with_provenance();
}

#[test]
fn build_launch_bundle_composed_model_policies_overlay_wins() {
    routing::build_launch_bundle_composed_model_policies_overlay_wins();
}

#[test]
fn build_launch_bundle_composed_model_policies_first_match_wins() {
    routing::build_launch_bundle_composed_model_policies_first_match_wins();
}

#[test]
fn build_launch_bundle_local_overlay_replaces_base_overlay_by_name() {
    routing::build_launch_bundle_local_overlay_replaces_base_overlay_by_name();
}

#[test]
fn build_launch_bundle_rejects_legacy_lock_missing_dependency_alias_authority() {
    routing::build_launch_bundle_rejects_legacy_lock_missing_dependency_alias_authority();
}

#[test]
fn build_launch_bundle_invalid_settings_default_harness_warns_and_falls_back_to_default() {
    routing::build_launch_bundle_invalid_settings_default_harness_warns_and_falls_back_to_default();
}

#[test]
fn build_launch_bundle_provider_fallback_skips_non_launch_bundle_harnesses() {
    routing::build_launch_bundle_provider_fallback_skips_non_launch_bundle_harnesses();
}

#[test]
fn build_launch_bundle_uses_settings_harness_order_before_default_harness() {
    routing::build_launch_bundle_uses_settings_harness_order_before_default_harness();
}

#[test]
fn build_launch_bundle_local_settings_harness_order_overrides_project_order() {
    routing::build_launch_bundle_local_settings_harness_order_overrides_project_order();
}

#[test]
fn build_launch_bundle_fails_when_local_settings_cannot_parse() {
    routing::build_launch_bundle_fails_when_local_settings_cannot_parse();
}

#[test]
fn build_launch_bundle_settings_harness_order_runs_gate_checks_before_selection() {
    routing::build_launch_bundle_settings_harness_order_runs_gate_checks_before_selection();
}

#[test]
fn build_launch_bundle_legacy_harness_link_filters_ambient_path_candidates() {
    routing::build_launch_bundle_legacy_harness_link_filters_ambient_path_candidates();
}

#[test]
fn build_launch_bundle_link_constraints_block_unrelated_default_fallbacks() {
    routing::build_launch_bundle_link_constraints_block_unrelated_default_fallbacks();
}

#[test]
fn build_launch_bundle_model_policy_fallback_uses_linked_harness() {
    routing::build_launch_bundle_model_policy_fallback_uses_linked_harness();
}

#[test]
fn build_launch_bundle_model_policy_fallback_walks_chain() {
    routing::build_launch_bundle_model_policy_fallback_walks_chain();
}

#[test]
fn build_launch_bundle_model_policy_fallback_exhaustion_errors() {
    routing::build_launch_bundle_model_policy_fallback_exhaustion_errors();
}

#[test]
fn build_launch_bundle_model_policy_fallback_skips_no_fallback_rules() {
    routing::build_launch_bundle_model_policy_fallback_skips_no_fallback_rules();
}

#[test]
fn build_launch_bundle_cli_model_override_does_not_apply_model_policy_fallback() {
    routing::build_launch_bundle_cli_model_override_does_not_apply_model_policy_fallback();
}

#[test]
fn build_launch_bundle_cli_harness_override_beats_settings_harness_order() {
    routing::build_launch_bundle_cli_harness_override_beats_settings_harness_order();
}

#[test]
fn build_launch_bundle_profile_harness_beats_settings_harness_order() {
    routing::build_launch_bundle_profile_harness_beats_settings_harness_order();
}

#[test]
fn build_launch_bundle_unavailable_profile_harness_pivots_to_installed_candidate() {
    routing::build_launch_bundle_unavailable_profile_harness_pivots_to_installed_candidate();
}

#[test]
fn build_launch_bundle_unavailable_profile_harness_errors_without_installed_fallback() {
    routing::build_launch_bundle_unavailable_profile_harness_errors_without_installed_fallback();
}

#[test]
fn build_launch_bundle_profile_harness_without_installed_harnesses_uses_passthrough_candidate() {
    routing::build_launch_bundle_profile_harness_without_installed_harnesses_uses_passthrough_candidate();
}

#[test]
fn build_launch_bundle_unavailable_cli_harness_errors_without_pivoting() {
    routing::build_launch_bundle_unavailable_cli_harness_errors_without_pivoting();
}

#[test]
fn build_launch_bundle_cli_harness_soft_fail_clears_profile_model_in_final_routing() {
    routing::build_launch_bundle_cli_harness_soft_fail_clears_profile_model_in_final_routing();
}

#[test]
fn build_launch_bundle_alias_harness_beats_settings_harness_order() {
    routing::build_launch_bundle_alias_harness_beats_settings_harness_order();
}

#[test]
fn build_launch_bundle_cli_model_override_uses_settings_harness_order_before_profile_harness() {
    routing::build_launch_bundle_cli_model_override_uses_settings_harness_order_before_profile_harness();
}

#[test]
fn build_launch_bundle_all_invalid_harness_order_warns_and_falls_through_to_default_harness() {
    routing::build_launch_bundle_all_invalid_harness_order_warns_and_falls_through_to_default_harness();
}

#[test]
fn build_launch_bundle_harness_order_none_installed_uses_default_harness() {
    routing::build_launch_bundle_harness_order_none_installed_uses_default_harness();
}

#[test]
fn build_launch_bundle_settings_default_harness_accepts_case_insensitive_name() {
    routing::build_launch_bundle_settings_default_harness_accepts_case_insensitive_name();
}

#[test]
fn build_launch_bundle_cli_overrides_profile_execution_policy_fields() {
    execution_policy::build_launch_bundle_cli_overrides_profile_execution_policy_fields();
}

#[test]
fn build_launch_bundle_harness_override_execution_policy_applies_before_profile_and_alias() {
    execution_policy::build_launch_bundle_harness_override_execution_policy_applies_before_profile_and_alias();
}

#[test]
fn build_launch_bundle_profile_execution_policy_flows_without_cli_override() {
    execution_policy::build_launch_bundle_profile_execution_policy_flows_without_cli_override();
}

#[test]
fn build_launch_bundle_preserves_mixed_tool_allow_deny_and_harness_override_replacement() {
    tool_policy::build_launch_bundle_preserves_mixed_tool_allow_deny_and_harness_override_replacement();
}

#[test]
fn build_launch_bundle_normalizes_tool_head_and_preserves_scoped_payload() {
    tool_policy::build_launch_bundle_normalizes_tool_head_and_preserves_scoped_payload();
}

#[test]
fn build_launch_bundle_warns_for_unknown_first_class_tool_and_preserves_mcp() {
    tool_policy::build_launch_bundle_warns_for_unknown_first_class_tool_and_preserves_mcp();
}

#[test]
fn build_launch_bundle_opencode_tool_normalization_maps_web_aliases_and_warns_unknown() {
    tool_policy::build_launch_bundle_opencode_tool_normalization_maps_web_aliases_and_warns_unknown(
    );
}

#[test]
fn build_launch_bundle_cursor_and_pi_unknown_tools_pass_silently() {
    tool_policy::build_launch_bundle_cursor_and_pi_unknown_tools_pass_silently();
}

#[test]
fn build_launch_bundle_accepts_cursor_harness_flag_and_marks_experimental() {
    cursor::build_launch_bundle_accepts_cursor_harness_flag_and_marks_experimental();
}

#[test]
fn build_launch_bundle_accepts_profile_cursor_harness() {
    cursor::build_launch_bundle_accepts_profile_cursor_harness();
}

#[test]
fn build_launch_bundle_cursor_alias_uses_cursor_overrides_for_model_facing_policy() {
    cursor::build_launch_bundle_cursor_alias_uses_cursor_overrides_for_model_facing_policy();
}

#[test]
fn build_launch_bundle_emits_native_config_for_resolved_harness_and_keeps_prompt_clean() {
    native_config::build_launch_bundle_emits_native_config_for_resolved_harness_and_keeps_prompt_clean();
}

#[test]
fn build_launch_bundle_invalid_native_config_shape_fails_with_diagnostic() {
    native_config::build_launch_bundle_invalid_native_config_shape_fails_with_diagnostic();
}

#[test]
fn build_launch_bundle_fails_on_unknown_agent_harness() {
    errors::build_launch_bundle_fails_on_unknown_agent_harness();
}

#[test]
fn build_launch_bundle_fails_on_invalid_top_level_agent_field_value() {
    errors::build_launch_bundle_fails_on_invalid_top_level_agent_field_value();
}

#[test]
fn build_launch_bundle_fails_on_non_overridable_model_invocable_override() {
    errors::build_launch_bundle_fails_on_non_overridable_model_invocable_override();
}

#[test]
fn build_launch_bundle_fails_when_inventory_agent_has_fatal_frontmatter_diagnostic() {
    errors::build_launch_bundle_fails_when_inventory_agent_has_fatal_frontmatter_diagnostic();
}

#[test]
fn build_launch_bundle_fails_when_agent_file_missing() {
    errors::build_launch_bundle_fails_when_agent_file_missing();
}
