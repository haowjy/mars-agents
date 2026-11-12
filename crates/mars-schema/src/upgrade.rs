//! Pure legacy Mars definition and manifest upgrades to schema v1.

use serde_yaml::{Mapping, Value};
use toml_edit::{DocumentMut, Item};

use crate::claude;
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::frontmatter::{Frontmatter, FrontmatterError};
use crate::harness_overrides::{Moved, move_keys_under};
use crate::item::DefinitionKind;
use crate::keys;
use crate::tool_policy;

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
    #[error(
        "`[package.tools]` needs `[package] schema = 1`; older Mars versions ignore it, so a package without a schema cannot carry it"
    )]
    PackageToolsNeedSchema,
}

/// A fold of one pre-v1 skill key into another field; whether it changed the frontmatter.
pub(crate) type Fold = fn(&mut Frontmatter, &mut Vec<SchemaDiagnostic>) -> bool;

/// The grammar of pre-v1 Mars: every key and value the upgrade below drops, renames, moves or
/// rewrites. The upgrade functions iterate it, and the strict reader asks [`PreV1::key`] and
/// [`PreV1::value`], so a file the upgrade would change is the file the reader refuses and the
/// two cannot drift apart. A schema 1 file in this format is in the wrong format, not a file with
/// a typo: dropping its `disallowed-tools` or `network_deny` unread would lose a restriction.
pub(crate) struct PreV1 {
    /// Agent keys the upgrade deletes.
    pub agent_dropped: &'static [&'static str],
    /// Spellings of Agent and skill keys that became another key: (old, new).
    pub renamed: &'static [(&'static str, &'static str)],
    /// Agent host sets that became another key; both spellings merge: (old, new).
    pub agent_host_sets: &'static [(&'static str, &'static str)],
    /// Agent keys that moved under a consumer namespace of `harness-overrides`:
    /// (namespace, key, key within the namespace).
    pub agent_moved: &'static [(&'static str, &'static str, &'static str)],
    /// Agent keys that moved under a namespace when their value is a map: (namespace, key).
    /// Moved by the pre-v1 conversion only: a foreign dialect reads such a map itself (OpenCode's
    /// `permission`).
    pub agent_moved_maps: &'static [(&'static str, &'static str)],
    /// Agent values rewritten: (key, old value, new value).
    pub agent_values: &'static [(&'static str, &'static str, &'static str)],
    /// Skill keys that carried a tool policy, which schema 1 skills do not have. The deny-list
    /// spellings of [`tool_policy::DISALLOWED_SPELLINGS`] are theirs too (see
    /// [`crate::skill::tool_keys`]).
    pub skill_tool_keys: &'static [&'static str],
    /// Skill keys the upgrade folds into another field, each with the fold.
    pub skill_folded: &'static [(&'static str, Fold)],
    /// Skill keys the upgrade deletes.
    pub skill_dropped: &'static [&'static str],
    /// Model policy `override` keys the upgrade deletes.
    pub policy_override_dropped: &'static [&'static str],
    /// `[package]` keys the upgrade deletes.
    pub package_dropped: &'static [&'static str],
    /// `harness-overrides` namespaces that took their full host names: (old, new).
    pub namespaces: &'static [(&'static str, &'static str)],
}

pub(crate) const PRE_V1: PreV1 = PreV1 {
    agent_dropped: &[
        "fanout",
        "models",
        "mcp-tools",
        "autocompact",
        "autocompact_pct",
    ],
    renamed: &[
        ("model_invocable", "model-invocable"),
        ("user_invocable", "user-invocable"),
    ],
    agent_host_sets: &[
        ("network_allow", "network-allow"),
        ("network_deny", "network-deny"),
    ],
    agent_moved: &[
        ("meridian-flow", "permission", "permission"),
        (
            "meridian-cli",
            "resident-rearm-budget",
            "resident-rearm-budget",
        ),
        ("meridian-cli", "meridian-capabilities", "capabilities"),
    ],
    agent_moved_maps: &[("opencode", "permission")],
    agent_values: &[
        ("approval", "yolo", "never"),
        ("effort", "adaptive", "medium"),
        ("effort", "disabled", "none"),
    ],
    skill_tool_keys: &["tools", "allowed-tools", "allowed_tools"],
    skill_folded: &[
        ("type", fold_type),
        ("when_to_use", fold_when_to_use),
        ("disable-model-invocation", fold_disable_model_invocation),
    ],
    skill_dropped: &["invocation", "allow_implicit_invocation"],
    policy_override_dropped: &["autocompact", "autocompact_pct", "harness"],
    package_dropped: &["visibility"],
    namespaces: &[("flow", "meridian-flow"), ("meridian", "meridian-cli")],
};

