//! Pure legacy Mars definition and manifest upgrades to schema v1.

use serde_yaml::{Mapping, Value};
use toml_edit::{DocumentMut, Item};

use crate::claude;
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::frontmatter::{Frontmatter, FrontmatterError};
use crate::harness_overrides::{Moved, move_keys_under};
use crate::item::DefinitionKind;
use crate::{
    agent::{REMOVED_AGENT_FIELDS, REMOVED_AGENT_VALUES},
    skill,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpgradeResult {
    pub text: String,
    /// What the upgrade changed, one note per key it dropped or moved.
    pub diagnostics: Vec<SchemaDiagnostic>,
    /// A changed definition whose text was re-rendered rather than edited minimally: the whole
    /// frontmatter, or a `harness-overrides` block that could not be grown in place.
    pub fully_rewritten: bool,
    /// Authored key paths whose values or presence changed.
    pub edited_paths: Vec<Vec<PathSegment>>,
}

impl UpgradeResult {
    pub(crate) fn changed_from(&self, input: &str) -> bool {
        self.text != input
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UpgradeError {
    #[error(transparent)]
    Frontmatter(#[from] FrontmatterError),
    #[error("failed to parse mars.toml: {0}")]
    Manifest(#[from] toml_edit::TomlError),
    #[error("mars.toml at a package root must contain [package]")]
    MissingPackage,
}

pub fn upgrade_definition(kind: DefinitionKind, text: &str) -> Result<UpgradeResult, UpgradeError> {
    rewrite(text, |frontmatter, diagnostics| match kind {
        DefinitionKind::Agent => upgrade_agent(frontmatter, diagnostics),
        DefinitionKind::Skill => upgrade_skill(frontmatter, diagnostics),
    })
}

/// Apply only what schema 1 removes with a mechanical replacement: the Agent keys that moved
/// under `harness-overrides`.
///
/// `mars migrate` runs this on a schema 1 file it is about to write back, so a file that
/// carries a key the reader rejects is repaired instead of failing the run. Every reading
/// caller leaves it off, because an authored schema 1 file is read exactly as written.
pub fn relocate_removed_fields(
    kind: DefinitionKind,
    text: &str,
) -> Result<UpgradeResult, UpgradeError> {
    rewrite(text, |frontmatter, notes| {
        rename_consumer_namespaces(frontmatter, notes);
        if kind == DefinitionKind::Agent {
            move_agent_fields(frontmatter, notes);
        }
    })
}

/// `text` with `apply`'s changes to its frontmatter, edited as minimally as the source shape
/// allows.
fn rewrite(
    text: &str,
    apply: impl FnOnce(&mut Frontmatter, &mut Vec<SchemaDiagnostic>),
) -> Result<UpgradeResult, UpgradeError> {
    let original = Frontmatter::parse(text)?;
    let mut frontmatter = original.clone();
    let mut diagnostics = Vec::new();
    apply(&mut frontmatter, &mut diagnostics);
    if frontmatter.mapping() == original.mapping() {
        return Ok(UpgradeResult {
            text: text.to_string(),
            diagnostics,
            fully_rewritten: false,
            edited_paths: Vec::new(),
        });
    }

    let edited_paths = changed_mapping_paths(original.mapping(), frontmatter.mapping());

    // The fallback re-renders the frontmatter only. The body is authored prompt text that may be
    // byte-sensitive (embedded examples, mixed line endings), so it is copied from the input
    // rather than from the render, which ends every line in LF.
    let fallback = {
        let mut rewritten = with_line_ending_of(text, frontmatter.render_frontmatter());
        rewritten.push_str(original.body());
        rewritten
    };
    let minimally_edited = edit_top_level_blocks(text, original.mapping(), frontmatter.mapping())
        .and_then(|edited| {
            Frontmatter::parse(&edited.text)
                .ok()
                .filter(|parsed| parsed.mapping() == frontmatter.mapping())
                .map(|_| edited)
        });
    Ok(UpgradeResult {
        fully_rewritten: minimally_edited
            .as_ref()
            .is_none_or(|edited| edited.re_rendered),
        text: minimally_edited.map_or(fallback, |edited| edited.text),
        diagnostics,
        edited_paths,
    })
}

/// Convert a `mars.toml` to schema v1.
///
/// `prepare` and `mars migrate` run this on a pre-v1 package. It never touches `requires-mars`:
/// the requirement gates whether a package may be read at all, so only `mars migrate`,
/// which a human runs on the package they own, adds one (see [`ensure_requires_mars_floor`]).
pub fn upgrade_manifest(text: &str) -> Result<UpgradeResult, UpgradeError> {
    let mut document = text.parse::<DocumentMut>()?;
    let package = document
        .get_mut("package")
        .and_then(Item::as_table_mut)
        .ok_or(UpgradeError::MissingPackage)?;
    if package.get("schema").and_then(Item::as_integer) == Some(1) {
        return Ok(UpgradeResult {
            text: text.to_string(),
            diagnostics: Vec::new(),
            fully_rewritten: false,
            edited_paths: Vec::new(),
        });
    }
    let mut diagnostics = Vec::new();
    let mut edited_paths = vec![package_path("schema")];
    package.insert("schema", toml_edit::value(1));
    if package.remove("visibility").is_some() {
        diagnostics.push(removed(&["package", "visibility"], "visibility"));
        edited_paths.push(package_path("visibility"));
    }
    Ok(UpgradeResult {
        text: with_line_ending_of(text, document.to_string()),
        diagnostics,
        fully_rewritten: false,
        edited_paths,
    })
}

/// Adding the `requires-mars` floor to a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloorOutcome {
    /// The manifest with the key added, or the input when it was not.
    pub text: String,
    /// `mars check`'s advice, when the requirement exists and admits a Mars that reads the
    /// package as pre-v1, or when the manifest has a shape migrate cannot edit minimally.
    pub warning: Option<String>,
}

/// Add `requires-mars = ">=0.20.0"` to a manifest that has no requirement.
///
/// `mars migrate` runs this on the manifest it is about to write, so a schema v1 package needs
/// no hand edit. An existing requirement is never rewritten: when it admits a Mars older than
/// the first release that reads schema 1, [`FloorOutcome::warning`] is `mars check`'s advice and
/// the value keeps its bytes and comment. A manifest migrate cannot edit minimally (an inline
/// `[package]` table) comes back unchanged with the same advice, never an error, so an
/// already-v1 package migrates exactly as it did before this key existed.
pub fn ensure_requires_mars_floor(text: &str) -> Result<FloorOutcome, UpgradeError> {
    let mut document = text.parse::<DocumentMut>()?;
    let package = document
        .get_mut("package")
        .ok_or(UpgradeError::MissingPackage)?;
    let requirement = package
        .as_table_like()
        .and_then(|table| table.get("requires-mars"))
        .and_then(Item::as_str)
        .map(str::to_owned);
    if requirement.is_none()
        && let Some(table) = package.as_table_mut()
    {
        table.insert(
            "requires-mars",
            toml_edit::value(format!(">={}", crate::package::SCHEMA_1_MARS_FLOOR)),
        );
        return Ok(FloorOutcome {
            text: with_line_ending_of(text, document.to_string()),
            warning: None,
        });
    }
    Ok(FloorOutcome {
        text: text.to_string(),
        warning: crate::package::schema_1_requires_mars_warning(requirement.as_deref()),
    })
}

fn package_path(key: &str) -> Vec<PathSegment> {
    vec![
        PathSegment::Key("package".to_string()),
        PathSegment::Key(key.to_string()),
    ]
}

/// Whether the file's first line ends in `\r\n`.
fn is_crlf(text: &str) -> bool {
    text.find('\n')
        .is_some_and(|end| text.as_bytes()[..end].ends_with(b"\r"))
}

/// `written` uses `\n` where the upgrade added lines; a CRLF file stays CRLF.
fn with_line_ending_of(original: &str, written: String) -> String {
    if !is_crlf(original) {
        return written;
    }
    let mut out = String::with_capacity(written.len() + written.len() / 16);
    let mut previous = '\0';
    for character in written.chars() {
        if character == '\n' && previous != '\r' {
            out.push('\r');
        }
        out.push(character);
        previous = character;
    }
    out
}

fn changed_mapping_paths(before: &Mapping, after: &Mapping) -> Vec<Vec<PathSegment>> {
    let mut keys = before
        .keys()
        .chain(after.keys())
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter()
        .filter(|key| {
            let key = Value::String((*key).to_string());
            before.get(&key) != after.get(&key)
        })
        .map(|key| vec![PathSegment::Key(key.to_string())])
        .collect()
}

#[derive(Debug)]
struct SourceBlock<'a> {
    key: &'a str,
    text: &'a str,
    key_end: usize,
    start: usize,
}

/// A frontmatter edited in place, and whether a block had to be re-rendered from its parsed
/// value, which drops the comments and flow style written inside it.
struct Edited {
    text: String,
    re_rendered: bool,
}

/// Rewrite only changed top-level blocks. Complex top-level YAML deliberately returns `None` so
/// the caller has one explicit, verified full-serialization fallback.
fn edit_top_level_blocks(text: &str, before: &Mapping, after: &Mapping) -> Option<Edited> {
    let (prefix, yaml, suffix) = frontmatter_source(text)?;
    let blocks = source_blocks(yaml)?;
    let before_keys = before
        .keys()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?;
    if blocks.iter().map(|block| block.key).collect::<Vec<_>>() != before_keys {
        return None;
    }

    let render = |key: &str, value: &Value| with_line_ending_of(text, render_block(key, value));
    let mut output = String::with_capacity(text.len());
    let mut re_rendered = false;
    output.push_str(prefix);
    let first_key_offset = blocks.first().map_or(yaml.len(), |block| block.start);
    output.push_str(&yaml[..first_key_offset]);

    let mut emitted = Vec::new();
    for block in blocks {
        let before_value = before.get(Value::String(block.key.to_string()))?;
        if let Some(rename) = renamed_key(block.key, before, after) {
            output.push_str(rename);
            output.push_str(&block.text[block.key_end..]);
            emitted.push(rename);
        } else if let Some(after_value) = after.get(Value::String(block.key.to_string())) {
            if before_value == after_value {
                output.push_str(block.text);
            } else if let Some(grown) = grown_in_place(&block, before_value, after_value) {
                output.push_str(&grown);
            } else {
                // Re-rendering a block loses what the author wrote inside it, which only a
                // `harness-overrides` block can avoid by growing in place, so a file that fell back
                // here is reported.
                re_rendered |= block.key == "harness-overrides";
                output.push_str(&render(block.key, after_value));
            }
            emitted.push(block.key);
        }
    }

    for (key, value) in after {
        let key = key.as_str()?;
        if emitted.contains(&key) {
            continue;
        }
        if !output.ends_with('\n') {
            output.push_str(&with_line_ending_of(text, "\n".to_owned()));
        }
        output.push_str(&render(key, value));
    }
    output.push_str(suffix);
    Some(Edited {
        text: output,
        re_rendered,
    })
}

/// `block` with the entries `after` adds, appended as text, so the comments and flow style the
/// author wrote in the entries already there survive. `None` when the block cannot take text that
/// simply: it is written in flow style, or an entry it has changed.
fn grown_in_place(block: &SourceBlock<'_>, before: &Value, after: &Value) -> Option<String> {
    let (Value::Mapping(before), Value::Mapping(after)) = (before, after) else {
        return None;
    };
    if before
        .iter()
        .any(|(key, value)| after.get(key) != Some(value))
    {
        return None;
    }
    let added = after
        .iter()
        .filter(|(key, _)| before.get(key).is_none())
        .collect::<Vec<_>>();
    let indent = (!added.is_empty()).then(|| child_indent(block)).flatten()?;
    // The block's text runs up to the next top-level key, so it ends with whatever blank lines and
    // comments were written above that key; the new entries go before them.
    let (body, tail) = split_trailing_run(block.text);
    let mut grown = body.to_string();
    if !grown.ends_with('\n') {
        grown.push('\n');
    }
    for (key, value) in added {
        let rendered = render_block(key.as_str()?, value);
        // The block's own indentation is the author's; serde's two spaces a level become theirs.
        let indented = rendered
            .lines()
            .map(|line| {
                let depth = line.len() - line.trim_start().len();
                format!("{}{}", indent.repeat(depth / 2 + 1), line.trim_start())
            })
            .collect::<Vec<_>>()
            .join("\n");
        grown.push_str(&indented);
        grown.push('\n');
    }
    grown.push_str(tail);
    Some(with_line_ending_of(block.text, grown))
}

/// `text` split where an appended entry belongs: before the trailing run of blank lines and
/// comments at the document's left edge, which belong to the key below the block.
fn split_trailing_run(text: &str) -> (&str, &str) {
    let mut split = text.len();
    for line in text.split_inclusive('\n').rev() {
        let content = line.trim_end_matches(['\r', '\n']);
        if !content.trim().is_empty() && !content.starts_with('#') {
            break;
        }
        split -= line.len();
    }
    text.split_at(split)
}

/// The indentation of `block`'s own children in its source text, and `None` when it has none to
/// line up with: a flow mapping, or a value that is not a mapping at all.
fn child_indent<'a>(block: &'a SourceBlock<'a>) -> Option<&'a str> {
    let first = block.text.split_inclusive('\n').next()?;
    let rest = first[block.key_end + 1..].trim_end_matches(['\r', '\n']);
    if !(rest.trim().is_empty() || rest.trim_start().starts_with('#')) {
        return None;
    }
    for line in block.text.split_inclusive('\n').skip(1) {
        let content = line.trim_end_matches(['\r', '\n']);
        if content.trim().is_empty() || content.trim_start().starts_with('#') {
            continue;
        }
        let indent = content.len() - content.trim_start().len();
        return (indent > 0).then(|| &content[..indent]);
    }
    None
}

fn renamed_key<'a>(old: &str, before: &Mapping, after: &'a Mapping) -> Option<&'a str> {
    let new = match old {
        "model_invocable" => "model-invocable",
        "user_invocable" => "user-invocable",
        "disallowed_tools" => "disallowed-tools",
        _ => return None,
    };
    if before.contains_key(Value::String(new.to_string())) {
        return None;
    }
    let old_value = before.get(Value::String(old.to_string()))?;
    (after.get(Value::String(new.to_string())) == Some(old_value)).then_some(new)
}

fn render_block(key: &str, value: &Value) -> String {
    let mut mapping = Mapping::new();
    mapping.insert(Value::String(key.to_string()), value.clone());
    let yaml = serde_yaml::to_string(&mapping).expect("serializing an upgraded YAML block");
    yaml.strip_prefix("---\n").unwrap_or(&yaml).to_string()
}

fn frontmatter_source(text: &str) -> Option<(&str, &str, &str)> {
    let first_end = text.find('\n').map_or(text.len(), |offset| offset + 1);
    if text[..first_end]
        .strip_prefix('\u{feff}')
        .unwrap_or(&text[..first_end])
        .trim_end()
        != "---"
    {
        return None;
    }
    let mut offset = first_end;
    for line in text[first_end..].split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((
                &text[..first_end],
                &text[first_end..offset],
                &text[offset..],
            ));
        }
        offset += line.len();
    }
    None
}

