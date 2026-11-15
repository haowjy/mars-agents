//! Keys Mars moved off, each with the page that says how to fix it. A reader that meets one
//! warns once (`field.retired`) and does not read it. Delete this module and every key here
//! falls back to the ordinary `field.unknown`.

use crate::diagnostic::{DOCS, PathSegment, SchemaDiagnostic, Severity};

/// The table a retired key sat in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Agent,
    Skill,
    Package,
    /// A name under `harness-overrides`.
    Namespace,
}

/// (scope, key, the anchor on [`PAGE`]).
const RETIRED: &[(Scope, &str, &str)] = &[
    (Scope::Agent, "disallowed-tools", "agent-deny-lists"),
    (Scope::Agent, "disallowed_tools", "agent-deny-lists"),
    (Scope::Agent, "disallowedTools", "agent-deny-lists"),
    (Scope::Agent, "mcp-tools", "agent-mcp-tools"),
    (
        Scope::Agent,
        "model_invocable",
        "agent-underscore-spellings",
    ),
    (Scope::Agent, "user_invocable", "agent-underscore-spellings"),
    (Scope::Agent, "network_allow", "agent-underscore-spellings"),
    (Scope::Agent, "network_deny", "agent-underscore-spellings"),
    (Scope::Agent, "permission", "agent-host-keys"),
    (Scope::Agent, "resident-rearm-budget", "agent-host-keys"),
    (Scope::Agent, "meridian-capabilities", "agent-host-keys"),
    (Scope::Agent, "fanout", "agent-fanout"),
    (Scope::Agent, "models", "agent-models"),
    (Scope::Agent, "autocompact", "agent-autocompact"),
    (Scope::Agent, "autocompact_pct", "agent-autocompact"),
    (Scope::Skill, "type", "skill-type-and-when-to-use"),
    (Scope::Skill, "when_to_use", "skill-type-and-when-to-use"),
    (Scope::Skill, "tools", "skill-tool-policy"),
    (Scope::Skill, "allowed-tools", "skill-tool-policy"),
    (Scope::Skill, "allowed_tools", "skill-tool-policy"),
    (Scope::Skill, "disallowed-tools", "skill-tool-policy"),
    (Scope::Skill, "disallowed_tools", "skill-tool-policy"),
    (Scope::Skill, "disallowedTools", "skill-tool-policy"),
    (
        Scope::Skill,
        "model_invocable",
        "agent-underscore-spellings",
    ),
    (Scope::Skill, "user_invocable", "agent-underscore-spellings"),
    (Scope::Skill, "disable-model-invocation", "skill-invocation"),
    (Scope::Skill, "invocation", "skill-invocation"),
    (
        Scope::Skill,
        "allow_implicit_invocation",
        "skill-invocation",
    ),
    (Scope::Package, "visibility", "package-visibility"),
    (Scope::Namespace, "flow", "host-namespaces"),
    (Scope::Namespace, "meridian", "host-namespaces"),
];

/// The warning for `key` of `scope` at `path`, when Mars retired it.
pub(crate) fn diagnostic(
    scope: Scope,
    key: &str,
    path: Vec<PathSegment>,
) -> Option<SchemaDiagnostic> {
    let (_, _, anchor) = RETIRED
        .iter()
        .find(|(known_scope, known, _)| *known_scope == scope && *known == key)?;
    let name = match scope {
        Scope::Package => format!("package.{key}"),
        Scope::Namespace => format!("harness-overrides.{key}"),
        Scope::Agent | Scope::Skill => key.to_owned(),
    };
    Some(SchemaDiagnostic::new(
        "field.retired",
        Severity::Warning,
        path,
        format!(
            "`{name}` is a key Mars no longer reads, so what it set has no effect: {DOCS}/retired-keys.md#{anchor}"
        ),
    ))
}