impl PreV1 {
    /// Whether `key` of a `kind` definition is pre-v1 Mars's.
    pub(crate) fn key(&self, kind: DefinitionKind, key: &str) -> bool {
        if self.renamed.iter().any(|(old, _)| *old == key) {
            return true;
        }
        match kind {
            DefinitionKind::Agent => {
                self.agent_dropped.contains(&key)
                    || self.agent_host_sets.iter().any(|(old, _)| *old == key)
                    || tool_policy::DISALLOWED_SPELLINGS.contains(&key)
                    || self.agent_moved.iter().any(|(_, moved, _)| *moved == key)
                    || self.agent_moved_maps.iter().any(|(_, moved)| *moved == key)
            }
            DefinitionKind::Skill => {
                crate::skill::tool_keys().any(|known| known == key)
                    || self.skill_folded.iter().any(|(known, _)| *known == key)
                    || self.skill_dropped.contains(&key)
            }
        }
    }

    /// Whether `value` of the Agent key `key` is one the upgrade rewrites.
    pub(crate) fn value(&self, key: &str, value: &str) -> bool {
        self.agent_values
            .iter()
            .any(|(known, old, _)| *known == key && *old == value)
    }

    pub(crate) fn namespace(&self, old: &str) -> bool {
        self.namespaces.iter().any(|(known, _)| *known == old)
    }
}

/// What the strict reader says of a key the upgrade would change.
pub(crate) fn pre_v1_key(path: Vec<PathSegment>, key: &str) -> SchemaDiagnostic {
    SchemaDiagnostic::new(
        "field.removed",
        Severity::Error,
        path,
        format!("`{key}` is a pre-v1 key; `mars migrate` upgrades this file"),
    )
}

/// What the strict reader says of a value the upgrade would rewrite, at `path`.
pub(crate) fn pre_v1_value(path: Vec<PathSegment>, key: &str, value: &str) -> SchemaDiagnostic {
    SchemaDiagnostic::new(
        "field.removed",
        Severity::Error,
        path,
        format!("`{key}: {value}` is a pre-v1 value; `mars migrate` upgrades this file"),
    )
}

pub fn upgrade_definition(kind: DefinitionKind, text: &str) -> Result<UpgradeResult, UpgradeError> {
    rewrite(text, |frontmatter, diagnostics| match kind {
        DefinitionKind::Agent => upgrade_agent(frontmatter, diagnostics),
        DefinitionKind::Skill => upgrade_skill(frontmatter, diagnostics),
    })
}