fn source_blocks(yaml: &str) -> Option<Vec<SourceBlock<'_>>> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in yaml.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        if !content.is_empty()
            && !content.starts_with(char::is_whitespace)
            && !content.starts_with('#')
        {
            let colon = content.find(':')?;
            let key = &content[..colon];
            if key.is_empty()
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return None;
            }
            starts.push((offset, key, colon));
        }
        offset += line.len();
    }
    let mut blocks = Vec::with_capacity(starts.len());
    for (index, (start, key, key_end)) in starts.iter().copied().enumerate() {
        let end = starts
            .get(index + 1)
            .map_or(yaml.len(), |(start, _, _)| *start);
        blocks.push(SourceBlock {
            key,
            text: &yaml[start..end],
            key_end,
            start,
        });
    }
    Some(blocks)
}

/// The rules for what only pre-v1 Mars wrote, key by key: the invocability renames, the
/// discarded Agent fields, the removed values, a scalar `permission` and the removed model-policy
/// override keys. No foreign dialect defines any of them, so every translation applies them
/// before its own rules (`lift_frontmatter`), and the legacy upgrade is the same rules plus its
/// own. A tree is never classified as pre-v1 Mars by finding one: its other keys are read in the
/// dialect they belong to.
pub(crate) fn lift_mars_only_keys(
    fm: &mut Frontmatter,
    kind: DefinitionKind,
    notes: &mut Vec<SchemaDiagnostic>,
) -> bool {
    let before = fm.mapping().clone();
    rename_field(fm, "model_invocable", "model-invocable");
    rename_field(fm, "user_invocable", "user-invocable");
    if kind == DefinitionKind::Agent {
        merge_host_set_spellings(fm, "network_allow", "network-allow");
        merge_host_set_spellings(fm, "network_deny", "network-deny");
        for field in REMOVED_AGENT_FIELDS.iter().filter(|field| field.discarded) {
            if fm.remove(field.key).is_some() {
                notes.push(removed(&[field.key], field.key));
            }
        }
        for removed in REMOVED_AGENT_VALUES {
            replace_scalar(fm, removed.key, removed.value, removed.replacement);
        }
        move_agent_fields(fm, notes);
        strip_model_policy_overrides(fm, notes);
    }
    fm.mapping() != &before
}

