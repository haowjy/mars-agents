//! The serialized v1 reading shared by native and WASM callers.

use serde::Serialize;

use crate::diagnostic::SchemaDiagnostic;
use crate::frontmatter::Frontmatter;
use crate::item::DefinitionKind;
use crate::ordered::OrderedValue;

/// The serialized v1 reading of one schema-owned file.
///
/// Serializes as the bare [`DefinitionRead`]; the caller knows the kind from the path (or, in
/// a prepared package, from the definition's `kind`).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Reading {
    Manifest(DefinitionRead<crate::package::PackageMetadata>),
    Agent(DefinitionRead<crate::agent::AgentProfile>),
    Skill(DefinitionRead<crate::skill::SkillProfile>),
}

impl Reading {
    pub fn diagnostics(&self) -> &[SchemaDiagnostic] {
        match self {
            Self::Manifest(reading) => &reading.diagnostics,
            Self::Agent(reading) => &reading.diagnostics,
            Self::Skill(reading) => &reading.diagnostics,
        }
    }
}

/// `read` was given a path that is not a schema-owned file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{0}` is not a schema file: expected `mars.toml`, `agents/<slug>.md` or `skills/<slug>/SKILL.md`"
)]
pub struct UnrecognizedPath(pub String);

/// Read an Agent from an installed `.mars` tree.
///
/// The bytes are what the Mars that installed them wrote, not an authored schema v1 file: a tree
/// an older Mars wrote carries the pre-v1 spellings (`type`, `when_to_use`, a `tools` list,
/// `permission`, ...) that the strict reader refuses. When the strict reading reports one, the
/// file is upgraded in memory with the same conversion `prepare` applies to a pre-v1 package and
/// read again. A file that reports any other error is simply invalid, and keeps that error: only
/// a spelling the upgrade can cure says the bytes predate schema v1. The upgrade's notes are
/// about the authored file, which `mars sync` and `mars migrate` report, so only the reading is
/// returned. An authored package's own Agent is read strictly with [`crate::agent::read`].
pub fn installed_agent(text: &str) -> DefinitionRead<crate::agent::AgentProfile> {
    installed(text, DefinitionKind::Agent, crate::agent::read)
}

/// Read a skill from an installed `.mars` tree; the skill counterpart of [`installed_agent`].
pub fn installed_skill(text: &str) -> DefinitionRead<crate::skill::SkillProfile> {
    installed(text, DefinitionKind::Skill, crate::skill::read)
}

fn installed<T>(
    text: &str,
    kind: DefinitionKind,
    strict: impl Fn(&str) -> DefinitionRead<T>,
) -> DefinitionRead<T> {
    let reading = strict(text);
    // `field.removed` is a key or value the reader names, and `tools.list-form` is the one pre-v1
    // shape Mars rewrote without removing a key. Both are what `upgrade_definition` converts.
    let pre_v1 = reading.diagnostics.iter().any(|diagnostic| {
        matches!(
            diagnostic.code.as_str(),
            "field.removed" | "tools.list-form"
        )
    });
    if !pre_v1 {
        return reading;
    }
    match crate::upgrade::upgrade_definition(kind, text) {
        Ok(upgraded) => strict(&upgraded.text),
        // A file the upgrade cannot parse is one the strict reader reports on as well.
        Err(_) => reading,
    }
}

/// Read one schema-owned file, choosing the reader from its package-relative path.
pub fn read(path: &str, text: &str) -> Result<Reading, UnrecognizedPath> {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["mars.toml"] => Ok(Reading::Manifest(crate::manifest::read(text))),
        ["agents", file]
            if file
                .strip_suffix(".md")
                .is_some_and(|slug| !slug.is_empty()) =>
        {
            Ok(read_definition(DefinitionKind::Agent, text))
        }
        ["skills", slug, "SKILL.md"] if !slug.is_empty() => {
            Ok(read_definition(DefinitionKind::Skill, text))
        }
        _ => Err(UnrecognizedPath(path.to_owned())),
    }
}

/// Read one Agent or skill file, the one way: the structure-preserving parse (an anchor, a
/// document-end marker and the like are diagnostics beside the fields, never a refusal to read
/// them), then the kind's field reader. `agent::read`, `skill::read`, `read` and `prepare`
/// are this function.
pub(crate) fn read_definition(kind: DefinitionKind, text: &str) -> Reading {
    match kind {
        DefinitionKind::Agent => Reading::Agent(crate::agent::read(text)),
        DefinitionKind::Skill => Reading::Skill(crate::skill::read(text)),
    }
}

