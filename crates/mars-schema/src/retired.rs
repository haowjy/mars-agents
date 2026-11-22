//! Keys and target directories Mars moved off, each with the page that says how to fix it. A
//! reader that meets a key warns once (`field.retired`) and does not read it. Delete this module and every key here
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
    (Scope::Namespace, "cursor", "cursor"),
];

/// A target directory Mars no longer writes. Sync leaves it and what it recorded there alone;
/// `mars unlink` removes what the lock records, which for the config files below is the
/// entries Mars added, not the files.
pub struct RetiredTarget {
    pub dir: &'static str,
    /// The anchor on `retired-keys.md`.
    pub anchor: &'static str,
    /// The JSON file, from the project root, that held MCP servers, and the object in it.
    pub mcp_path: &'static str,
    pub mcp_section: &'static str,
    /// The file under `dir` that held hook entries, under a `hooks` object.
    pub hook_file: &'static str,
    /// Keys Mars wrote beside `hooks`; a file holding only these is empty and `mars unlink`
    /// deletes it.
    pub hook_file_keys: &'static [&'static str],
}

pub const RETIRED_TARGETS: &[RetiredTarget] = &[RetiredTarget {
    dir: ".cursor",
    anchor: "cursor",
    mcp_path: ".cursor/mcp.json",
    mcp_section: "mcpServers",
    hook_file: "hooks.json",
    hook_file_keys: &["version"],
}];

/// The retired target `name` (`cursor`, `.Cursor`) names, if any.
pub fn retired_target(name: &str) -> Option<&'static RetiredTarget> {
    let name = name
        .trim()
        .trim_end_matches(['/', '\\'])
        .to_ascii_lowercase();
    let dir = if name.starts_with('.') {
        name
    } else {
        format!(".{name}")
    };
    RETIRED_TARGETS.iter().find(|target| target.dir == dir)
}

/// The warning for a harness name at `path` when `name` is a harness Mars retired; naming it
/// has no effect.
pub(crate) fn harness_diagnostic(name: &str, path: Vec<PathSegment>) -> Option<SchemaDiagnostic> {
    let retired = retired_target(name)?;
    let at = path
        .iter()
        .filter_map(|segment| match segment {
            PathSegment::Key(key) => Some(key.as_str()),
            PathSegment::Index(_) => None,
        })
        .collect::<Vec<_>>()
        .join(".");
    Some(SchemaDiagnostic::new(
        "field.retired",
        Severity::Warning,
        path,
        format!(
            "`{at}` names `{}`, a harness Mars no longer supports, so naming it has no effect; remove it: {DOCS}/retired-keys.md#{}",
            name.trim(),
            retired.anchor
        ),
    ))
}

/// The warnings a config file earns for what Mars retired: `settings.managed_root`, and a retired
/// harness named in `settings.default_harness`, `settings.harness_order` or a
/// `[models.<alias>] harness`. The one place the CLI and the WASM reader find them.
pub(crate) fn config_notes(table: &toml::Table) -> Vec<SchemaDiagnostic> {
    let key = |part: &str| PathSegment::Key(part.to_owned());
    let settings = table.get("settings").and_then(toml::Value::as_table);
    let mut diagnostics: Vec<SchemaDiagnostic> = managed_root_note(
        settings
            .and_then(|settings| settings.get("managed_root"))
            .and_then(toml::Value::as_str),
    )
    .into_iter()
    .collect();
    let mut check = |value: Option<&toml::Value>, path: Vec<PathSegment>| {
        if let Some(name) = value.and_then(toml::Value::as_str) {
            diagnostics.extend(harness_diagnostic(name, path));
        }
    };
    check(
        settings.and_then(|settings| settings.get("default_harness")),
        vec![key("settings"), key("default_harness")],
    );
    let order = settings
        .and_then(|settings| settings.get("harness_order"))
        .and_then(toml::Value::as_array);
    for (index, name) in order.into_iter().flatten().enumerate() {
        check(
            Some(name),
            vec![
                key("settings"),
                key("harness_order"),
                PathSegment::Index(index),
            ],
        );
    }
    diagnostics.extend(model_harness_diagnostics(table));
    diagnostics
}

/// The warnings for a retired harness a `[models.<alias>] harness` names: the one place a
/// dependent reads a package's harness choice.
pub fn model_harness_diagnostics(table: &toml::Table) -> Vec<SchemaDiagnostic> {
    table
        .get("models")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
        .filter_map(|(alias, entry)| {
            let name = entry.get("harness")?.as_str()?;
            let path = ["models", alias, "harness"]
                .map(|part| PathSegment::Key(part.to_owned()))
                .to_vec();
            harness_diagnostic(name, path)
        })
        .collect()
}

impl RetiredTarget {
    /// What a project that links this target is told; `unlink` is the command that removes it.
    pub fn message(&self, unlink: &str) -> String {
        format!(
            "`{}` is a target Mars no longer writes: nothing is synced to it. Run `{unlink}` to remove what earlier syncs installed there: {DOCS}/retired-keys.md#{}",
            self.dir, self.anchor
        )
    }
}

/// The `field.retired` note for a file's `managed_root`, which Mars no longer reads. It is built
/// from the file alone, so the CLI and the WASM reader say the same thing.
fn managed_root_note(managed_root: Option<&str>) -> Option<SchemaDiagnostic> {
    let managed_root = managed_root?;
    Some(SchemaDiagnostic::new(
        "field.retired",
        Severity::Warning,
        vec![
            PathSegment::Key("settings".to_owned()),
            PathSegment::Key("managed_root".to_owned()),
        ],
        format!(
            "`settings.managed_root` is a key Mars no longer reads, so it has no effect, and a sync removes what Mars installed in `{managed_root}` unless `targets` names it. To keep `{managed_root}`, write `targets = [\"{managed_root}\"]` under `[settings]` (`mars link {managed_root}` writes it and installs again) and delete `managed_root`. Without `targets`, routing is no longer limited to the harnesses the project named: {DOCS}/retired-keys.md#managed_root"
        ),
    ))
}

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
    ).losing())
}
