//! Which tool-rule key covers which: the one place that decides whether one rule's scope
//! holds inside another's.
//!
//! [`relation`] answers only what it can prove for every harness, and returns
//! [`Relation::Unknown`] for the rest. A wrong `Covers` makes a caller drop a rule that
//! should apply, so a case that is not listed below is `Unknown`, never a guess.
//!
//! What it proves, by capability:
//! - `edit` covers `write` and `notebook`; `web` covers `web_search` and `web_fetch`
//!   ([`capability_covers`]). Other pairs of distinct capabilities never overlap.
//! - An unscoped key covers every key of a capability it covers.
//! - `bash` patterns, compared as text: a literal followed by one trailing `*` covers
//!   every pattern that starts with that literal, and covers the bare command (`git *`
//!   covers `git`, not `gitk`). Two such patterns whose literals are not prefixes of each
//!   other are disjoint. This is what Pi's `globMatches`, Codex's `_glob_matches` and
//!   OpenCode's `Wildcard.match` agree on once `?` (a wildcard on OpenCode, a literal on
//!   the others), `\` (OpenCode reads it as `/`) and any other `*` are excluded.
//! - `skill` names with no wildcard character: two different ones are disjoint.
//! - An MCP server key covers its tools; `*` is a whole segment.
//!
//! Path capabilities are always `Unknown` for two different scopes: Pi anchors and
//! normalizes path patterns, Claude gives `/`, `//`, `./` and `~` different roots, and
//! Claude's `*` does not cross `/`, so no text comparison proves anything there.

use crate::mcp::{McpRef, McpSegment};
use crate::tool_names::{Capability, ParsedToolName, is_mcp_wire_token};
use crate::tool_policy::{EffectiveToolPolicy, ToolAction, ToolRule};

/// How the scope of key `a` stands to the scope of key `b`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// The same scope.
    Equal,
    /// Everything `b` matches, `a` matches too.
    Covers,
    /// Everything `a` matches, `b` matches too.
    CoveredBy,
    /// No call matches both.
    Disjoint,
    /// Not provable here: the scopes may overlap, nest or be apart.
    Unknown,
}

/// A rule's key: its capability and scope, without the action.
#[derive(Debug, Clone, Copy)]
pub struct ToolKey<'a> {
    pub capability: &'a Capability,
    pub pattern: Option<&'a str>,
}

impl<'a> From<&'a ToolRule> for ToolKey<'a> {
    fn from(rule: &'a ToolRule) -> Self {
        Self {
            capability: &rule.capability,
            pattern: rule.pattern.as_deref(),
        }
    }
}

impl<'a> From<&'a ParsedToolName> for ToolKey<'a> {
    fn from(name: &'a ParsedToolName) -> Self {
        Self {
            capability: &name.capability,
            pattern: name.pattern.as_deref(),
        }
    }
}

/// Whether capability `a` includes capability `b` as a part of itself.
pub fn capability_covers(a: &str, b: &str) -> bool {
    matches!(
        (a, b),
        ("edit", "write" | "notebook") | ("web", "web_search" | "web_fetch")
    )
}

pub fn relation(a: ToolKey<'_>, b: ToolKey<'_>) -> Relation {
    if a.capability == b.capability && a.pattern == b.pattern {
        return Relation::Equal;
    }
    match (a.capability, b.capability) {
        // `*` names the tools no other rule names, not a set a key can be measured against.
        (Capability::Any, _) | (_, Capability::Any) => Relation::Unknown,
        (Capability::Mcp(x), Capability::Mcp(y)) => mcp_relation(x, y),
        // An `mcp__server__tool` wire token may name the same tool as an `mcp(...)` key.
        (Capability::Mcp(_), Capability::Unknown(name))
        | (Capability::Unknown(name), Capability::Mcp(_)) => {
            if is_mcp_wire_token(name) {
                Relation::Unknown
            } else {
                Relation::Disjoint
            }
        }
        (Capability::Mcp(_), _) | (_, Capability::Mcp(_)) => Relation::Disjoint,
        // Wire tokens may carry `*` (`mcp__github__*`), so two of them can overlap.
        (Capability::Unknown(x), Capability::Unknown(y))
            if x == y || (is_mcp_wire_token(x) && is_mcp_wire_token(y)) =>
        {
            Relation::Unknown
        }
        (Capability::Unknown(_), _) | (_, Capability::Unknown(_)) => Relation::Disjoint,
        (Capability::Builtin(x), Capability::Builtin(y)) => builtin_relation(x, y, a, b),
    }
}