/// `mars migrate`'s conversion of a schema 1 definition: [`PRE_V1`]'s keys and values, converted
/// as the pre-v1 upgrade converts them, so the file the strict reader refuses for one of them is
/// the file this makes readable. Everything else in the file is left as written, including a
/// key or value the reader rejects that pre-v1 Mars never wrote. Every reading caller leaves it
/// off, because an authored schema 1 file is read exactly as written.
pub fn migrate_definition(kind: DefinitionKind, text: &str) -> Result<UpgradeResult, UpgradeError> {
    rewrite(text, |frontmatter, notes| {
        convert_pre_v1(frontmatter, kind, notes);
        crate::set_aside::set_aside_rejected_keys(
            frontmatter,
            kind,
            crate::set_aside::Destination::Nowhere,
            crate::set_aside::Reach::PreV1Keys,
            notes,
        );
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
    let mut diagnostics = Vec::new();
    let mut edited_paths = Vec::new();
    if package.get("schema").and_then(Item::as_integer) != Some(1) {
        if package.contains_key("tools") {
            return Err(UpgradeError::PackageToolsNeedSchema);
        }
        package.insert("schema", toml_edit::value(1));
        edited_paths.push(package_path("schema"));
    }
    for &key in PRE_V1.package_dropped {
        if package.remove(key).is_some() {
            diagnostics.push(removed(&["package", key], key));
            edited_paths.push(package_path(key));
        }
    }
    if edited_paths.is_empty() {
        return Ok(UpgradeResult {
            text: text.to_string(),
            diagnostics,
            fully_rewritten: false,
            edited_paths,
        });
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

/// Add `requires-mars` at `floor` to a manifest that has no requirement.
///
/// `mars migrate` runs this on the manifest it is about to write, with the floor
/// [`keys::floor`] finds for the whole package, so a package needs no hand edit. An existing
/// requirement is never rewritten: when it admits a Mars older than the floor,
/// [`FloorOutcome::warning`] is `mars check`'s advice and the value keeps its bytes and comment.
/// A manifest migrate cannot edit minimally (an inline `[package]` table) comes back unchanged
/// with the same advice, never an error, so an already-v1 package migrates exactly as it did
/// before this key existed.
pub fn ensure_requires_mars_floor(
    text: &str,
    floor: Option<&keys::Floor>,
) -> Result<FloorOutcome, UpgradeError> {
    let mut document = text.parse::<DocumentMut>()?;
    let package = document
        .get_mut("package")
        .ok_or(UpgradeError::MissingPackage)?;
    let Some(floor) = floor else {
        return Ok(FloorOutcome {
            text: text.to_string(),
            warning: None,
        });
    };
    let requirement = package
        .as_table_like()
        .and_then(|table| table.get("requires-mars"))
        .and_then(Item::as_str)
        .map(str::to_owned);
    if requirement.is_none()
        && let Some(table) = package.as_table_mut()
    {
        table.insert("requires-mars", toml_edit::value(floor.requirement()));
        return Ok(FloorOutcome {
            text: with_line_ending_of(text, document.to_string()),
            warning: None,
        });
    }
    Ok(FloorOutcome {
        text: text.to_string(),
        warning: crate::package::requires_mars_shortfall(requirement.as_deref(), floor),
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
            } else if let Some(edited) = grown_in_place(&block, before_value, after_value)
                .or_else(|| renamed_in_place(&block, before_value, after_value))
            {
                output.push_str(&edited);
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

/// `block` with its host namespace keys renamed where they are written, so the comments, flow
/// style and quoting around them survive. `None` when `after` is more than the renames of
/// `before`, or a renamed key is not written as a plain key at the block's own indentation.
fn renamed_in_place(block: &SourceBlock<'_>, before: &Value, after: &Value) -> Option<String> {
    let (Value::Mapping(before), Value::Mapping(after)) = (before, after) else {
        return None;
    };
    if before.len() != after.len() {
        return None;
    }
    let mut renames = Vec::new();
    for ((old, old_value), (new, new_value)) in before.iter().zip(after) {
        if old_value != new_value {
            return None;
        }
        if old != new {
            let pair = (old.as_str()?, new.as_str()?);
            if !PRE_V1.namespaces.contains(&pair) {
                return None;
            }
            renames.push(pair);
        }
    }
    let indent = child_indent(block)?;
    let mut done = 0;
    let mut edited = String::with_capacity(block.text.len());
    for line in block.text.split_inclusive('\n') {
        let renamed = line.strip_prefix(indent).and_then(|rest| {
            renames.iter().find_map(|&(old, new)| {
                let after_key = rest.strip_prefix(old)?;
                after_key
                    .trim_start_matches(' ')
                    .starts_with(':')
                    .then(|| format!("{indent}{new}{after_key}"))
            })
        });
        match renamed {
            Some(line) => {
                done += 1;
                edited.push_str(&line);
            }
            None => edited.push_str(line),
        }
    }
    (done == renames.len()).then_some(edited)
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
    let new = PRE_V1
        .renamed
        .iter()
        .find_map(|&(known, new)| (known == old).then_some(new))?;
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
    for &(old, new) in PRE_V1.renamed {
        rename_invocability(fm, old, new, notes);
    }
    if kind == DefinitionKind::Agent {
        for &(old, new) in PRE_V1.agent_host_sets {
            merge_host_set_spellings(fm, old, new);
        }
        for &key in PRE_V1.agent_dropped {
            if fm.remove(key).is_some() {
                notes.push(removed(&[key], key));
            }
        }
        for &(key, old, new) in PRE_V1.agent_values {
            replace_scalar(fm, key, old, new);
        }
        move_agent_fields(fm, notes);
        strip_model_policy_overrides(fm, notes);
    }
    fm.mapping() != &before
}

/// Every conversion [`PRE_V1`] names, applied to a definition of either format.
fn convert_pre_v1(fm: &mut Frontmatter, kind: DefinitionKind, notes: &mut Vec<SchemaDiagnostic>) {
    // Before `move_agent_fields`, so a top-level key lands in the block it names, not beside a
    // block still written under the old name.
    rename_consumer_namespaces(fm, notes);
    match kind {
        DefinitionKind::Agent => {
            fold_deny_lists(fm);
            lift_mars_only_keys(fm, kind, notes);
            move_map_fields(fm, notes);
        }
        DefinitionKind::Skill => {
            lift_mars_only_keys(fm, kind, notes);
            for &(_, fold) in PRE_V1.skill_folded {
                fold(fm, notes);
            }
            for &key in PRE_V1.skill_dropped {
                if fm.remove(key).is_some() {
                    notes.push(removed(&[key], key));
                }
            }
            for key in crate::skill::tool_keys() {
                if fm.remove(key).is_some() {
                    notes.push(SchemaDiagnostic::new(
                        "skill.tools-dropped",
                        Severity::Warning,
                        vec![PathSegment::Key(key.to_string())],
                        format!("removed skill key `{key}`; skill tool rules have no v1 meaning"),
                    ));
                }
            }
        }
    }
}

fn upgrade_agent(fm: &mut Frontmatter, diagnostics: &mut Vec<SchemaDiagnostic>) {
    // Only here: in a schema 1 file a `tools` list is ambiguous (grants in pre-v1 Mars, an
    // allowlist in Claude's dialect), so `mars migrate` leaves it for the reader to refuse.
    tools_list_to_allow(fm);
    convert_pre_v1(fm, DefinitionKind::Agent, diagnostics);
    // A pre-v1 file passed unknown keys to every harness; Claude's own keep reaching Claude.
    claude::translate::move_to_claude_overrides(fm, DefinitionKind::Agent, diagnostics);
    set_aside_rejected_keys(fm, DefinitionKind::Agent, diagnostics);
}

fn upgrade_skill(fm: &mut Frontmatter, diagnostics: &mut Vec<SchemaDiagnostic>) {
    convert_pre_v1(fm, DefinitionKind::Skill, diagnostics);
    claude::translate::move_to_claude_overrides(fm, DefinitionKind::Skill, diagnostics);
    set_aside_rejected_keys(fm, DefinitionKind::Skill, diagnostics);
}

/// Pre-v1 `type` becomes `metadata.type`; an existing `metadata.type` wins and `type` stays.
fn fold_type(fm: &mut Frontmatter, _notes: &mut Vec<SchemaDiagnostic>) -> bool {
    let Some(skill_type) = fm.get("type").cloned() else {
        return false;
    };
    let metadata = fm
        .get("metadata")
        .cloned()
        .unwrap_or_else(|| Value::Mapping(Mapping::new()));
    let Value::Mapping(mut metadata) = metadata else {
        return false;
    };
    let key = Value::String("type".to_string());
    if metadata.contains_key(&key) {
        return false;
    }
    metadata.insert(key, skill_type);
    fm.insert("metadata", Value::Mapping(metadata));
    fm.remove("type");
    true
}

/// Pre-v1 `when_to_use` is appended to `description`.
fn fold_when_to_use(fm: &mut Frontmatter, _notes: &mut Vec<SchemaDiagnostic>) -> bool {
    let Some(when) = fm
        .get("when_to_use")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return false;
    };
    let description = match fm.get("description") {
        Some(Value::String(description)) => format!("{description}\n\n{when}"),
        None => when,
        Some(_) => return false,
    };
    fm.insert("description", Value::String(description));
    fm.remove("when_to_use");
    true
}

/// Claude's `disable-model-invocation: true` is `model-invocable: false`. An existing
/// `model-invocable: false` stays; beside `model-invocable: true` the restricting value wins, with
/// a note.
pub(crate) fn fold_disable_model_invocation(
    fm: &mut Frontmatter,
    notes: &mut Vec<SchemaDiagnostic>,
) -> bool {
    let Some(disable) = fm.remove("disable-model-invocation") else {
        return false;
    };
    if disable.as_bool() != Some(true) {
        return true;
    }
    match crate::invocability::find_invocability_field(fm, "model-invocable") {
        None => fm.insert("model-invocable", Value::Bool(false)),
        Some(field) if field.value == Value::Bool(true) => {
            fm.insert(&field.consumed_key, Value::Bool(false));
            notes.push(conflicting_duplicate(
                "disable-model-invocation",
                &disable,
                &field.consumed_key,
                &field.value,
            ));
        }
        Some(_) => {}
    }
    true
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
        crate::set_aside::Reach::Everything,
        diagnostics,
    );
}

/// A `tools` list becomes allow rules.
fn tools_list_to_allow(fm: &mut Frontmatter) {
    if let Some(Value::Sequence(entries)) = fm.get("tools").cloned()
        && entries.iter().all(|entry| entry.as_str().is_some())
    {
        let policy = entries
            .into_iter()
            .map(|entry| (entry, Value::String("allow".to_string())))
            .collect();
        fm.insert("tools", Value::Mapping(policy));
    }
}

/// Every deny list (`disallowed-tools` and the other spellings) becomes deny rules. A deny list
/// that is not a list of names stays for the reader.
fn fold_deny_lists(fm: &mut Frontmatter) {
    for key in tool_policy::DISALLOWED_SPELLINGS {
        let Some(Value::Sequence(disallowed)) = fm.get(key).cloned() else {
            continue;
        };
        if !disallowed.iter().all(|entry| entry.as_str().is_some()) {
            continue;
        }
        let Value::Mapping(mut policy) = fm
            .get("tools")
            .cloned()
            .unwrap_or_else(|| Value::Mapping(Mapping::new()))
        else {
            return;
        };
        for entry in disallowed {
            policy.insert(entry, Value::String("deny".to_string()));
        }
        fm.insert("tools", Value::Mapping(policy));
        fm.remove(key);
    }
}

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
    for &(namespace, from, to) in PRE_V1.agent_moved {
        if moves_under_namespace(fm, from) {
            move_one(fm, namespace, from, to, notes);
        }
    }
}

/// Move the keys of [`PreV1::agent_moved_maps`] whose value is a map.
fn move_map_fields(fm: &mut Frontmatter, notes: &mut Vec<SchemaDiagnostic>) {
    for &(namespace, key) in PRE_V1.agent_moved_maps {
        if matches!(fm.get(key), Some(Value::Mapping(_))) {
            move_one(fm, namespace, key, key, notes);
        }
    }
}

fn move_one(
    fm: &mut Frontmatter,
    namespace: &str,
    from: &str,
    to: &str,
    notes: &mut Vec<SchemaDiagnostic>,
) {
    if let Moved::Done { displaced, .. } = move_keys_under(fm, namespace, &[(from, to)])
        && !displaced.is_empty()
    {
        notes.push(dropped_twin(
            &agent_label(fm),
            vec![PathSegment::Key(from.to_string())],
            from,
            namespace,
            to,
        ));
    }
}

/// Rename `harness-overrides.flow` and `.meridian` to the full host names, in place. When the
/// block under the new name exists too, the two merge key by key: a key only the old block sets
/// moves over, and a key both set keeps the new block's value, the old one dropped with a note.
fn rename_consumer_namespaces(fm: &mut Frontmatter, notes: &mut Vec<SchemaDiagnostic>) {
    let agent = agent_label(fm);
    let Some(Value::Mapping(overrides)) = fm.get_mut("harness-overrides") else {
        return;
    };
    for &(old, new) in PRE_V1.namespaces {
        let old_key = Value::String(old.to_string());
        let new_key = Value::String(new.to_string());
        let Some(old_block) = overrides.get(&old_key).cloned() else {
            continue;
        };
        let block_path = vec![
            PathSegment::Key("harness-overrides".to_string()),
            PathSegment::Key(old.to_string()),
        ];
        let Some(new_block) = overrides.get_mut(&new_key) else {
            // Rebuilt so the renamed block keeps its place among its siblings.
            *overrides = std::mem::take(overrides)
                .into_iter()
                .map(|(key, value)| {
                    if key == old_key {
                        (new_key.clone(), value)
                    } else {
                        (key, value)
                    }
                })
                .collect();
            continue;
        };
        match (new_block, old_block) {
            (Value::Mapping(new_block), Value::Mapping(old_block)) => {
                let twin = Twin {
                    agent: &agent,
                    old,
                    new,
                };
                merge_old_block(new_block, old_block, &twin, &[], notes);
            }
            _ => notes.push(SchemaDiagnostic::new(
                "field.removed",
                Severity::Warning,
                block_path,
                format!(
                    "dropped `harness-overrides.{old}`; `harness-overrides.{new}` already sets it"
                ),
            )),
        }
        overrides.remove(&old_key);
    }
}

fn agent_label(fm: &Frontmatter) -> String {
    fm.get("name").and_then(Value::as_str).map_or_else(
        || "this agent".to_string(),
        |name| format!("agent `{name}`"),
    )
}

/// The two namespace blocks being merged, for naming a dropped key in its note.
struct Twin<'a> {
    agent: &'a str,
    old: &'a str,
    new: &'a str,
}

/// Fold `old` into `new` key by key. A key only `old` sets moves over (a non-string key too:
/// the reader reports it), mappings on both sides merge recursively, and any other clash keeps
/// the `new` value and notes the dropped one.
fn merge_old_block(
    new: &mut Mapping,
    old: Mapping,
    twin: &Twin,
    rel: &[String],
    notes: &mut Vec<SchemaDiagnostic>,
) {
    for (key, value) in old {
        let name = key
            .as_str()
            .map_or_else(|| format!("{key:?}"), str::to_owned);
        let mut key_rel = rel.to_vec();
        key_rel.push(name);
        match new.get_mut(&key) {
            None => {
                new.insert(key, value);
            }
            Some(existing) if *existing == value => {}
            Some(Value::Mapping(existing)) if value.is_mapping() => {
                let Value::Mapping(value) = value else {
                    unreachable!()
                };
                merge_old_block(existing, value, twin, &key_rel, notes);
            }
            Some(_) => {
                let dotted = key_rel.join(".");
                let mut path = vec![
                    PathSegment::Key("harness-overrides".to_string()),
                    PathSegment::Key(twin.old.to_string()),
                ];
                path.extend(key_rel.into_iter().map(PathSegment::Key));
                let dropped = format!("harness-overrides.{}.{dotted}", twin.old);
                notes.push(dropped_twin(twin.agent, path, &dropped, twin.new, &dotted));
            }
        }
    }
}

/// The note for a value `dropped` (named as written) because the `namespace` block already sets
/// `key` (dotted, relative to the block). A dropped `permission` or `capabilities` entry carried
/// a restriction, which the note names as lost (`field.restriction-lost`, as the set-aside
/// reports the same loss).
fn dropped_twin(
    agent: &str,
    path: Vec<PathSegment>,
    dropped: &str,
    namespace: &str,
    key: &str,
) -> SchemaDiagnostic {
    if key == "permission" || key == "capabilities" || key.starts_with("capabilities.") {
        return SchemaDiagnostic::new(
            crate::set_aside::RESTRICTION_LOST,
            Severity::Warning,
            path,
            format!(
                "{agent} lost the restriction `{key}`: it was dropped, because \
                 `harness-overrides.{namespace}.{key}` already sets a value, and an override \
                 written wins"
            ),
        );
    }
    SchemaDiagnostic::new(
        "field.removed",
        Severity::Warning,
        path,
        format!("dropped `{dropped}`; `harness-overrides.{namespace}.{key}` already sets it"),
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
        // The legacy values mean the same here as at the top level.
        for &(key, old, new) in PRE_V1.agent_values {
            let key = Value::String(key.to_string());
            if overrides.get(&key).and_then(Value::as_str) == Some(old) {
                overrides.insert(key, Value::String(new.to_string()));
            }
        }
        for &key in PRE_V1.policy_override_dropped {
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

/// Rename an invocability key. Written under both spellings with different values, the restricting
/// one (`false`) is kept, with a note; any other clash keeps the current spelling's value.
fn rename_invocability(
    fm: &mut Frontmatter,
    old: &str,
    new: &str,
    notes: &mut Vec<SchemaDiagnostic>,
) {
    let (Some(old_value), Some(new_value)) = (fm.get(old).cloned(), fm.get(new).cloned()) else {
        return rename_field(fm, old, new);
    };
    fm.remove(old);
    if old_value != new_value
        && (old_value == Value::Bool(false) || new_value == Value::Bool(false))
    {
        fm.insert(new, Value::Bool(false));
        notes.push(conflicting_duplicate(old, &old_value, new, &new_value));
    }
}

/// The note for two keys that set one thing differently, where the restricting `false` was kept.
fn conflicting_duplicate(
    first: &str,
    first_value: &Value,
    second: &str,
    second_value: &Value,
) -> SchemaDiagnostic {
    SchemaDiagnostic::new(
        "field.removed",
        Severity::Warning,
        vec![PathSegment::Key(first.to_string())],
        format!(
            "`{first}: {}` conflicts with `{second}: {}`; kept `false`, the value that restricts",
            crate::ordered::plain(first_value),
            crate::ordered::plain(second_value),
        ),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A value that suits the key; any key without one takes `x`.
    fn sample_value(key: &str) -> &'static str {
        match key {
            "permission" => "read",
            "resident-rearm-budget" => "3",
            "meridian-capabilities" => "{spawn: true}",
            "network_allow" | "network_deny" => "[example.com]",
            _ if tool_policy::DISALLOWED_SPELLINGS.contains(&key) => "[Bash]",
            _ => "x",
        }
    }

    /// One file per row of [`PRE_V1`]: (what the row is, the file path, the file).
    fn rows() -> Vec<(String, &'static str, String)> {
        const AGENT: &str = "agents/a.md";
        const SKILL: &str = "skills/s/SKILL.md";
        let agent = |field: String| format!("---\nname: a\ndescription: d\n{field}\n---\nbody\n");
        let skill = |field: String| format!("---\nname: s\ndescription: d\n{field}\n---\nbody\n");
        let line = |key: &str| format!("{key}: {}", sample_value(key));
        let mut rows = Vec::new();
        for key in PRE_V1.agent_dropped {
            rows.push((format!("agent_dropped {key}"), AGENT, agent(line(key))));
        }
        for (key, _) in PRE_V1.renamed {
            rows.push((
                format!("renamed {key}"),
                AGENT,
                agent(format!("{key}: true")),
            ));
            rows.push((
                format!("renamed {key}"),
                SKILL,
                skill(format!("{key}: true")),
            ));
        }
        for (key, _) in PRE_V1.agent_host_sets {
            rows.push((format!("agent_host_sets {key}"), AGENT, agent(line(key))));
        }
        for key in tool_policy::DISALLOWED_SPELLINGS {
            rows.push((format!("disallowed {key}"), AGENT, agent(line(key))));
        }
        for (_, key, _) in PRE_V1.agent_moved {
            rows.push((format!("agent_moved {key}"), AGENT, agent(line(key))));
        }
        for (namespace, key) in PRE_V1.agent_moved_maps {
            rows.push((
                format!("agent_moved_maps {namespace} {key}"),
                AGENT,
                agent(format!("{key}: {{bash: deny}}")),
            ));
        }
        for (key, old, _) in PRE_V1.agent_values {
            let policy = format!(
                "model-policies:\n  - match: {{alias: fast}}\n    override:\n      {key}: {old}"
            );
            rows.push((
                format!("agent_values {key}: {old}"),
                AGENT,
                agent(format!("{key}: {old}")),
            ));
            rows.push((
                format!("agent_values {key}: {old} in a policy override"),
                AGENT,
                agent(policy),
            ));
        }
        for key in crate::skill::tool_keys() {
            rows.push((format!("skill tool {key}"), SKILL, skill(line(key))));
        }
        for (key, _) in PRE_V1.skill_folded {
            rows.push((format!("skill_folded {key}"), SKILL, skill(line(key))));
        }
        for key in PRE_V1.skill_dropped {
            rows.push((format!("skill_dropped {key}"), SKILL, skill(line(key))));
        }
        for key in PRE_V1.policy_override_dropped {
            let policy = format!(
                "model-policies:\n  - match: {{alias: fast}}\n    override:\n      effort: low\n      {key}: {}",
                sample_value(key)
            );
            rows.push((
                format!("policy_override_dropped {key}"),
                AGENT,
                agent(policy),
            ));
        }
        for (old, _) in PRE_V1.namespaces {
            let overrides = format!("harness-overrides:\n  {old}:\n    permission: read");
            rows.push((format!("namespaces {old}"), AGENT, agent(overrides)));
        }
        for key in PRE_V1.package_dropped {
            let manifest = format!(
                "[package]\nname = \"p\"\nversion = \"0.1.0\"\nschema = 1\n{key} = \"x\"\n"
            );
            rows.push((format!("package_dropped {key}"), "mars.toml", manifest));
        }
        rows
    }

    fn migrate_with_notes(path: &str, text: &str) -> UpgradeResult {
        match path {
            "mars.toml" => upgrade_manifest(text).unwrap(),
            "agents/a.md" => migrate_definition(DefinitionKind::Agent, text).unwrap(),
            _ => migrate_definition(DefinitionKind::Skill, text).unwrap(),
        }
    }

    fn migrate(path: &str, text: &str) -> String {
        migrate_with_notes(path, text).text
    }

    fn errors(path: &str, text: &str) -> Vec<String> {
        crate::read::read(path, text)
            .unwrap()
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.is_error())
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message()))
            .collect()
    }

    /// The reader refuses a schema 1 file exactly when `mars migrate` converts it, for every row
    /// of the list both read.
    #[test]
    fn the_reader_refuses_what_migrate_converts() {
        for (row, path, text) in rows() {
            let refused = errors(path, &text);
            assert!(!refused.is_empty(), "{row}: the reader accepts\n{text}");
            for error in &refused {
                assert!(
                    error.starts_with("field.removed: ")
                        && error.ends_with("; `mars migrate` upgrades this file")
                        && (error.contains(" is a pre-v1 key;")
                            || error.contains(" is a pre-v1 value;")),
                    "{row}: the reader refuses it for another reason: {error}"
                );
            }
            let result = migrate_with_notes(path, &text);
            let migrated = result.text.clone();
            assert_ne!(migrated, text, "{row}: migrate leaves the file as it is");
            // Setting a key aside makes the file readable too, but a row is a conversion: it
            // keeps what the key meant.
            assert!(
                result
                    .diagnostics
                    .iter()
                    .all(|note| note.code != crate::set_aside::RESTRICTION_LOST),
                "{row}: migrate drops a restriction: {:?}",
                result.diagnostics
            );
            assert_eq!(
                errors(path, &migrated),
                Vec::<String>::new(),
                "{row}: still refused after migrate\n{migrated}"
            );
            assert_eq!(migrate(path, &migrated), migrated, "{row}: not idempotent");
        }
    }

    /// The other direction: a file with no key of the list is not refused, and migrate leaves it
    /// byte for byte, including a key it does not read and a value the reader rejects.
    #[test]
    fn migrate_leaves_a_file_without_a_pre_v1_key() {
        let agent = "---\nname: a\ndescription: d\nmodel: opus\ncolor: red\ntools:\n  read: allow\n  bash: deny\nharness-overrides:\n  claude:\n    x: 1\nsandbox: bogus\n---\nbody\n";
        assert_eq!(migrate("agents/a.md", agent), agent);
        // A `tools` list is ambiguous, so the reader refuses it for its own reason and migrate
        // leaves it; a current override is accepted.
        let list = "---\nname: a\ndescription: d\ntools: [Read, Grep]\nmodel-policies:\n  - match: {alias: fast}\n    override:\n      approval: never\n      effort: low\n---\nbody\n";
        assert_eq!(migrate("agents/a.md", list), list);
        assert!(
            errors("agents/a.md", list)
                .iter()
                .all(|error| error.starts_with("tools.list-form: "))
        );
        let skill = "---\nname: s\ndescription: d\nmodel-invocable: false\ncolor: red\n---\nbody\n";
        assert_eq!(migrate("skills/s/SKILL.md", skill), skill);
        let manifest = "[package]\nname = \"p\"\nversion = \"0.1.0\"\nschema = 1\nfuture = 1\n";
        assert_eq!(migrate("mars.toml", manifest), manifest);
    }
}
