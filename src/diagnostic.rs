use std::borrow::Cow;

use serde::Serialize;

use crate::compiler::lossiness::{LossRecord, LossyField};
use crate::lock::{LockFile, RecordRef};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    /// Machine-readable code, e.g. "shadow-collision", "manifest-path-dep". A schema
    /// diagnostic's code passes through unchanged.
    pub code: Cow<'static, str>,
    /// Human-readable message.
    pub message: String,
    /// Optional context (source name, item path, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Diagnostic category for tooling and structured output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<DiagnosticCategory>,
}

impl Diagnostic {
    /// A problem at the level its finder gives it, with no category or context.
    pub(crate) fn new(
        level: DiagnosticLevel,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            level,
            code: code.into(),
            message: message.into(),
            context: None,
            category: None,
        }
    }

    pub(crate) fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(DiagnosticLevel::Warning, code, message)
    }

    /// A schema diagnostic at the level its reader gave it, under the caller's message.
    pub(crate) fn from_schema(
        diagnostic: &mars_schema::diagnostic::SchemaDiagnostic,
        message: String,
    ) -> Self {
        use mars_schema::diagnostic::Severity;
        Self {
            level: match diagnostic.severity {
                Severity::Error => DiagnosticLevel::Error,
                Severity::Warning => DiagnosticLevel::Warning,
                Severity::Info => DiagnosticLevel::Info,
            },
            code: diagnostic.code.clone().into(),
            message,
            context: None,
            category: Some(DiagnosticCategory::Validation),
        }
    }

    /// Whether a command prints this. `detail` is true for `validate`, `check` and `--verbose`:
    /// the commands that were asked what Mars noticed. A command fails on any `Error`
    /// (`fail_on_errors`); that and this are the whole policy.
    pub fn shown(&self, detail: bool) -> bool {
        self.level != DiagnosticLevel::Info || detail
    }

    /// Whether this diagnostic means the sync may have fallen short of the state it wanted
    /// (a failed write, an edit left in place), so `already up to date` is not printed.
    /// Every warning counts unless it is advisory or lossiness.
    pub fn blocks_up_to_date(&self) -> bool {
        self.level == DiagnosticLevel::Warning
            && !matches!(
                self.category,
                Some(DiagnosticCategory::Advisory | DiagnosticCategory::Lossiness)
            )
    }
}

/// Who wrote what a problem is about, and so who can fix it: the project's own file is the
/// user's to change; a dependency's is its author's. When nothing the package declares is lost,
/// a finding in a dependency's file is `Info` to the installer and a `Warning` to its author.
/// A loss is a `Warning` to both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner<'a> {
    Own,
    /// A path dependency: the user can edit it, so a finding is a `Warning` as in their own
    /// files, but its items are recorded in the lock under this source.
    Path(&'a str),
    /// Installed from this source.
    Dependency(&'a str),
}

impl<'a> Owner<'a> {
    /// The owner of an item that came from `source`.
    pub(crate) fn of(source: &'a str) -> Self {
        if source == crate::types::SourceOrigin::LocalPackage.to_string() {
            Self::Own
        } else {
            Self::Dependency(source)
        }
    }

    /// The lock record a loss in this owner's items concerns.
    fn record(self) -> RecordRef {
        RecordRef::Source(match self {
            Self::Own => crate::types::SourceOrigin::LocalPackage.to_string(),
            Self::Path(source) | Self::Dependency(source) => source.to_owned(),
        })
    }
}

/// What the user loses if they ignore a diagnostic. The code that finds a problem states it;
/// nothing downstream decides again whether the problem is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    /// Mars cannot use this input, and the command fails.
    Error,
    /// The user loses something: a restriction not applied, a file not written, an item not
    /// installed.
    Warning,
    /// Nothing is lost, and it may be intended (a key Mars does not read). Shown only when
    /// asked: [`Diagnostic::shown`].
    Info,
}

/// Whether lossiness diagnostics are collected and surfaced to the user.
///
/// Shared by the sync pipeline (`SyncRequest`) and the check/init preview path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LossinessMode {
    /// Suppress lossiness warnings (export, repair, …).
    #[default]
    Hidden,
    /// A sync's default: one line for the project covering the restrictions lost by the items
    /// whose native file this run wrote or changed (`Lossiness::Dropped`). A loss repeated on
    /// every no-change sync is not news.
    Written,
    /// Check, init and plugin builds: the same line, covering every item.
    Surface,
    /// Every lossy field per item and target, written this run or not: quiet, approximate and
    /// launch-time ones too.
    Verbose,
}

