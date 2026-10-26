//! Canonical source staging — lift foreign frontmatter before discovery/validation.
//!
//! `stage_canonical_source` copies a fetched package tree into a derived,
//! dependency-scoped directory and routes markdown frontmatter through
//! `lift_frontmatter`. Downstream pipeline stages read only the staged tree.

mod lift;
mod overlay;
mod publish;

pub(crate) use publish::{
    PublishInput, discover_inputs as discover_publish_inputs,
    inputs_for_items as publish_inputs_for_items,
};

use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::config::SkillOverlay;
use crate::diagnostic::DiagnosticCollector;
use crate::dialect::Dialect;
use crate::error::MarsError;
use crate::platform::cache::safe_component;
use crate::resolve::RootedSourceRef;
use crate::types::{RenameMap, SourceName};
use mars_schema::frontmatter::Frontmatter;
use mars_schema::item::ItemKind;

pub(crate) use lift::lift_frontmatter_with_change;
pub(crate) use overlay::{apply_skill_overlay, skill_overlay_lookup_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HookSurfaceState {
    Readable,
    Unreadable {
        hook_names: std::collections::BTreeSet<String>,
    },
}

pub(crate) struct StagedRootedSource {
    pub rooted: RootedSourceRef,
    pub hook_surface: HookSurfaceState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefinitionPreparation {
    Schema1,
    Legacy,
    Foreign(Dialect),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RootedStageOptions {
    pub preparation: DefinitionPreparation,
}

/// Stage a package tree and repoint `package_root` at the staged output.
pub(crate) fn stage_rooted_source(
    source_name: &SourceName,
    rooted: RootedSourceRef,
    options: RootedStageOptions,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    staging_root: &Path,
    diag: &mut DiagnosticCollector,
) -> Result<StagedRootedSource, MarsError> {
    stage_rooted_source_with_policy(
        source_name,
        rooted,
        options,
        skill_overrides,
        renames,
        staging_root,
        diag,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn stage_rooted_source_for_publish_inputs(
    source_name: &SourceName,
    rooted: RootedSourceRef,
    options: RootedStageOptions,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    staging_root: &Path,
    inputs: &[PublishInput],
    diag: &mut DiagnosticCollector,
) -> Result<StagedRootedSource, MarsError> {
    stage_rooted_source_with_policy(
        source_name,
        rooted,
        options,
        skill_overrides,
        renames,
        staging_root,
        diag,
        Some(inputs),
    )
}

#[allow(clippy::too_many_arguments)]
fn stage_rooted_source_with_policy(
    source_name: &SourceName,
    rooted: RootedSourceRef,
    options: RootedStageOptions,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    staging_root: &Path,
    diag: &mut DiagnosticCollector,
    publish_inputs: Option<&[PublishInput]>,
) -> Result<StagedRootedSource, MarsError> {
    let staged_package_root = staging_dir_for(staging_root, source_name);
    if staged_package_root.symlink_metadata().is_ok() {
        crate::platform::fs::safe_remove(&staged_package_root)?;
    }
    let changed = stage_canonical_source_with_policy(
        &rooted.package_root,
        &staged_package_root,
        options.preparation,
        skill_overrides,
        renames,
        Some(source_name.as_ref()),
        diag,
        publish_inputs,
    )?;
    if options.preparation == DefinitionPreparation::Legacy && changed {
        diag.warn_once(
            "legacy-package",
            format!("package `{source_name}` uses the pre-v1 format; run `mars migrate`"),
        );
    }
    let hook_surface = detect_unreadable_hook_surface(&staged_package_root, diag)?;
    Ok(StagedRootedSource {
        rooted: RootedSourceRef {
            checkout_root: rooted.checkout_root,
            package_root: staged_package_root,
        },
        hook_surface,
    })
}

fn detect_unreadable_hook_surface(
    package_root: &Path,
    diag: &mut DiagnosticCollector,
) -> Result<HookSurfaceState, MarsError> {
    let hook_dirs = crate::discover::discover_hook_directories(package_root, diag)?;
    let mut hook_names = std::collections::BTreeSet::new();
    for hook_dir in hook_dirs {
        let manifest = hook_dir.join("hook.toml");
        let raw = fs::read_to_string(&manifest)?;
        let Ok(value) = toml::from_str::<toml::Value>(&raw) else {
            continue;
        };
        if !crate::compiler::hooks::uses_removed_schema(&value) {
            continue;
        }
        if let Some(hook_name) = hook_dir.file_name().and_then(|name| name.to_str()) {
            hook_names.insert(hook_name.to_owned());
        }
    }
    if hook_names.is_empty() {
        return Ok(HookSurfaceState::Readable);
    }
    Ok(HookSurfaceState::Unreadable { hook_names })
}

/// Copy `source_root` into `dest_root`, rewriting frontmatter through lift.
pub(crate) fn stage_canonical_source(
    source_root: &Path,
    dest_root: &Path,
    dialect: Dialect,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    fallback_skill_name: Option<&str>,
    diag: &mut DiagnosticCollector,
) -> Result<(), MarsError> {
    stage_canonical_source_with_policy(
        source_root,
        dest_root,
        DefinitionPreparation::Foreign(dialect),
        skill_overrides,
        renames,
        fallback_skill_name,
        diag,
        None,
    )?;
    Ok(())
}

pub(crate) fn stage_prepared_package(
    source_root: &Path,
    dest_root: &Path,
    preparation: DefinitionPreparation,
    excluded_paths: &[PathBuf],
    diag: &mut DiagnosticCollector,
) -> Result<bool, MarsError> {
    if dest_root.exists() {
        fs::remove_dir_all(dest_root)?;
    }
    fs::create_dir_all(dest_root)?;
    copy_and_lift_tree(
        source_root,
        dest_root,
        preparation,
        &IndexMap::new(),
        &RenameMap::new(),
        None,
        excluded_paths,
        diag,
    )
}

#[allow(clippy::too_many_arguments)]
fn stage_canonical_source_with_policy(
    source_root: &Path,
    dest_root: &Path,
    preparation: DefinitionPreparation,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    fallback_skill_name: Option<&str>,
    diag: &mut DiagnosticCollector,
    publish_inputs: Option<&[PublishInput]>,
) -> Result<bool, MarsError> {
    if dest_root.exists() {
        fs::remove_dir_all(dest_root)?;
    }
    fs::create_dir_all(dest_root)?;
    match publish_inputs {
        None => copy_and_lift_tree(
            source_root,
            dest_root,
            preparation,
            skill_overrides,
            renames,
            fallback_skill_name,
            &[],
            diag,
        ),
        Some(inputs) => publish::copy_and_lift_inputs(
            source_root,
            dest_root,
            preparation,
            skill_overrides,
            renames,
            fallback_skill_name,
            inputs,
            diag,
        ),
    }
}

/// Stage a single local item path (agent file or skill directory).
#[allow(clippy::too_many_arguments)]
pub(crate) fn stage_local_item(
    source_path: &Path,
    kind: ItemKind,
    preparation: DefinitionPreparation,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    staging_root: &Path,
    item_key: &str,
    skill_overlay_key: Option<&str>,
    excluded_paths: &[PathBuf],
    diag: &mut DiagnosticCollector,
) -> Result<PathBuf, MarsError> {
    let dest = staging_root.join("_local").join(safe_component(item_key));
    if dest.symlink_metadata().is_ok() {
        crate::platform::fs::safe_remove(&dest)?;
    }
    fs::create_dir_all(dest.parent().unwrap_or(&dest))?;

    let (path, changed) = match kind {
        ItemKind::Agent | ItemKind::McpServer => {
            fs::create_dir_all(&dest)?;
            let dest_file =
                dest.join(source_path.file_name().ok_or_else(|| MarsError::Source {
                    source_name: "_local".to_string(),
                    message: format!(
                        "local agent path has no file name: {}",
                        source_path.display()
                    ),
                })?);
            let changed = process_markdown_file(
                source_path,
                source_path,
                &dest_file,
                kind,
                &mut StageOverlayContext {
                    preparation,
                    skill_overrides,
                    renames: &RenameMap::new(),
                    package_root: source_path.parent().unwrap_or(source_path),
                    fallback_skill_name: None,
                    skill_overlay_key,
                    diag,
                },
            )?;
            (dest_file, changed)
        }
        ItemKind::Skill | ItemKind::Hook | ItemKind::BootstrapDoc => {
            fs::create_dir_all(&dest)?;
            let changed = copy_and_lift_tree(
                source_path,
                &dest,
                preparation,
                skill_overrides,
                &RenameMap::new(),
                skill_overlay_key,
                excluded_paths,
                diag,
            )?;
            (dest, changed)
        }
    };
    if preparation == DefinitionPreparation::Legacy && changed {
        diag.warn_once(
            "legacy-authored-content",
            "authored `.mars-src` content uses the pre-v1 format; run `mars migrate`",
        );
    }
    Ok(path)
}

fn staging_dir_for(staging_root: &Path, source_name: &SourceName) -> PathBuf {
    staging_root.join(safe_component(source_name.as_ref()))
}

struct StageOverlayContext<'a> {
    preparation: DefinitionPreparation,
    skill_overrides: &'a IndexMap<String, SkillOverlay>,
    renames: &'a RenameMap,
    package_root: &'a Path,
    fallback_skill_name: Option<&'a str>,
    skill_overlay_key: Option<&'a str>,
    diag: &'a mut DiagnosticCollector,
}

#[allow(clippy::too_many_arguments)]
fn copy_and_lift_tree(
    source_root: &Path,
    dest_root: &Path,
    preparation: DefinitionPreparation,
    skill_overrides: &IndexMap<String, SkillOverlay>,
    renames: &RenameMap,
    fallback_skill_name: Option<&str>,
    excluded_paths: &[PathBuf],
    diag: &mut DiagnosticCollector,
) -> Result<bool, MarsError> {
    let definitions =
        crate::discover::discover_definition_files(source_root, fallback_skill_name, diag)?
            .into_iter()
            .filter_map(|(path, kind)| {
                path.strip_prefix(source_root)
                    .ok()
                    .map(|relative| (relative.to_path_buf(), kind))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
    let entries = crate::fs::authored_tree_entries(
        source_root,
        excluded_paths,
        crate::fs::AuthoredSymlinkPolicy::Follow,
        diag,
    )?;
    let mut legacy_changed = false;
    for entry in entries {
        let dest_path = dest_root.join(&entry.relative);
        if entry.kind == crate::fs::AuthoredEntryKind::Directory {
            fs::create_dir_all(&dest_path)?;
        } else if preparation == DefinitionPreparation::Legacy
            && entry.relative == Path::new("mars.toml")
        {
            let original = fs::read_to_string(&entry.source)?;
            let upgraded = mars_schema::upgrade::upgrade_manifest(&original).map_err(|error| {
                MarsError::Source {
                    source_name: "legacy package".to_string(),
                    message: format!("failed to upgrade {}: {error}", entry.source.display()),
                }
            })?;
            legacy_changed |= upgraded.changed_from(&original);
            fs::write(&dest_path, upgraded.text)?;
        } else if should_lift_markdown(&entry.relative) && definitions.contains_key(&entry.relative)
        {
            let src_path = entry.source;
            let kind = definitions
                .get(&entry.relative)
                .map(|kind| match kind {
                    mars_schema::upgrade::DefinitionKind::Agent => ItemKind::Agent,
                    mars_schema::upgrade::DefinitionKind::Skill => ItemKind::Skill,
                })
                .unwrap_or_else(|| item_kind_for_markdown(&entry.relative));
            legacy_changed |= process_markdown_file(
                &src_path,
                &source_root.join(&entry.relative),
                &dest_path,
                kind,
                &mut StageOverlayContext {
                    preparation,
                    skill_overrides,
                    renames,
                    package_root: source_root,
                    fallback_skill_name,
                    skill_overlay_key: None,
                    diag,
                },
            )?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&entry.source, &dest_path)?;
        }
    }
    Ok(legacy_changed)
}

fn should_lift_markdown(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("SKILL.md") | Some("BOOTSTRAP.md")
    ) || path
        .extension()
        .is_some_and(|ext| ext == "md" || ext == "mdc")
}

fn item_kind_for_markdown(path: &Path) -> ItemKind {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("SKILL.md") => ItemKind::Skill,
        Some("BOOTSTRAP.md") => ItemKind::BootstrapDoc,
        _ if path.extension().is_some_and(|ext| ext == "mdc") => ItemKind::Skill,
        _ => ItemKind::Agent,
    }
}

fn process_markdown_file(
    src: &Path,
    logical_src: &Path,
    dest: &Path,
    kind: ItemKind,
    ctx: &mut StageOverlayContext<'_>,
) -> Result<bool, MarsError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }

    let skill_overlay = (kind == ItemKind::Skill)
        .then(|| {
            ctx.skill_overlay_key.map(str::to_owned).or_else(|| {
                skill_overlay_lookup_name(
                    logical_src,
                    ctx.package_root,
                    ctx.renames,
                    ctx.fallback_skill_name,
                )
            })
        })
        .flatten()
        .and_then(|name| {
            ctx.skill_overrides
                .get(&name)
                .map(|overlay| (name, overlay))
        });

    let original = fs::read_to_string(src)?;
    let mut legacy_changed = false;
    if let Ok(parsed) = Frontmatter::parse(&original) {
        let (mut fm, mut changed, mut prepared_text) = match ctx.preparation {
            DefinitionPreparation::Schema1 => (parsed, false, None),
            DefinitionPreparation::Foreign(dialect) => {
                let (lifted, changed) = lift_frontmatter_with_change(dialect, kind, &parsed);
                (lifted, changed, None)
            }
            DefinitionPreparation::Legacy => {
                let definition_kind = match kind {
                    ItemKind::Skill | ItemKind::BootstrapDoc => {
                        Some(mars_schema::upgrade::DefinitionKind::Skill)
                    }
                    ItemKind::Agent => Some(mars_schema::upgrade::DefinitionKind::Agent),
                    ItemKind::Hook | ItemKind::McpServer => None,
                };
                if let Some(definition_kind) = definition_kind {
                    let upgraded =
                        mars_schema::upgrade::upgrade_definition(definition_kind, &original)
                            .map_err(|error| MarsError::Source {
                                source_name: "legacy package".to_string(),
                                message: format!(
                                    "failed to upgrade {}: {error}",
                                    logical_src.display()
                                ),
                            })?;
                    if upgraded.changed_from(&original) {
                        legacy_changed = true;
                        (
                            Frontmatter::parse(&upgraded.text).map_err(|error| {
                                MarsError::Source {
                                    source_name: "legacy package".to_string(),
                                    message: format!(
                                        "failed to read upgraded {}: {error}",
                                        logical_src.display()
                                    ),
                                }
                            })?,
                            true,
                            Some(upgraded.text),
                        )
                    } else {
                        (parsed, false, None)
                    }
                } else {
                    (parsed, false, None)
                }
            }
        };

        if let Some((_skill_name, overlay)) = skill_overlay
            && !overlay.is_empty()
        {
            let (overlaid, overlay_changed) = apply_skill_overlay(&fm, overlay);
            fm = overlaid;
            changed |= overlay_changed;
            if overlay_changed {
                prepared_text = None;
            }
        }

        if changed {
            fs::write(dest, prepared_text.unwrap_or_else(|| fm.render()))?;
        } else {
            fs::copy(src, dest)?;
        }
    } else {
        fs::copy(src, dest)?;
    }
    if matches!(kind, ItemKind::Agent | ItemKind::Skill) {
        let prepared = fs::read_to_string(dest)?;
        let reading = if kind == ItemKind::Agent {
            mars_schema::agent::read(&prepared).diagnostics
        } else {
            mars_schema::skill::read(&prepared).diagnostics
        };
        let context = format!("{kind} `{}`", logical_src.display());
        for diagnostic in &reading {
            ctx.diag.schema(diagnostic, &context, false);
        }
    }
    Ok(legacy_changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SkillOverlay;
    use crate::diagnostic::DiagnosticCollector;
    use crate::hash;
    use crate::types::{ItemName, RenameMap};
    use tempfile::TempDir;

    fn stage_source(
        source_root: &Path,
        dest_root: &Path,
        preparation: DefinitionPreparation,
        skill_overrides: &IndexMap<String, SkillOverlay>,
        renames: &RenameMap,
        fallback_skill_name: Option<&str>,
    ) -> DiagnosticCollector {
        let mut diag = DiagnosticCollector::new();
        stage_canonical_source_with_policy(
            source_root,
            dest_root,
            preparation,
            skill_overrides,
            renames,
            fallback_skill_name,
            &mut diag,
            None,
        )
        .unwrap();
        diag
    }

    #[test]
    fn empty_skill_overlay_leaves_bytes_identical() {
        let source = TempDir::new().unwrap();
        let skill = source.path().join("skills/demo");
        fs::create_dir_all(&skill).unwrap();
        let original = "---\nname: demo\ndescription: base\n---\n# Body\n";
        fs::write(skill.join("SKILL.md"), original).unwrap();

        let mut overrides = IndexMap::new();
        overrides.insert("demo".to_string(), SkillOverlay::default());

        let dest = TempDir::new().unwrap();
        stage_source(
            source.path(),
            dest.path(),
            DefinitionPreparation::Foreign(Dialect::Claude),
            &overrides,
            &RenameMap::new(),
            None,
        );

        assert_eq!(
            fs::read_to_string(dest.path().join("skills/demo/SKILL.md")).unwrap(),
            original
        );
    }

    #[test]
    fn stage_skill_directory_is_faithful_copy_with_identity_lift() {
        let source = TempDir::new().unwrap();
        let skill = source.path().join("skills/demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: demo\n---\n# Body\n").unwrap();
        fs::write(skill.join("helper.sh"), "#!/bin/sh\n").unwrap();

        let dest = TempDir::new().unwrap();
        stage_source(
            source.path(),
            dest.path(),
            DefinitionPreparation::Foreign(Dialect::Claude),
            &IndexMap::new(),
            &RenameMap::new(),
            None,
        );

        let staged_skill = dest.path().join("skills/demo");
        assert!(staged_skill.join("helper.sh").exists());
        assert_eq!(
            fs::read_to_string(staged_skill.join("SKILL.md")).unwrap(),
            "---\nname: demo\n---\n# Body\n"
        );
        assert_eq!(
            hash::compute_hash(&staged_skill, ItemKind::Skill).unwrap(),
            hash::compute_hash(&skill, ItemKind::Skill).unwrap()
        );
    }

    #[test]
    fn claude_foreign_skill_lift_invalidates_staged_hash() {
        let source = TempDir::new().unwrap();
        let skill = source.path().join("skills/demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: d\ndisable-model-invocation: true\n---\n# Body\n",
        )
        .unwrap();

        let native = TempDir::new().unwrap();
        stage_source(
            source.path(),
            native.path(),
            DefinitionPreparation::Schema1,
            &IndexMap::new(),
            &RenameMap::new(),
            None,
        );

        let claude = TempDir::new().unwrap();
        stage_source(
            source.path(),
            claude.path(),
            DefinitionPreparation::Foreign(Dialect::Claude),
            &IndexMap::new(),
            &RenameMap::new(),
            None,
        );

        let native_staged = fs::read_to_string(native.path().join("skills/demo/SKILL.md")).unwrap();
        let claude_staged = fs::read_to_string(claude.path().join("skills/demo/SKILL.md")).unwrap();
        assert!(native_staged.contains("disable-model-invocation"));
        assert!(!claude_staged.contains("disable-model-invocation"));
        assert!(claude_staged.contains("model-invocable: false"));

        let native_hash =
            hash::compute_hash(&native.path().join("skills/demo"), ItemKind::Skill).unwrap();
        let claude_hash =
            hash::compute_hash(&claude.path().join("skills/demo"), ItemKind::Skill).unwrap();
        assert_ne!(native_hash, claude_hash);
    }

    #[test]
    fn stage_skill_overlay_keys_by_installed_name_after_rename() {
        let source = TempDir::new().unwrap();
        let skill = source.path().join("skills/planning");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: planning\ndescription: base\n---\n# Planning\n",
        )
        .unwrap();

        let mut renames = RenameMap::new();
        renames.insert(
            ItemName::from("planning"),
            ItemName::from("research-planning"),
        );

        let mut overrides = IndexMap::new();
        overrides.insert(
            "research-planning".to_string(),
            SkillOverlay {
                description: Some("Renamed overlay".to_string()),
                ..SkillOverlay::default()
            },
        );

        let dest = TempDir::new().unwrap();
        stage_source(
            source.path(),
            dest.path(),
            DefinitionPreparation::Foreign(Dialect::Claude),
            &overrides,
            &renames,
            None,
        );

        let staged = fs::read_to_string(dest.path().join("skills/planning/SKILL.md")).unwrap();
        assert!(staged.contains("description: Renamed overlay"));
    }

    #[test]
    fn stage_flat_skill_overlay_uses_fallback_name() {
        let source = TempDir::new().unwrap();
        fs::write(
            source.path().join("SKILL.md"),
            "---\nname: base\ndescription: base\n---\n# Flat\n",
        )
        .unwrap();

        let mut overrides = IndexMap::new();
        overrides.insert(
            "my-flat-skill".to_string(),
            SkillOverlay {
                description: Some("Flat overlay".to_string()),
                ..SkillOverlay::default()
            },
        );

        let dest = TempDir::new().unwrap();
        stage_source(
            source.path(),
            dest.path(),
            DefinitionPreparation::Foreign(Dialect::Claude),
            &overrides,
            &RenameMap::new(),
            Some("my-flat-skill"),
        );

        let staged = fs::read_to_string(dest.path().join("SKILL.md")).unwrap();
        assert!(staged.contains("description: Flat overlay"));
    }

    #[test]
    fn local_bare_skill_keeps_foreign_key_for_mars_native_validation() {
        let root = TempDir::new().unwrap();
        let mars_src = root.path().join(".mars-src");
        let skill = mars_src.join("skills/demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: d\ndisable-model-invocation: true\n---\n# Body\n",
        )
        .unwrap();

        assert_eq!(Dialect::resolve_local(None, &mars_src), Dialect::MarsNative);

        let mut diag = DiagnosticCollector::new();
        let staged = stage_local_item(
            &skill,
            ItemKind::Skill,
            DefinitionPreparation::Schema1,
            &IndexMap::new(),
            &root.path().join("staging"),
            "skill:demo",
            Some("demo"),
            &[],
            &mut diag,
        )
        .unwrap();

        let content = fs::read_to_string(staged.join("SKILL.md")).unwrap();
        assert!(content.contains("disable-model-invocation"));

        let mut diags = Vec::new();
        mars_schema::skill::parse_skill_content(&content, &mut diags).unwrap();
        assert!(diags.iter().any(|d| d.is_error()));
    }

    #[test]
    fn local_claude_container_infers_claude_and_lifts_foreign_keys() {
        let root = TempDir::new().unwrap();
        let mars_src = root.path().join(".mars-src");
        let skill = mars_src.join(".claude/skills/demo");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: demo\ndescription: d\ndisable-model-invocation: true\n---\n# Body\n",
        )
        .unwrap();

        assert_eq!(Dialect::resolve_local(None, &mars_src), Dialect::Claude);

        let mut diag = DiagnosticCollector::new();
        let staged = stage_local_item(
            &skill,
            ItemKind::Skill,
            DefinitionPreparation::Foreign(Dialect::Claude),
            &IndexMap::new(),
            &root.path().join("staging"),
            "skill:demo",
            Some("demo"),
            &[],
            &mut diag,
        )
        .unwrap();

        let content = fs::read_to_string(staged.join("SKILL.md")).unwrap();
        assert!(!content.contains("disable-model-invocation"));
        assert!(content.contains("model-invocable: false"));
    }
}