fn upgrade_agent(fm: &mut Frontmatter, diagnostics: &mut Vec<SchemaDiagnostic>) {
    upgrade_tools(fm);
    lift_mars_only_keys(fm, DefinitionKind::Agent, diagnostics);
    // A pre-v1 file passed unknown keys to every harness; Claude's own keep reaching Claude.
    claude::translate::move_to_claude_overrides(fm, DefinitionKind::Agent, diagnostics);
    set_aside_rejected_keys(fm, DefinitionKind::Agent, diagnostics);
}

fn upgrade_skill(fm: &mut Frontmatter, diagnostics: &mut Vec<SchemaDiagnostic>) {
    lift_mars_only_keys(fm, DefinitionKind::Skill, diagnostics);

    if let Some(skill_type) = fm.get("type").cloned() {
        let metadata = fm
            .get("metadata")
            .cloned()
            .unwrap_or_else(|| Value::Mapping(Mapping::new()));
        if let Value::Mapping(mut metadata) = metadata {
            let key = Value::String("type".to_string());
            if !metadata.contains_key(&key) {
                metadata.insert(key, skill_type);
                fm.insert("metadata", Value::Mapping(metadata));
                fm.remove("type");
            }
        }
    }

    if let Some(when) = fm
        .get("when_to_use")
        .and_then(Value::as_str)
        .map(str::to_owned)
    {
        let description = match fm.get("description") {
            Some(Value::String(description)) => Some(format!("{description}\n\n{when}")),
            None => Some(when),
            Some(_) => None,
        };
        if let Some(description) = description {
            fm.insert("description", Value::String(description));
            fm.remove("when_to_use");
        }
    }

    for key in skill::tool_keys() {
        if fm.remove(key).is_some() {
            diagnostics.push(SchemaDiagnostic::new(
                "skill.tools-dropped",
                Severity::Warning,
                vec![PathSegment::Key(key.to_string())],
                format!("removed skill key `{key}`; skill tool rules have no v1 meaning"),
            ));
        }
    }

    claude::translate::move_to_claude_overrides(fm, DefinitionKind::Skill, diagnostics);
    set_aside_rejected_keys(fm, DefinitionKind::Skill, diagnostics);
}