fn builtin_relation(x: &str, y: &str, a: ToolKey<'_>, b: ToolKey<'_>) -> Relation {
    if x == y {
        return match (a.pattern, b.pattern) {
            (None, Some(_)) => Relation::Covers,
            (Some(_), None) => Relation::CoveredBy,
            (Some(p), Some(q)) if x == "bash" => bash_relation(p, q),
            // A skill name with no wildcard character names one skill, and every harness
            // compares it whole.
            (Some(p), Some(q)) if x == "skill" && is_plain(p) && is_plain(q) => Relation::Disjoint,
            _ => Relation::Unknown,
        };
    }
    if capability_covers(x, y) {
        // `edit(src/**)` against `write` overlaps without either holding the other.
        return if a.pattern.is_none() {
            Relation::Covers
        } else {
            Relation::Unknown
        };
    }
    if capability_covers(y, x) {
        return if b.pattern.is_none() {
            Relation::CoveredBy
        } else {
            Relation::Unknown
        };
    }
    Relation::Disjoint
}

/// A `bash` pattern as far as the three matchers agree on its meaning.
enum BashPattern<'a> {
    /// Matches only this command.
    Literal(&'a str),
    /// Matches every command that starts with this text, and the bare command when the
    /// text ends in a space.
    Prefix(&'a str),
    Other,
}

/// No character any of the matchers reads as a wildcard or an escape.
fn is_plain(text: &str) -> bool {
    !text.contains(['*', '?', '\\', '\n', '\r'])
}

fn bash_pattern(pattern: &str) -> BashPattern<'_> {
    let wildcards = pattern.matches('*').count();
    let literal = pattern.strip_suffix('*').unwrap_or(pattern);
    let plain = is_plain(literal);
    match (wildcards, pattern.ends_with('*'), plain) {
        (0, _, true) => BashPattern::Literal(pattern),
        (1, true, true) => BashPattern::Prefix(literal),
        _ => BashPattern::Other,
    }
}

fn prefix_matches(prefix: &str, command: &str) -> bool {
    command.starts_with(prefix) || prefix.strip_suffix(' ') == Some(command)
}

fn bash_relation(p: &str, q: &str) -> Relation {
    match (bash_pattern(p), bash_pattern(q)) {
        (BashPattern::Literal(_), BashPattern::Literal(_)) => Relation::Disjoint,
        (BashPattern::Prefix(prefix), BashPattern::Literal(command)) => {
            if prefix_matches(prefix, command) {
                Relation::Covers
            } else {
                Relation::Disjoint
            }
        }
        (BashPattern::Literal(command), BashPattern::Prefix(prefix)) => {
            if prefix_matches(prefix, command) {
                Relation::CoveredBy
            } else {
                Relation::Disjoint
            }
        }
        (BashPattern::Prefix(x), BashPattern::Prefix(y)) => {
            if y.starts_with(x) {
                Relation::Covers
            } else if x.starts_with(y) {
                Relation::CoveredBy
            } else {
                Relation::Disjoint
            }
        }
        _ => Relation::Unknown,
    }
}