/// What the strict field reader says about `fm`, without the structure parse.
pub(crate) fn field_diagnostics(kind: DefinitionKind, fm: &Frontmatter) -> Vec<SchemaDiagnostic> {
    let mut diagnostics = Vec::new();
    match kind {
        DefinitionKind::Agent => {
            crate::agent::read_fields(fm, &mut diagnostics);
        }
        DefinitionKind::Skill => {
            crate::skill::read_fields(fm, &mut diagnostics);
        }
    }
    diagnostics
}

/// The structure-preserving parse and ordered frontmatter shared by the Agent and skill
/// readers; `fields` is the kind's field reader. Diagnostics come in the order structure,
/// fields, then values with no JSON form.
pub(crate) fn read_with<T>(
    text: &str,
    fields: impl FnOnce(&Frontmatter, &mut Vec<SchemaDiagnostic>) -> T,
) -> DefinitionRead<T> {
    match Frontmatter::parse_preserving_structure(text) {
        Ok((frontmatter, structural_errors)) => {
            let mut diagnostics = structural_errors
                .iter()
                .map(crate::diagnostic::frontmatter_error)
                .collect::<Vec<_>>();
            let fields = fields(&frontmatter, &mut diagnostics);
            DefinitionRead {
                frontmatter: frontmatter_json(frontmatter.mapping(), &mut diagnostics),
                fields: Some(fields),
                body: Some(frontmatter.body().to_string()),
                diagnostics,
            }
        }
        Err(error) => DefinitionRead::unread(vec![crate::diagnostic::frontmatter_error(&error)]),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DefinitionRead<T> {
    /// Ordered top-level mapping, retaining unknown authored keys.
    pub frontmatter: Vec<(String, OrderedValue)>,
    pub fields: Option<T>,
    pub body: Option<String>,
    pub diagnostics: Vec<SchemaDiagnostic>,
}

/// A definition with no errors: see [`DefinitionRead::into_valid`].
#[derive(Debug, Clone)]
pub struct Valid<T> {
    pub fields: T,
    pub body: String,
    pub warnings: Vec<SchemaDiagnostic>,
}

impl<T> DefinitionRead<T> {
    /// A file that could not be read: no fields, only why.
    pub(crate) fn unread(diagnostics: Vec<SchemaDiagnostic>) -> Self {
        Self {
            frontmatter: Vec::new(),
            fields: None,
            body: None,
            diagnostics,
        }
    }

    /// The typed fields, or why there are none: the frontmatter cannot be parsed at all.
    pub fn into_fields(self) -> Result<T, String> {
        self.fields.ok_or_else(|| {
            self.diagnostics
                .first()
                .map_or("unreadable".to_owned(), |diagnostic| {
                    diagnostic.message.clone()
                })
        })
    }

    /// What a caller that acts on the file needs: the fields, the body and the warnings that
    /// came with them, or the first error. A file with any error is refused whole.
    pub fn into_valid(self) -> Result<Valid<T>, SchemaDiagnostic> {
        if let Some(error) = self
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.is_error())
        {
            return Err(error.clone());
        }
        match (self.fields, self.body) {
            (Some(fields), Some(body)) => Ok(Valid {
                fields,
                body,
                warnings: self.diagnostics,
            }),
            _ => unreachable!("a reading with no error has its fields and body"),
        }
    }
}

pub(crate) fn frontmatter_json(
    mapping: &serde_yaml::Mapping,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> Vec<(String, OrderedValue)> {
    mapping
        .iter()
        .filter_map(|(key, value)| {
            let Some(key) = key.as_str() else {
                diagnostics.push(SchemaDiagnostic::new(
                    "yaml.non-string-key",
                    crate::diagnostic::Severity::Error,
                    Vec::new(),
                    "frontmatter keys must be strings",
                ));
                return None;
            };
            match OrderedValue::from_yaml(value) {
                Ok(value) => Some((key.to_owned(), value)),
                Err(message) => {
                    diagnostics.push(SchemaDiagnostic::new(
                        "yaml.non-json-value",
                        crate::diagnostic::Severity::Error,
                        vec![crate::diagnostic::PathSegment::Key(key.to_owned())],
                        message,
                    ));
                    None
                }
            }
        })
        .collect()
}
