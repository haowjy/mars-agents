//! The agy permission strings a launch can apply for a scoped rule the agent file cannot hold.
//!
//! agy's agent file `tools:` lists whole tools, so a rule with a pattern is lost there. `agy
//! --settings` takes `permissions.deny` and `permissions.ask` per launch, which `meridian agy`
//! applies from the launch bundle's `tools.rules[]`. [`translate`] is the only place a rule
//! becomes such a string, and it carries exactly the forms probed against agy (see the
//! work item's `evidence/p11548.md`); anything else stays `unknown` with its reason. Allow is
//! never carried: `--settings` replaces the user's allow list.
//!
//! These strings are not tool names: `project()` and the agent file's `tools:` never see them,
//! because one unknown name there fails the agent at its first prompt.

use mars_schema::mcp::McpSegment;
use mars_schema::tool_names::Capability;
use mars_schema::tool_policy::{ToolAction, ToolRule};
use mars_schema::tool_project::ProjectedRule;

/// What a sync warning says about a carried rule: the agent file does not hold it.
pub(crate) const LAUNCH_ONLY: &str = "enforced only when launched by `meridian agy`";

/// A rule agy applies from `--settings`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Carried {
    /// The permission strings for the rule's action list (`deny` or `ask`).
    pub permissions: Vec<String>,
    /// What agy does not catch.
    pub note: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Translation {
    Carried(Carried),
    NotCarried(&'static str),
}

const BASH_GAPS: &str = "agy matches command words only: `bash -c`, `sh -c`, `$(...)`, `xargs`, absolute paths and options before a subcommand are not caught";
const FILE_GAPS: &str = "agy blocks its file tools only: shell commands still reach the files";
const SKILL_GAPS: &str = "agy stops the skill's SKILL.md from loading by file read: shell commands can still read it, and the skill is still listed";
const MCP_GAPS: &str =
    "not verified: whether a top-level `mcp_<server>_<tool>` call is stopped as well";
const DIR_FORM: &str =
    "the path covers the whole directory tree, where the rule's `*` stops at `/`";
const ROOTS: &str = "a relative path applies under every added directory root";
const PREFIX_FORM: &str = "agy matches the command words as a prefix, so longer commands match too";

/// The rule as agy's permissions spell it, for a deny or an ask.
pub(crate) fn translate(rule: &ToolRule) -> Translation {
    use Translation::NotCarried;
    if rule.action == ToolAction::Allow {
        return NotCarried(
            "an allow is never carried: `--settings` would replace the user's allow list",
        );
    }
    match &rule.capability {
        Capability::Builtin("bash") => rule.pattern.as_deref().map_or(NotCarried(UNSCOPED), bash),
        Capability::Builtin("read") => files(rule.pattern.as_deref(), "read_file"),
        Capability::Builtin("write" | "edit") => files(rule.pattern.as_deref(), "write_file"),
        Capability::Builtin("skill") => skill(rule),
        Capability::Mcp(mcp) => mcp_rule(&mcp.server, &mcp.tool),
        _ => NotCarried("agy has no permission string for this rule"),
    }
}

/// The translation of a rule the projection left out of the agent file (a gap), unless a
/// stricter rule covers it: that one already wins every call.
fn for_gap(rule: &ToolRule, projected: &ProjectedRule) -> Option<Translation> {
    (projected.covered_by.is_none() && projected.gap().is_some()).then(|| translate(rule))
}

pub(crate) fn carried(rule: &ToolRule, projected: &ProjectedRule) -> Option<Carried> {
    match for_gap(rule, projected)? {
        Translation::Carried(carried) => Some(carried),
        Translation::NotCarried(_) => None,
    }
}

/// Why a deny or ask that neither the file nor a launch applies is not carried.
pub(crate) fn not_carried(rule: &ToolRule, projected: &ProjectedRule) -> Option<&'static str> {
    match for_gap(rule, projected)? {
        Translation::NotCarried(reason) if rule.action != ToolAction::Allow => Some(reason),
        _ => None,
    }
}