impl LossinessMode {
    pub fn surfaces_lossiness(self) -> bool {
        self != LossinessMode::Hidden
    }

    /// Whether every lossy field prints per item, not only the project line for dropped ones.
    pub fn verbose(self) -> bool {
        self == LossinessMode::Verbose
    }
}

/// Broad category for a diagnostic — used in structured output and validation gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticCategory {
    /// Compatibility and version requirement issues.
    Compatibility,
    /// Lossiness during lowering to a target (dropped/approximate fields).
    Lossiness,
    /// Schema validation and structural checks.
    Validation,
    /// A warning that says nothing about whether the sync reached the state it wanted
    /// (an unset env var, a skipped unknown event). It does not stop `already up to date`.
    Advisory,
}

/// One finding that arises once per item but has one cause and one piece of advice, such as the
/// files of a source that collide with unmanaged ones. [`DiagnosticCollector::warn_each`] gathers
/// the items of a group and `drain` prints one line for it.
#[derive(Clone, Copy)]
pub(crate) struct ItemGroup<'a> {
    pub code: &'static str,
    /// What the items share: a source or a target.
    pub subject: &'a str,
    /// The line for `count` items, named by `names`.
    pub describe: fn(subject: &str, count: usize, names: &str) -> String,
}

struct GroupedItems {
    code: &'static str,
    subject: String,
    describe: fn(&str, usize, &str) -> String,
    items: Vec<String>,
}

/// How many item names a grouped line lists before it counts the rest; `--verbose` lists all.
const GROUP_NAME_CAP: usize = 3;

/// Collects diagnostics during pipeline execution.
pub struct DiagnosticCollector {
    diagnostics: Vec<Diagnostic>,
    grouped: Vec<GroupedItems>,
    /// A loss and the lock record it concerns, until `settle` finds the sync changed that
    /// record (`hold`).
    held: Vec<(RecordRef, Diagnostic)>,
    /// No sync will `settle` the held losses, so each prints at once, every run.
    lists_held: bool,
    engine_fallbacks: Vec<crate::resolve::EngineFallback>,
    lossiness_mode: LossinessMode,
    /// The command that re-runs this one with `--verbose`, named in the default lossiness line.
    rerun_verbose: &'static str,
    /// Every lossy field recorded so far; `drain` renders them for `lossiness_mode`.
    lossy_records: Vec<LossRecord>,
    /// Agents whose effort moved to a level the model takes; `drain` renders them.
    effort_moves: Vec<crate::compiler::lossiness::EffortMoveRecord>,
    /// Files that carry each repeated `field.unknown` note, by the warning's own text.
    unknown_field_sites: std::collections::HashMap<String, Vec<String>>,
}

impl DiagnosticCollector {
    pub fn new() -> Self {
        Self::with_lossiness_mode(LossinessMode::default())
    }

    /// Name the command whose `--verbose` shows the detail behind the default lossiness line,
    /// for a caller that is not `mars sync`.
    pub fn rerun_verbose_with(mut self, command: &'static str) -> Self {
        self.rerun_verbose = command;
        self
    }

    /// For a command that writes no lock (`mars validate`), whatever its lossiness mode:
    /// there is no sync to make a held loss news, so every one prints each time.
    pub fn listing_held(mut self) -> Self {
        self.lists_held = true;
        self
    }

    pub fn with_lossiness_mode(lossiness_mode: LossinessMode) -> Self {
        Self {
            diagnostics: Vec::new(),
            grouped: Vec::new(),
            held: Vec::new(),
            lists_held: matches!(
                lossiness_mode,
                LossinessMode::Surface | LossinessMode::Verbose
            ),
            engine_fallbacks: Vec::new(),
            lossiness_mode,
            rerun_verbose: "mars sync --verbose",
            lossy_records: Vec::new(),
            effort_moves: Vec::new(),
            unknown_field_sites: std::collections::HashMap::new(),
        }
    }