/// What the rules above could not make schema v1 is dropped, with a warning naming it, so the
/// upgrade never hands the strict reader a value it rejects. It is not kept under
/// `harness-overrides.meridian-flow`: Meridian Flow defines one key, `permission`, and `lift_mars_only_keys` writes
/// it there when it is `read` or `edit`. Anything else in that namespace would not be Meridian Flow's.
fn set_aside_rejected_keys(
    fm: &mut Frontmatter,
    kind: DefinitionKind,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) {
    crate::set_aside::set_aside_rejected_keys(
        fm,
        kind,
        crate::set_aside::Destination::Nowhere,
        diagnostics,
    );
}

fn upgrade_tools(fm: &mut Frontmatter) {
    rename_field(fm, "disallowed_tools", "disallowed-tools");
    if let Some(Value::Sequence(entries)) = fm.get("tools").cloned()
        && entries.iter().all(|entry| entry.as_str().is_some())
    {
        let policy = entries
            .into_iter()
            .map(|entry| (entry, Value::String("allow".to_string())))
            .collect();
        fm.insert("tools", Value::Mapping(policy));
    }

    let Some(Value::Sequence(disallowed)) = fm.get("disallowed-tools").cloned() else {
        return;
    };
    if !disallowed.iter().all(|entry| entry.as_str().is_some()) {
        return;
    }
    let policy = fm
        .get("tools")
        .cloned()
        .unwrap_or_else(|| Value::Mapping(Mapping::new()));
    let Value::Mapping(mut policy) = policy else {
        return;
    };
    for entry in disallowed {
        policy.insert(entry, Value::String("deny".to_string()));
    }
    fm.insert("tools", Value::Mapping(policy));
    fm.remove("disallowed-tools");
}