const UNSCOPED: &str = "a rule with no pattern is carried by the agent file's `tools:`";

fn carry(permissions: Vec<String>, notes: &[&str]) -> Translation {
    Translation::Carried(Carried {
        permissions,
        note: notes.join("; "),
    })
}

fn plain(text: &str, extra: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// `bash(<words>)`, ending in nothing, `:*` or ` *`, where the words are plain.
fn bash(pattern: &str) -> Translation {
    let (words, wildcard) = match pattern
        .strip_suffix(":*")
        .or_else(|| pattern.strip_suffix(" *"))
    {
        Some(words) => (words, true),
        None => (pattern, false),
    };
    let plain_words = !words.is_empty() && words.split(' ').all(|word| plain(word, "_./=@+,-"));
    if !plain_words {
        return Translation::NotCarried(
            "agy matches only plain command words: no quotes, `$`, pipes or inner `*`",
        );
    }
    let permission = vec![format!("command({words})")];
    if wildcard {
        carry(permission, &[BASH_GAPS])
    } else {
        carry(permission, &[PREFIX_FORM, BASH_GAPS])
    }
}

/// `read`, `write` and `edit` of a relative path, or of a directory as `dir/*` or `dir/**`.
fn files(pattern: Option<&str>, tool: &str) -> Translation {
    use Translation::NotCarried;
    let Some(pattern) = pattern else {
        return NotCarried(UNSCOPED);
    };
    let (path, dir_glob) = match pattern
        .strip_suffix("/**")
        .or_else(|| pattern.strip_suffix("/*"))
    {
        Some(dir) => (dir, true),
        None => (pattern, false),
    };
    let relative = !path.starts_with(['/', '~']);
    let clean = path
        .trim_end_matches('/')
        .split('/')
        .all(|segment| !matches!(segment, "" | "." | "..") && !segment.contains(NOT_IN_PATH));
    if !relative || !clean {
        return NotCarried(
            "agy takes only a relative path with no `..` or glob, or a directory as `dir/*`",
        );
    }
    let notes: &[&str] = if dir_glob {
        &[DIR_FORM, ROOTS, FILE_GAPS]
    } else {
        &[ROOTS, FILE_GAPS]
    };
    carry(vec![format!("{tool}({path})")], notes)
}

const NOT_IN_PATH: &[char] = &['*', '?', '[', ']', '{', '}', '(', ')', '\\', '\n', '\t'];

/// A skill is loaded by reading its `SKILL.md` from either skills directory agy reads.
fn skill(rule: &ToolRule) -> Translation {
    let name = rule.pattern.as_deref().filter(|name| plain(name, "_.-"));
    match (rule.action, name) {
        (ToolAction::Deny, Some(name)) if !name.starts_with('.') => carry(
            [".agy", ".agents"]
                .map(|root| format!("read_file({root}/skills/{name})"))
                .to_vec(),
            &[SKILL_GAPS],
        ),
        (ToolAction::Deny, _) => Translation::NotCarried("agy takes only a plain skill name"),
        _ => Translation::NotCarried("agy stops a skill from loading, which only a deny asks for"),
    }
}

/// `mcp(<server>/<tool>)`; a whole server (`mcp(<server>)` or `/*`) is `mcp(<server>/*)`.
fn mcp_rule(server: &McpSegment, tool: &McpSegment) -> Translation {
    let McpSegment::Named(server) = server else {
        return Translation::NotCarried("agy cannot scope an MCP tool across every server");
    };
    let tool = match tool {
        McpSegment::Any => "*",
        McpSegment::Named(tool) if plain(tool, "_.-") => tool,
        McpSegment::Named(_) => {
            return Translation::NotCarried("agy takes only a plain MCP tool name");
        }
    };
    if !plain(server, "_.-") {
        return Translation::NotCarried("agy takes only a plain MCP server name");
    }
    carry(vec![format!("mcp({server}/{tool})")], &[MCP_GAPS])
}
