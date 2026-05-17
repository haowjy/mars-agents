// qa-validated: launch-bundle-blocker-audit
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
fn build_launch_bundle_rejects_prompt_file_flag() {
    schema::build_launch_bundle_rejects_prompt_file_flag();
}

#[test]
fn build_launch_bundle_fails_when_no_model_available() {
    schema::build_launch_bundle_fails_when_no_model_available();
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
fn build_launch_bundle_uses_harness_variant_skill_for_codex() {
    prompt_surface::build_launch_bundle_uses_harness_variant_skill_for_codex();
}

#[test]
fn build_launch_bundle_uses_harness_override_skills_for_prompt_surface() {
    prompt_surface::build_launch_bundle_uses_harness_override_skills_for_prompt_surface();
}

#[test]
fn build_launch_bundle_skips_model_non_invocable_skills() {
    prompt_surface::build_launch_bundle_skips_model_non_invocable_skills();
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
fn build_launch_bundle_uses_provider_harness_for_openai_model_when_alias_has_no_harness() {
    routing::build_launch_bundle_uses_provider_harness_for_openai_model_when_alias_has_no_harness();
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
fn build_launch_bundle_invalid_settings_default_harness_warns_and_falls_back_to_default() {
    routing::build_launch_bundle_invalid_settings_default_harness_warns_and_falls_back_to_default();
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
