//! The class guard for translated definitions. After a foreign or pre-v1 definition is
//! translated, whatever the strict v1 reader would still reject is set aside: moved under
//! `harness-overrides.<harness>` when a harness could read it, dropped when none could, and
//! reported once by key. Only an authored schema-v1 file is ever refused for its fields, because
//! a consumer cannot fix a dependency's file.

use crate::HarnessId;
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::frontmatter::Frontmatter;
use crate::harness_overrides::{Moved, move_keys_under};
use crate::item::DefinitionKind;

/// Keys whose errors cannot be cured by moving them: the file's identity, and the block moves go
/// to.
const KEPT_KEYS: &[&str] = &["name", "description", "harness-overrides"];

/// Where a rejected key goes.
#[derive(Clone, Copy)]
pub(crate) enum Destination<'a> {
    /// Under `harness-overrides.<name>`.
    Overrides(&'a str),
    /// Nowhere: the source has no harness whose keys these are.
    Nowhere,
}

impl Destination<'_> {
    /// The destination for a foreign dialect's keys: the harness that reads that dialect.
    pub(crate) fn for_dialect(dialect: crate::dialect::Dialect) -> Destination<'static> {
        use crate::dialect::Dialect;
        match dialect {
            Dialect::Claude => Destination::Overrides(HarnessId::Claude.as_str()),
            Dialect::Codex => Destination::Overrides(HarnessId::Codex.as_str()),
            Dialect::OpenCode => Destination::Overrides(HarnessId::OpenCode.as_str()),
            Dialect::Cursor => Destination::Overrides(HarnessId::Cursor.as_str()),
            Dialect::AgentSkills | Dialect::MarsNative => Destination::Nowhere,
        }
    }
}

/// The keys of `fm` the strict reader rejects with an error, with what it said, in file order.
fn rejected_keys(kind: DefinitionKind, fm: &Frontmatter) -> Vec<(String, String)> {
    let errors = crate::read::field_diagnostics(kind, fm)
        .into_iter()
        .filter(|diagnostic| {
            diagnostic.is_error()
                && diagnostic.code != "field.required"
                && !diagnostic.code.starts_with("yaml.")
        })
        .filter_map(|diagnostic| match diagnostic.path.first() {
            Some(PathSegment::Key(key)) => Some((key.clone(), diagnostic.message)),
            _ => None,
        })
        .collect::<Vec<_>>();
    fm.keys()
        .into_iter()
        .filter(|key| !KEPT_KEYS.contains(&key.as_str()))
        .filter_map(|key| {
            let (_, why) = errors.iter().find(|(rejected, _)| *rejected == key)?;
            Some((key, why.clone()))
        })
        .collect()
}

fn removed_without_replacement(kind: DefinitionKind, key: &str) -> bool {
    let table = match kind {
        DefinitionKind::Agent => crate::agent::REMOVED_AGENT_FIELDS,
        DefinitionKind::Skill => crate::skill::REMOVED_SKILL_FIELDS,
    };
    table
        .iter()
        .any(|field| field.key == key && field.discarded)
}

/// Set aside every key of `fm` the strict reader rejects, with one warning per key in
/// `changes`. Returns whether the frontmatter changed. When `harness-overrides` or the
/// destination entry is not a mapping the keys stay, and the reader reports them.
pub(crate) fn set_aside_rejected_keys(
    fm: &mut Frontmatter,
    kind: DefinitionKind,
    destination: Destination<'_>,
    changes: &mut Vec<SchemaDiagnostic>,
) -> bool {
    let rejected = rejected_keys(kind, fm);
    if rejected.is_empty() {
        return false;
    }
    let mut warn = |key: &str, message: String| {
        changes.push(SchemaDiagnostic::new(
            "field.set-aside",
            Severity::Warning,
            vec![PathSegment::Key(key.to_owned())],
            message,
        ));
    };
    let (movable, dropped): (Vec<_>, Vec<_>) = rejected.into_iter().partition(|(key, _)| {
        matches!(destination, Destination::Overrides(_)) && !removed_without_replacement(kind, key)
    });
    let mut changed = false;
    for (key, why) in &dropped {
        fm.remove(key);
        changed = true;
        warn(
            key,
            format!("`{key}` is not valid in schema v1 ({why}) and no harness reads it; dropped"),
        );
    }
    if let Destination::Overrides(harness) = destination
        && !movable.is_empty()
    {
        let keys = movable
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        if let Moved::Done { moved, displaced } = move_keys_under(fm, harness, &keys) {
            changed = true;
            for (key, why) in &movable {
                let message = if moved.contains(key) {
                    format!(
                        "`{key}` is not valid in schema v1 ({why}); kept under `harness-overrides.{harness}`"
                    )
                } else if displaced.contains(key) {
                    format!(
                        "`{key}` is not valid in schema v1 ({why}); dropped, `harness-overrides.{harness}.{key}` already sets it"
                    )
                } else {
                    continue;
                };
                warn(key, message);
            }
        }
    }
    changed
}