/// The Agent keys schema 1 replaced with a `harness-overrides` key: the consumer namespace that
/// owns it, and the name it takes there.
///
/// One conflict rule covers all three: a block already written wins, and its top-level twin is
/// dropped with a note naming it. `permission` moves only for the values Flow defines; see
/// [`moves_under_namespace`].
const MOVED_AGENT_FIELDS: &[(&str, &str, &str)] = &[
    ("meridian-flow", "permission", "permission"),
    (
        "meridian-cli",
        "resident-rearm-budget",
        "resident-rearm-budget",
    ),
    ("meridian-cli", "meridian-capabilities", "capabilities"),
];

/// Whether the top-level `key` moves under its consumer namespace. Meridian Flow defines
/// `read` and `edit` only, so any other `permission` value stays where it is for the reader to report.
fn moves_under_namespace(fm: &Frontmatter, key: &str) -> bool {
    key != "permission"
        || fm
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|permission| matches!(permission, "read" | "edit"))
}

/// Move the Agent keys schema 1 replaced with a `harness-overrides` key. An existing block wins,
/// and the top-level twin it displaces is dropped with a note: the reader would otherwise report
/// the key `mars migrate` left behind.
fn move_agent_fields(fm: &mut Frontmatter, notes: &mut Vec<SchemaDiagnostic>) {
    let mut displaced = Vec::new();
    for &namespace in crate::harness::RESERVED_CONSUMER_NAMESPACES {
        let keys = MOVED_AGENT_FIELDS
            .iter()
            .filter_map(|(owner, from, to)| {
                (*owner == namespace && moves_under_namespace(fm, from)).then_some((*from, *to))
            })
            .collect::<Vec<_>>();
        if let Moved::Done {
            displaced: dropped, ..
        } = move_keys_under(fm, namespace, &keys)
        {
            displaced.extend(dropped.into_iter().map(|(from, to)| (namespace, from, to)));
        }
    }
    for (namespace, from, to) in displaced {
        notes.push(dropped_twin(fm, namespace, from, to));
    }
}