    pub(crate) fn schema(
        &mut self,
        diagnostic: &mars_schema::diagnostic::SchemaDiagnostic,
        context: &str,
        owner: Owner<'_>,
    ) {
        use mars_schema::diagnostic::Severity;

        let code = Cow::Owned(diagnostic.code.clone());
        let message = format!("{context}: {}", diagnostic.message);
        if self
            .diagnostics
            .iter()
            .any(|existing| existing.code == code && existing.message == message)
        {
            return;
        }
        if diagnostic.code == "field.unknown" {
            self.note_unknown_field(code, &diagnostic.message, context);
        } else if diagnostic.severity == Severity::Error {
            self.error_with_category(code, message, DiagnosticCategory::Validation);
        } else if diagnostic.lost {
            self.hold_until_written(
                owner,
                Diagnostic {
                    category: Some(DiagnosticCategory::Advisory),
                    ..Diagnostic::new(DiagnosticLevel::Warning, code, message)
                },
            );
        } else if diagnostic.severity == Severity::Info || matches!(owner, Owner::Dependency(_)) {
            self.info(code, message);
        } else {
            self.warn_advisory(code, message);
        }
    }

    /// One note per unknown field name, listing every file that carries it: the first file
    /// is reported as an ordinary note, and each further one extends the same note.
    fn note_unknown_field(&mut self, code: Cow<'static, str>, text: &str, context: &str) {
        let sites = self.unknown_field_sites.entry(text.to_owned()).or_default();
        if sites.iter().any(|site| site == context) {
            return;
        }
        sites.push(context.to_owned());
        if sites.len() == 1 {
            self.info(code.clone(), format!("{context}: {text}"));
            return;
        }
        let message = format!("{text} (in {} files: {})", sites.len(), sites.join(", "));
        let first = format!("{}: {text}", sites[0]);
        if let Some(existing) = self.diagnostics.iter_mut().find(|existing| {
            existing.code == code
                && (existing.message == first || existing.message.starts_with(text))
        }) {
            existing.message = message;
        }
    }

    pub(crate) fn record_engine_fallback(&mut self, fallback: crate::resolve::EngineFallback) {
        self.engine_fallbacks.push(fallback);
    }

    pub(crate) fn take_engine_fallbacks(&mut self) -> Vec<crate::resolve::EngineFallback> {
        std::mem::take(&mut self.engine_fallbacks)
    }

    /// Buffer one lowered item's lossy fields; `drain` renders them by mode. `concerns` are the
    /// lock records of the files the item was lowered into: the loss is news when the sync
    /// changes one of them (`settle`). A caller with no lock to compare, a preview, passes none
    /// and the loss is news at once.
    pub(crate) fn record_lossy_fields(
        &mut self,
        item_kind: &'static str,
        item_name: &str,
        fields: &[LossyField],
        concerns: &[RecordRef],
    ) {
        if !self.lossiness_mode.surfaces_lossiness() {
            return;
        }
        self.lossy_records
            .extend(fields.iter().map(|field| LossRecord {
                item_kind,
                item_name: item_name.to_string(),
                field: field.clone(),
                written: concerns.is_empty(),
                concerns: concerns.to_vec(),
            }));
    }

    /// Buffer an Agent's effort move for `harness`; `drain` renders it by mode.
    pub(crate) fn record_effort_move(
        &mut self,
        agent: &str,
        harness: &str,
        warning: &crate::models::reasoning::EffortWarning,
        concerns: &[RecordRef],
    ) {
        if !self.lossiness_mode.surfaces_lossiness() {
            return;
        }
        self.effort_moves
            .push(crate::compiler::lossiness::EffortMoveRecord {
                agent: agent.to_string(),
                harness: harness.to_string(),
                message: warning.message.clone(),
                not_sent: warning.not_sent,
                written: concerns.is_empty(),
                concerns: concerns.to_vec(),
            });
    }

