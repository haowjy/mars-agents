//! Pure legacy Mars definition and manifest upgrades to schema v1.

use serde_yaml::{Mapping, Value};
use toml_edit::{DocumentMut, Item};

use crate::claude;
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::frontmatter::{Frontmatter, FrontmatterError};
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
    /// What it could not carry into schema v1 and the author should know of (a restriction
    /// lost), reported on its own, not as a change.
    #[serde(default)]
    pub findings: Vec<SchemaDiagnostic>,
    /// A changed definition whose source shape prevented top-level block editing.
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
    let original = Frontmatter::parse(text)?;
    let mut frontmatter = original.clone();
    let (mut diagnostics, mut findings) = (Vec::new(), Vec::new());
    match kind {
        DefinitionKind::Agent => upgrade_agent(&mut frontmatter, &mut diagnostics, &mut findings),
        DefinitionKind::Skill => upgrade_skill(&mut frontmatter, &mut diagnostics, &mut findings),
    }
    if frontmatter.mapping() == original.mapping() {
        return Ok(UpgradeResult {
            text: text.to_string(),
            diagnostics,
            findings,
            fully_rewritten: false,
            edited_paths: Vec::new(),
        });
    }

    let edited_paths = changed_mapping_paths(original.mapping(), frontmatter.mapping());

    let fallback = with_line_ending_of(text, frontmatter.render());
    let minimally_edited = edit_top_level_blocks(text, original.mapping(), frontmatter.mapping())
        .and_then(|edited| {
            Frontmatter::parse(&edited)
                .ok()
                .filter(|parsed| parsed.mapping() == frontmatter.mapping())
                .map(|_| edited)
        });
    Ok(UpgradeResult {
        fully_rewritten: minimally_edited.is_none(),
        text: minimally_edited.unwrap_or(fallback),
        diagnostics,
        findings,
        edited_paths,
    })
}

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
            findings: Vec::new(),
            fully_rewritten: false,
            edited_paths: Vec::new(),
        });
    }
    let mut edited_paths = vec![vec![
        PathSegment::Key("package".to_string()),
        PathSegment::Key("schema".to_string()),
    ]];
    package.insert("schema", toml_edit::value(1));
    let mut diagnostics = Vec::new();
    if package.remove("visibility").is_some() {
        diagnostics.push(removed(&["package", "visibility"], "visibility"));
        edited_paths.push(vec![
            PathSegment::Key("package".to_string()),
            PathSegment::Key("visibility".to_string()),
        ]);
    }
    Ok(UpgradeResult {
        text: with_line_ending_of(text, document.to_string()),
        diagnostics,
        findings: Vec::new(),
        fully_rewritten: false,
        edited_paths,
    })
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

/// Rewrite only changed top-level blocks. Complex top-level YAML deliberately returns `None` so
/// the caller has one explicit, verified full-serialization fallback.
fn edit_top_level_blocks(text: &str, before: &Mapping, after: &Mapping) -> Option<String> {
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
            } else {
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
    Some(output)
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
        for field in REMOVED_AGENT_FIELDS.iter().filter(|field| field.discarded) {
            if fm.remove(field.key).is_some() {
                notes.push(removed(&[field.key], field.key));
            }
        }
        for removed in REMOVED_AGENT_VALUES {
            replace_scalar(fm, removed.key, removed.value, removed.replacement);
        }
        move_permission(fm);
        strip_model_policy_overrides(fm, notes);
    }
    fm.mapping() != &before
}

fn upgrade_agent(
    fm: &mut Frontmatter,
    diagnostics: &mut Vec<SchemaDiagnostic>,
    findings: &mut Vec<SchemaDiagnostic>,
) {
    upgrade_tools(fm);
    lift_mars_only_keys(fm, DefinitionKind::Agent, diagnostics);
    // A pre-v1 file passed unknown keys to every harness; Claude's own keep reaching Claude.
    claude::translate::move_to_claude_overrides(fm, DefinitionKind::Agent, diagnostics);
    set_aside_rejected_keys(fm, DefinitionKind::Agent, diagnostics, findings);
}

fn upgrade_skill(
    fm: &mut Frontmatter,
    diagnostics: &mut Vec<SchemaDiagnostic>,
    findings: &mut Vec<SchemaDiagnostic>,
) {
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
    set_aside_rejected_keys(fm, DefinitionKind::Skill, diagnostics, findings);
}

/// What the rules above could not make schema v1 is dropped, with a warning naming it, so the
/// upgrade never hands the strict reader a value it rejects. It is not kept under
/// `harness-overrides.flow`: Flow defines one key, `permission`, and `lift_mars_only_keys` writes
/// it there when it is `read` or `edit`. Anything else in that namespace would not be Flow's.
fn set_aside_rejected_keys(
    fm: &mut Frontmatter,
    kind: DefinitionKind,
    diagnostics: &mut Vec<SchemaDiagnostic>,
    findings: &mut Vec<SchemaDiagnostic>,
) {
    crate::set_aside::set_aside_rejected_keys(
        fm,
        kind,
        crate::set_aside::Destination::Nowhere,
        diagnostics,
        findings,
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

fn move_permission(fm: &mut Frontmatter) {
    let Some(permission @ ("read" | "edit")) = fm.get("permission").and_then(Value::as_str) else {
        return;
    };
    let permission = permission.to_string();
    let overrides = fm
        .get("harness-overrides")
        .cloned()
        .unwrap_or_else(|| Value::Mapping(Mapping::new()));
    let Value::Mapping(mut overrides) = overrides else {
        return;
    };
    let flow_key = Value::String("flow".to_string());
    let flow = overrides
        .get(&flow_key)
        .cloned()
        .unwrap_or_else(|| Value::Mapping(Mapping::new()));
    let Value::Mapping(mut flow) = flow else {
        return;
    };
    let permission_key = Value::String("permission".to_string());
    if flow.contains_key(&permission_key) {
        return;
    }
    flow.insert(permission_key, Value::String(permission));
    overrides.insert(flow_key, Value::Mapping(flow));
    fm.insert("harness-overrides", Value::Mapping(overrides));
    fm.remove("permission");
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
        for key in ["network", "autocompact", "autocompact_pct", "harness"] {
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