fn mcp_relation(a: &McpRef, b: &McpRef) -> Relation {
    let server = segment_relation(&a.server, &b.server);
    let tool = segment_relation(&a.tool, &b.tool);
    match (server, tool) {
        (Relation::Disjoint, _) | (_, Relation::Disjoint) => Relation::Disjoint,
        (Relation::Equal, other) | (other, Relation::Equal) => other,
        (x, y) if x == y => x,
        // `*/tool` against `github/*`: they meet at `github/tool` and neither holds the other.
        _ => Relation::Unknown,
    }
}

fn segment_relation(a: &McpSegment, b: &McpSegment) -> Relation {
    match (a, b) {
        (McpSegment::Any, McpSegment::Any) => Relation::Equal,
        (McpSegment::Any, McpSegment::Named(_)) => Relation::Covers,
        (McpSegment::Named(_), McpSegment::Any) => Relation::CoveredBy,
        (McpSegment::Named(x), McpSegment::Named(y)) if x == y => Relation::Equal,
        (McpSegment::Named(_), McpSegment::Named(_)) => Relation::Disjoint,
    }
}

/// Each allow or ask in `policy`, by index, with the stricter rule that covers it or has its
/// key: a call it matches also matches that rule, so under strictest-wins it never applies.
/// `*` takes no part, and a pair [`relation`] cannot decide is left out: the enforcer matches
/// the real call.
pub fn covered(policy: &EffectiveToolPolicy) -> Vec<(usize, &ToolRule)> {
    policy
        .rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| rule.action != ToolAction::Deny && !rule.is_wildcard())
        .filter_map(|(index, rule)| {
            policy
                .rules
                .iter()
                .filter(|other| other.action.strictness() > rule.action.strictness())
                .filter(|other| !other.is_wildcard())
                .filter(|other| {
                    matches!(
                        relation(ToolKey::from(*other), ToolKey::from(rule)),
                        Relation::Equal | Relation::Covers
                    )
                })
                .min_by_key(|other| std::cmp::Reverse(other.action.strictness()))
                .map(|other| (index, other))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::Relation::*;
    use super::*;
    use crate::tool_names::parse_mars_tool_name;

    fn rel(a: &str, b: &str) -> Relation {
        let (a, b) = (
            parse_mars_tool_name(a).unwrap(),
            parse_mars_tool_name(b).unwrap(),
        );
        relation((&a).into(), (&b).into())
    }

    fn flipped(relation: Relation) -> Relation {
        match relation {
            Covers => CoveredBy,
            CoveredBy => Covers,
            other => other,
        }
    }

    #[test]
    fn relations() {
        let rows: &[(&str, &str, Relation)] = &[
            // Same key, spelled two ways.
            ("bash", "shell", Equal),
            ("bash(git push:*)", "bash(git push *)", Equal),
            ("read(src/**)", "read(src/**)", Equal),
            ("mcp(github)", "mcp(github/*)", Equal),
            // Capabilities.
            ("edit", "write", Covers),
            ("edit", "notebook", Covers),
            ("web", "web_search", Covers),
            ("web", "web_fetch", Covers),
            ("write", "notebook", Disjoint),
            ("web_search", "web_fetch", Disjoint),
            ("edit", "web", Disjoint),
            ("bash", "read", Disjoint),
            ("grep", "read", Disjoint),
            ("edit", "bash(git *)", Disjoint),
            ("skill(init)", "agent(init)", Disjoint),
            // Unscoped covers scoped; scoped never covers unscoped.
            ("bash", "bash(git *)", Covers),
            ("read", "read(src/**)", Covers),
            ("edit", "write(src/**)", Covers),
            ("web", "web_fetch(domain:example.com)", Covers),
            ("write(src/**)", "edit", CoveredBy),
            ("edit(src/**)", "write", Unknown),
            ("edit(src/**)", "write(src/**)", Unknown),
            ("web_fetch(domain:a.com)", "web", CoveredBy),
            // Bash prefixes.
            ("bash(git *)", "bash(git push *)", Covers),
            ("bash(git *)", "bash(git push)", Covers),
            ("bash(git *)", "bash(git)", Covers),
            ("bash(git *)", "bash(gitk)", Disjoint),
            ("bash(git *)", "bash(gitk *)", Disjoint),
            ("bash(git*)", "bash(gitk *)", Covers),
            ("bash(git*)", "bash(git *)", Covers),
            ("bash(git*)", "bash(git)", Covers),
            ("bash(*)", "bash(git push *)", Covers),
            ("bash(git push *)", "bash(git pull *)", Disjoint),
            ("bash(git push *)", "bash(git status)", Disjoint),
            ("bash(git status)", "bash(git status -s)", Disjoint),
            ("bash(git status)", "bash(git status)", Equal),
            ("bash(git push *)", "bash(git push -f *)", Covers),
            ("bash(git push -f *)", "bash(git push *)", CoveredBy),
            ("bash(pkill -f *)", "bash(pkill -f)", Covers),
            // Bash patterns that are not a literal plus one trailing `*`.
            ("bash(git * main)", "bash(git push main)", Unknown),
            ("bash(git *)", "bash(git * main)", Unknown),
            ("bash(git **)", "bash(git push *)", Unknown),
            ("bash(*git)", "bash(git)", Unknown),
            ("bash(git ?*)", "bash(git a *)", Unknown),
            ("bash(git push?)", "bash(git push1)", Unknown),
            ("bash(git push?)", "bash(git push *)", Unknown),
            ("bash(git *)", "bash(git\\ push)", Unknown),
            ("bash(git\\ *)", "bash(git\\ push *)", Unknown),
            // Paths are never compared as text.
            ("read(src/**)", "read(src/lib/**)", Unknown),
            ("read(./src/*)", "read(src/*)", Unknown),
            ("read(~/x)", "read(/home/x)", Unknown),
            ("read(src/a)", "read(src/b)", Unknown),
            ("write(src/**)", "write(docs/**)", Unknown),
            ("grep(src/**)", "grep(src/a/**)", Unknown),
            ("skill(init)", "skill(review)", Disjoint),
            ("skill(init)", "skill(re*)", Unknown),
            (
                "web_fetch(domain:a.com)",
                "web_fetch(domain:b.com)",
                Unknown,
            ),
            // MCP.
            ("mcp(github)", "mcp(github/create_issue)", Covers),
            ("mcp(github/*)", "mcp(github/create_issue)", Covers),
            ("mcp(*/*)", "mcp(github)", Covers),
            ("mcp(*/*)", "mcp(github/create_issue)", Covers),
            ("mcp(*/create_issue)", "mcp(github/create_issue)", Covers),
            (
                "mcp(github/create_issue)",
                "mcp(github/delete_repo)",
                Disjoint,
            ),
            ("mcp(github)", "mcp(gitlab)", Disjoint),
            ("mcp(github)", "mcp(gitlab/x)", Disjoint),
            ("mcp(*/create_issue)", "mcp(github/*)", Unknown),
            ("mcp(github/Create)", "mcp(github/create)", Disjoint),
            ("mcp(github)", "bash", Disjoint),
            ("mcp(github)", "bash(git *)", Disjoint),
            ("mcp(*/*)", "edit", Disjoint),
            // `*`, unknown names and wire tokens.
            ("*", "bash", Unknown),
            ("*", "mcp(github)", Unknown),
            ("*", "*", Equal),
            ("mystery", "mystery", Equal),
            ("mystery", "mystery(x)", Unknown),
            ("mystery", "other", Disjoint),
            ("mystery", "bash", Disjoint),
            ("mystery", "mcp(github)", Disjoint),
            ("mcp__github__create_issue", "mcp(github)", Unknown),
            (
                "mcp__github__create_issue",
                "mcp__github__create_issue",
                Equal,
            ),
            ("mcp__github__a", "mcp__github__b", Unknown),
        ];
        for (a, b, expected) in rows {
            assert_eq!(rel(a, b), *expected, "{a} against {b}");
            assert_eq!(rel(b, a), flipped(*expected), "{b} against {a}");
        }
    }
}