    pub fn error(&mut self, code: impl Into<Cow<'static, str>>, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Error,
            code: code.into(),
            message: message.into(),
            context: None,
            category: None,
        });
    }

    pub fn error_with_category(
        &mut self,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
        category: DiagnosticCategory,
    ) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Error,
            code: code.into(),
            message: message.into(),
            context: None,
            category: Some(category),
        });
    }

    pub fn warn(&mut self, code: impl Into<Cow<'static, str>>, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            code: code.into(),
            message: message.into(),
            context: None,
            category: None,
        });
    }

    /// A warning that does not imply the sync fell short of the state it wanted. Every other
    /// warning does: `already up to date` is withheld while one is present, so an unmarked
    /// site costs a reassuring line, never a false one.
    pub fn warn_advisory(
        &mut self,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
    ) {
        self.warn_with_category(code, message, DiagnosticCategory::Advisory);
    }

    /// A warning for `item`, joined with the others of its `group` into one line at `drain`.
    pub(crate) fn warn_each(&mut self, group: ItemGroup<'_>, item: impl Into<String>) {
        let item = item.into();
        let existing = self
            .grouped
            .iter_mut()
            .find(|found| found.code == group.code && found.subject == group.subject);
        match existing {
            Some(found) if found.items.contains(&item) => {}
            Some(found) => found.items.push(item),
            None => self.grouped.push(GroupedItems {
                code: group.code,
                subject: group.subject.to_owned(),
                describe: group.describe,
                items: vec![item],
            }),
        }
    }

    pub(crate) fn has_item(&self, code: &str, subject: &str, item: &str) -> bool {
        self.grouped.iter().any(|group| {
            group.code == code && group.subject == subject && group.items.iter().any(|i| i == item)
        })
    }

    /// Keep a loss until `settle`: it is news only when the sync changed the lock record it
    /// concerns, and one on a record that stays as it was is not. A command that lists every
    /// loss (`--verbose`, `mars check`, `mars validate`) shows it at once.
    pub(crate) fn hold(&mut self, record: RecordRef, diagnostic: Diagnostic) {
        if self.lists_held {
            self.diagnostics.push(diagnostic)
        } else {
            self.held.push((record, diagnostic))
        }
    }

    /// [`Self::hold`] for a loss as a warning: the user gets less than the item declares.
    pub(crate) fn hold_warning(
        &mut self,
        record: RecordRef,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
    ) {
        self.hold(
            record,
            Diagnostic {
                category: Some(DiagnosticCategory::Advisory),
                ..Diagnostic::new(DiagnosticLevel::Warning, code, message)
            },
        );
    }

    /// A loss in what `owner` installed, news when the sync changes one of its items.
    pub(crate) fn hold_until_written(&mut self, owner: Owner<'_>, diagnostic: Diagnostic) {
        self.hold(owner.record(), diagnostic);
    }

    /// The one rule for when a held loss is news: the sync wrote `new` over `old`, and the
    /// record the loss concerns is new or different in `new`. What does not pass is dropped by
    /// `drain`, so a sync that fails before this reports nothing held.
    pub(crate) fn settle(&mut self, old: &LockFile, new: &LockFile) {
        let (news, held): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|(record, _)| record.is_news(old, new));
        self.held = held;
        self.diagnostics
            .extend(news.into_iter().map(|(_, diagnostic)| diagnostic));
        let changed = |concerns: &[RecordRef]| concerns.iter().any(|c| c.is_news(old, new));
        for record in &mut self.lossy_records {
            record.written |= changed(&record.concerns);
        }
        for moved in &mut self.effort_moves {
            moved.written |= changed(&moved.concerns);
        }
    }

    pub(crate) fn warn_once(
        &mut self,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
    ) {
        let code = code.into();
        let message = message.into();
        if !self.has(&code, &message) {
            self.warn(code, message);
        }
    }

    pub(crate) fn has_code(&self, code: &str) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code)
    }

    fn has(&self, code: &str, message: &str) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code && diagnostic.message == message)
    }

    pub fn warn_with_category(
        &mut self,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
        category: DiagnosticCategory,
    ) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            code: code.into(),
            message: message.into(),
            context: None,
            category: Some(category),
        });
    }

    pub fn info(&mut self, code: impl Into<Cow<'static, str>>, message: impl Into<String>) {
        let message = message.into();
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Info,
            code: code.into(),
            message,
            context: None,
            category: None,
        });
    }

    pub(crate) fn info_once(&mut self, code: &'static str, message: impl Into<String>) {
        let message = message.into();
        if self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == code && diagnostic.message == message)
        {
            return;
        }
        self.info(code, message);
    }

    pub fn warn_with_context(
        &mut self,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
        context: impl Into<String>,
    ) {
        self.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Warning,
            code: code.into(),
            message: message.into(),
            context: Some(context.into()),
            category: None,
        });
    }

    pub(crate) fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn extend(&mut self, diagnostics: Vec<Diagnostic>) {
        self.diagnostics.extend(diagnostics);
    }

    pub(crate) fn fail_on_errors(&self) -> Result<(), crate::error::MarsError> {
        let errors = self
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
            .map(|diagnostic| format!("[{}] {}", diagnostic.code, diagnostic.message))
            .collect::<Vec<_>>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(crate::error::ConfigError::Invalid {
                message: errors.join("\n"),
            }
            .into())
        }
    }

    pub fn drain(&mut self) -> Vec<Diagnostic> {
        let records = std::mem::take(&mut self.lossy_records);
        self.diagnostics.extend(crate::compiler::lossiness::render(
            &records,
            self.lossiness_mode,
            self.rerun_verbose,
        ));
        self.held.clear();
        for group in std::mem::take(&mut self.grouped) {
            let shown = if self.lossiness_mode.verbose() {
                group.items.len()
            } else {
                GROUP_NAME_CAP
            };
            let mut names = group
                .items
                .iter()
                .take(shown)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            if group.items.len() > shown {
                names.push_str(&format!(" and {} more", group.items.len() - shown));
            }
            self.diagnostics.push(Diagnostic::warning(
                group.code,
                (group.describe)(&group.subject, group.items.len(), &names),
            ));
        }
        let moves = std::mem::take(&mut self.effort_moves);
        crate::compiler::lossiness::render_effort_moves(
            &moves,
            self.lossiness_mode,
            self.rerun_verbose,
            &mut self.diagnostics,
        );
        std::mem::take(&mut self.diagnostics)
    }
}

