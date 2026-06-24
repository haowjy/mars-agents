//! Typed compiler harness descriptors used by native agent and skill lowering.
//!
//! This module is the compiler-side owner for facts that used to be duplicated
//! across lowerers: canonical ids, skill variant keys, tool-name conventions,
//! MCP projection policy, and lowering-policy selection hooks. The
//! descriptor is keyed by [`HarnessKind`] so compiler code does not re-match raw
//! `"claude"`/`"codex"` strings at each call site.

use crate::compiler::agents::HarnessKind;
use crate::config::Settings;
use crate::harness::registry::HarnessId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolNamingConvention {
    PascalCase,
    SnakeCase,
    Lowercase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpProjectionPolicy {
    ClaudeToolList,
    CodexServerConfig,
    CursorToolList,
    OpenCodeToolList,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentLoweringPolicyKind {
    Claude,
    Codex,
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillLoweringPolicyKind {
    Claude,
    Codex,
    OpenCode,
    Pi,
    Cursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompilerHarnessDescriptor {
    pub kind: HarnessKind,
    pub id: HarnessId,
    pub canonical_id: &'static str,
    pub variant_key: &'static str,
    pub target_dir: &'static str,
    pub tool_naming: ToolNamingConvention,
    pub mcp_projection: McpProjectionPolicy,
    pub agent_policy: AgentLoweringPolicyKind,
    pub skill_policy: SkillLoweringPolicyKind,
}

const DESCRIPTORS: &[CompilerHarnessDescriptor] = &[
    CompilerHarnessDescriptor {
        kind: HarnessKind::Claude,
        id: HarnessId::Claude,
        canonical_id: "claude",
        variant_key: "claude",
        target_dir: ".claude",
        tool_naming: ToolNamingConvention::PascalCase,
        mcp_projection: McpProjectionPolicy::ClaudeToolList,
        agent_policy: AgentLoweringPolicyKind::Claude,
        skill_policy: SkillLoweringPolicyKind::Claude,
    },
    CompilerHarnessDescriptor {
        kind: HarnessKind::Codex,
        id: HarnessId::Codex,
        canonical_id: "codex",
        variant_key: "codex",
        target_dir: ".codex",
        tool_naming: ToolNamingConvention::SnakeCase,
        mcp_projection: McpProjectionPolicy::CodexServerConfig,
        agent_policy: AgentLoweringPolicyKind::Codex,
        skill_policy: SkillLoweringPolicyKind::Codex,
    },
    CompilerHarnessDescriptor {
        kind: HarnessKind::Pi,
        id: HarnessId::Pi,
        canonical_id: "pi",
        variant_key: "pi",
        target_dir: ".pi",
        tool_naming: ToolNamingConvention::Lowercase,
        mcp_projection: McpProjectionPolicy::Unsupported,
        agent_policy: AgentLoweringPolicyKind::Markdown,
        skill_policy: SkillLoweringPolicyKind::Pi,
    },
    CompilerHarnessDescriptor {
        kind: HarnessKind::OpenCode,
        id: HarnessId::OpenCode,
        canonical_id: "opencode",
        variant_key: "opencode",
        target_dir: ".opencode",
        tool_naming: ToolNamingConvention::Lowercase,
        mcp_projection: McpProjectionPolicy::OpenCodeToolList,
        agent_policy: AgentLoweringPolicyKind::Markdown,
        skill_policy: SkillLoweringPolicyKind::OpenCode,
    },
    CompilerHarnessDescriptor {
        kind: HarnessKind::Cursor,
        id: HarnessId::Cursor,
        canonical_id: "cursor",
        variant_key: "cursor",
        target_dir: ".cursor",
        tool_naming: ToolNamingConvention::PascalCase,
        mcp_projection: McpProjectionPolicy::CursorToolList,
        agent_policy: AgentLoweringPolicyKind::Markdown,
        skill_policy: SkillLoweringPolicyKind::Cursor,
    },
];

pub(crate) fn descriptor(kind: HarnessKind) -> &'static CompilerHarnessDescriptor {
    DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.kind == kind)
        .expect("compiler harness descriptor exists")
}

pub(crate) fn descriptor_for_variant_key(key: &str) -> Option<&'static CompilerHarnessDescriptor> {
    let normalized = key.trim().to_ascii_lowercase();
    DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.variant_key == normalized)
}

pub(crate) fn descriptor_for_canonical_id(id: &str) -> Option<&'static CompilerHarnessDescriptor> {
    let normalized = id.trim().to_ascii_lowercase();
    DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.canonical_id == normalized)
}

pub(crate) fn known_canonical_ids() -> impl Iterator<Item = &'static str> {
    DESCRIPTORS.iter().map(|descriptor| descriptor.canonical_id)
}

/// Harnesses that would receive native agent artifacts during sync for these settings.
pub(crate) fn configured_emit_harnesses(settings: &Settings) -> Vec<HarnessKind> {
    settings
        .managed_targets()
        .iter()
        .filter_map(|target| {
            DESCRIPTORS
                .iter()
                .find(|descriptor| descriptor.target_dir == target)
                .map(|descriptor| descriptor.kind)
        })
        .collect()
}