/// Rename `harness-overrides.flow` and `.meridian` to the full host names, in place. A block
/// already written under the new name wins, and the old one is dropped with a note.
fn rename_consumer_namespaces(fm: &mut Frontmatter, notes: &mut Vec<SchemaDiagnostic>) {
    let Some(Value::Mapping(overrides)) = fm.get_mut("harness-overrides") else {
        return;
    };
    for &(old, new) in crate::harness::RENAMED_CONSUMER_NAMESPACES {
        if !overrides.contains_key(old) {
            continue;
        }
        if overrides.contains_key(new) {
            overrides.remove(old);
            notes.push(SchemaDiagnostic::new(
                "field.removed",
                Severity::Warning,
                vec![
                    PathSegment::Key("harness-overrides".to_string()),
                    PathSegment::Key(old.to_string()),
                ],
                format!(
                    "dropped `harness-overrides.{old}`; `harness-overrides.{new}` already sets it"
                ),
            ));
            continue;
        }
        // Rebuilt so the renamed block keeps its place among its siblings.
        *overrides = std::mem::take(overrides)
            .into_iter()
            .map(|(key, value)| match key.as_str() {
                Some(name) if name == old => (Value::String(new.to_string()), value),
                _ => (key, value),
            })
            .collect();
    }
}