impl Default for DiagnosticCollector {
    fn default() -> Self {
        Self::with_lossiness_mode(LossinessMode::default())
    }
}

impl Diagnostic {
    /// The line a printer shows. `tagged` adds the `[code]` tag and the `(context)` suffix, which
    /// say which rule fired and which setting or file it concerns.
    pub fn line(&self, tagged: bool) -> String {
        let prefix = match self.level {
            DiagnosticLevel::Error => "error",
            DiagnosticLevel::Warning => "warning",
            DiagnosticLevel::Info => "info",
        };
        match (&self.context, tagged) {
            (Some(context), true) => {
                format!("{prefix}[{}]: {} ({context})", self.code, self.message)
            }
            (None, true) => format!("{prefix}[{}]: {}", self.code, self.message),
            (_, false) => format!("{prefix}: {}", self.message),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.line(false))
    }
}

/// Compatibility preflight: whether the running Mars meets a project's `min_mars_version`.
///
/// `None` when it does, or when the project states no requirement. A requirement that does not
/// parse is an error: the loader refuses such a file, so this is reached only by one it has not
/// read.
pub fn compatibility_preflight(min_required: Option<&str>) -> Result<Option<Diagnostic>, String> {
    let Some(min) = min_required else {
        return Ok(None);
    };
    let requirement = mars_schema::config::parse_min_mars_version(min)?;
    let running =
        semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is semver");
    if mars_schema::package::requirement_matches(&requirement, &running) {
        return Ok(None);
    }
    Ok(Some(Diagnostic {
        level: DiagnosticLevel::Error,
        code: "compat-version".into(),
        message: format!(
            "this project requires mars {requirement} but the installed binary is {running}; \
             upgrade with: cargo install mars-agents"
        ),
        context: None,
        category: Some(DiagnosticCategory::Compatibility),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meridian_only(field: &str) -> LossyField {
        LossyField {
            field: field.into(),
            target: "Claude".into(),
            classification: crate::compiler::lossiness::Lossiness::MeridianOnly,
        }
    }

    #[test]
    fn collector_verbose_surfaces_meridian_only_detail() {
        let mut coll = DiagnosticCollector::with_lossiness_mode(LossinessMode::Verbose);
        coll.record_lossy_fields("agent", "coder", &[meridian_only("approval")], &[]);
        let diags = coll.drain();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "agent-field-meridian-only");
    }

    #[test]
    fn collector_surface_is_silent_about_meridian_only_fields() {
        let mut coll = DiagnosticCollector::with_lossiness_mode(LossinessMode::Surface);
        coll.record_lossy_fields("agent", "coder", &[meridian_only("approval")], &[]);
        let diags = coll.drain();
        assert!(diags.is_empty());
    }

    #[test]
    fn collector_error_with_category() {
        let mut coll = DiagnosticCollector::new();
        coll.error_with_category(
            "compat-version",
            "too old",
            DiagnosticCategory::Compatibility,
        );
        let diags = coll.drain();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, DiagnosticLevel::Error);
        assert_eq!(diags[0].category, Some(DiagnosticCategory::Compatibility));
    }

    #[test]
    fn display_shows_error_prefix() {
        let d = Diagnostic {
            level: DiagnosticLevel::Error,
            code: "test".into(),
            message: "something broke".to_string(),
            context: None,
            category: None,
        };
        assert_eq!(d.to_string(), "error: something broke");
    }

    #[test]
    fn display_shows_warning_prefix() {
        let d = Diagnostic {
            level: DiagnosticLevel::Warning,
            code: "test".into(),
            message: "heads up".to_string(),
            context: None,
            category: None,
        };
        assert_eq!(d.to_string(), "warning: heads up");
    }
}
