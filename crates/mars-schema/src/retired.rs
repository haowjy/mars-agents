//! Keys Mars moved off, each with the page that says how to fix it. A reader that meets one
//! warns once (`field.retired`) and does not read it. Delete this module and every key here
//! falls back to the ordinary `field.unknown`.

use crate::diagnostic::{DOCS, PathSegment, SchemaDiagnostic, Severity};
use crate::tool_policy::DISALLOWED_SPELLINGS;

/// The table a retired key sat in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Agent,
    Skill,
    Package,
    /// A name under `harness-overrides`.
    Namespace,
}

/// (scope, key, the anchor on `retired-keys.md`). The tool-policy keys are not listed: they come
/// from [`anchor`], which reads the spellings the translators already name.
const RETIRED: &[(Scope, &str, &str)] = &[
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
    (Scope::Skill, "type", "skill-type-and-when_to_use"),
    (Scope::Skill, "when_to_use", "skill-type-and-when_to_use"),
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

fn anchor(scope: Scope, key: &str) -> Option<&'static str> {
    let listed = RETIRED
        .iter()
        .find(|(known_scope, known, _)| *known_scope == scope && *known == key)
        .map(|(_, _, anchor)| *anchor);
    listed.or_else(|| match scope {
        Scope::Agent if DISALLOWED_SPELLINGS.contains(&key) => Some("agent-deny-lists"),
        Scope::Skill if crate::skill::tool_keys().any(|known| known == key) => {
            Some("skill-tool-policy")
        }
        _ => None,
    })
}

/// The warning for `key` of `scope` at `path`, when Mars retired it.
pub(crate) fn diagnostic(
    scope: Scope,
    key: &str,
    path: Vec<PathSegment>,
) -> Option<SchemaDiagnostic> {
    let anchor = anchor(scope, key)?;
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