/// The note for a top-level key dropped because its namespace block already sets one. A dropped
/// `permission` carried a restriction, which the note names as lost (`field.restriction-lost`, as
/// the set-aside reports the same loss).
fn dropped_twin(fm: &Frontmatter, namespace: &str, from: &str, to: &str) -> SchemaDiagnostic {
    let path = vec![PathSegment::Key(from.to_string())];
    if from == "permission" {
        let agent = fm.get("name").and_then(Value::as_str).map_or_else(
            || "this agent".to_string(),
            |name| format!("agent `{name}`"),
        );
        return SchemaDiagnostic::new(
            crate::set_aside::RESTRICTION_LOST,
            Severity::Warning,
            path,
            format!(
                "{agent} lost the restriction `permission`: it was dropped, because \
                 `harness-overrides.{namespace}.{to}` already sets a value, and an override \
                 written wins"
            ),
        );
    }
    SchemaDiagnostic::new(
        "field.removed",
        Severity::Warning,
        path,
        format!("dropped `{from}`; `harness-overrides.{namespace}.{to}` already sets it"),
    )
}

fn strip_model_policy_overrides(fm: &mut Frontmatter, diagnostics: &mut Vec<SchemaDiagnostic>) {
    let Some(Value::Sequence(policies)) = fm.get_mut("model-policies") else {
        return;
    };
    for (index, policy) in policies.iter_mut().enumerate() {
        let Value::Mapping(policy) = policy else {
            continue;
        };
        let Some(Value::Mapping(overrides)) = policy.get_mut(Value::String("override".to_string()))
        else {
            continue;
        };
        // The legacy effort spellings mean the same here as at the top level.
        for (old, new) in [("adaptive", "medium"), ("disabled", "none")] {
            let key = Value::String("effort".to_string());
            if overrides.get(&key).and_then(Value::as_str) == Some(old) {
                overrides.insert(key, Value::String(new.to_string()));
            }
        }
        for key in ["autocompact", "autocompact_pct", "harness"] {
            if overrides.remove(Value::String(key.to_string())).is_some() {
                diagnostics.push(SchemaDiagnostic::new(
                    "field.removed",
                    Severity::Warning,
                    vec![
                        PathSegment::Key("model-policies".to_string()),
                        PathSegment::Index(index),
                        PathSegment::Key("override".to_string()),
                        PathSegment::Key(key.to_string()),
                    ],
                    format!("removed model policy override key `{key}`"),
                ));
            }
        }
    }
}

fn rename_field(fm: &mut Frontmatter, old: &str, new: &str) {
    let Some(value) = fm.remove(old) else {
        return;
    };
    if fm.get(new).is_none() {
        fm.insert(new, value);
    }
}

/// A host set written under both spellings keeps every host of both: dropping either would
/// lose a deny. A value that is not a host list or a `{replace: [...]}` map stays where it is, so
/// the reader reports it.
fn merge_host_set_spellings(fm: &mut Frontmatter, old: &str, new: &str) {
    let (Some(old_value), Some(new_value)) = (fm.get(old), fm.get(new)) else {
        return rename_field(fm, old, new);
    };
    let (Some((old_hosts, old_replaces)), Some((mut hosts, new_replaces))) =
        (host_set_parts(old_value), host_set_parts(new_value))
    else {
        return;
    };
    for host in old_hosts {
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    let merged = if old_replaces || new_replaces {
        let mut replace = serde_yaml::Mapping::new();
        replace.insert(Value::String("replace".to_string()), Value::Sequence(hosts));
        Value::Mapping(replace)
    } else {
        Value::Sequence(hosts)
    };
    fm.insert(new, merged);
    fm.remove(old);
}

fn host_set_parts(value: &Value) -> Option<(Vec<Value>, bool)> {
    match value {
        Value::Sequence(hosts) => Some((hosts.clone(), false)),
        Value::Mapping(map) if map.len() == 1 => map
            .get("replace")
            .and_then(Value::as_sequence)
            .map(|hosts| (hosts.clone(), true)),
        _ => None,
    }
}

fn replace_scalar(fm: &mut Frontmatter, key: &str, old: &str, new: &str) {
    if fm.get(key).and_then(Value::as_str) == Some(old) {
        fm.insert(key, Value::String(new.to_string()));
    }
}

fn removed(path: &[&str], key: &str) -> SchemaDiagnostic {
    SchemaDiagnostic::new(
        "field.removed",
        Severity::Warning,
        path.iter()
            .map(|part| PathSegment::Key((*part).to_string()))
            .collect(),
        format!("removed legacy key `{key}`"),
    )
}
