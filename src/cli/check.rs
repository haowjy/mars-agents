//! `mars check [PATH]` — validate a source package before publishing.
//!
//! Scans a directory as a mars source package
//! (`agents/*.md`, `skills/*/SKILL.md`, or a flat root `SKILL.md`)
//! and validates structure, frontmatter, and internal skill dependencies.
//! No config or lock file needed — works on raw source directories. A `mars.lock`, when
//! present, supplies dependency model aliases for the model-policy check.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::MarsError;

use mars_schema::keys::{self, Surface};
use mars_schema::prepare::Classification;
use mars_schema::read::Reading;

use super::output;

/// Arguments for `mars check`.
#[derive(Debug, clap::Args)]
pub struct CheckArgs {
    /// Directory to validate as a source package (default: current directory).
    pub path: Option<PathBuf>,

    /// Show per-item detail for every lossy field: dropped restrictions, approximate mappings, quiet ones, and launch-time fields handled by meridian at spawn.
    #[arg(long)]
    pub verbose: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct CheckReport {
    agents: usize,
    skills: usize,
    pub(crate) errors: Vec<String>,
    warnings: Vec<String>,
}

/// Run `mars check`. `PATH` names the directory to check; without it, `--root` does.
///
/// `PATH` wins when both are given, because a host such as Meridian appends `--root <project>`
/// to every command it forwards, and `meridian mars check <package>` must check the package.
pub fn run(args: &CheckArgs, root: Option<&Path>, json: bool) -> Result<i32, MarsError> {
    let authored_base = match args.path.as_deref().or(root) {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => std::env::current_dir()?.join(path),
        None => std::env::current_dir()?,
    };

    if !authored_base.is_dir() {
        return Err(MarsError::Config(crate::error::ConfigError::Invalid {
            message: format!("{} is not a directory", authored_base.display()),
        }));
    }

    let mut publish_diag = crate::diagnostic::DiagnosticCollector::new();
    let (lens, prepared) = crate::resolve::PublishLens::load(&authored_base, &mut publish_diag)?;
    let mut report = check_prepared(&prepared, &authored_base, &lens, &mut publish_diag)?;
    let lock = crate::lock::load_for_runtime_aliases(&authored_base)?;
    let known_aliases = crate::models::merged_runtime_aliases(
        &lock.dependency_model_aliases,
        lens.manifest.as_ref().map(|config| &config.models),
    );
    let unsynced = lock.dependencies.is_empty()
        && lens
            .manifest
            .as_ref()
            .is_some_and(|config| !config.dependencies.is_empty());
    let unconsulted = if unsynced {
        " (mars.lock records no dependencies, so their catalogs were not consulted; run `mars sync` first)"
    } else {
        ""
    };
    for definition in &prepared.definitions {
        let Reading::Agent(reading) = &definition.reading else {
            continue;
        };
        let Some(profile) = reading.fields.as_ref() else {
            continue;
        };
        for policy in &profile.model_policies {
            if policy.match_type == mars_schema::model_policy::ModelPolicyMatchType::Alias
                && !known_aliases.contains_key(&policy.match_value)
            {
                report.warnings.push(format!(
                    "{}: agent `{}` has a model policy for alias `{}`, which is not a catalog alias here{unconsulted}",
                    definition.source_path, definition.slug, policy.match_value
                ));
            }
        }
    }
    report
        .warnings
        .extend(requires_mars_warnings(&prepared, &lens));
    let requires_mars = lens
        .manifest
        .as_ref()
        .and_then(|config| config.package.as_ref())
        .map(|package| package.requires_mars.as_deref());
    let (hook_errors, hook_warnings) =
        crate::compiler::hooks::check_hooks(&authored_base, requires_mars, &mut publish_diag)?;
    report.errors.extend(hook_errors);
    report.warnings.extend(hook_warnings);
    report
        .warnings
        .extend(plugin_warnings(&authored_base, &lens, &mut publish_diag)?);
    let mut diagnostics = publish_diag.drain();
    diagnostics.extend(lossiness_diagnostics_for_check(
        &lens,
        &prepared,
        &authored_base,
        args.verbose,
    )?);
    let mut surfaced = Vec::new();
    for diagnostic in diagnostics {
        if diagnostic.level == crate::diagnostic::DiagnosticLevel::Error {
            report
                .errors
                .push(format!("[{}] {}", diagnostic.code, diagnostic.message));
        } else {
            surfaced.push(diagnostic);
        }
    }
    let diagnostics = surfaced;
    let clean = report.errors.is_empty();

    if json {
        let mut json_report = report;
        for diag in diagnostics
            .iter()
            .filter(|diag| diag.level != crate::diagnostic::DiagnosticLevel::Info)
        {
            json_report.warnings.push(diag.to_string());
        }
        output::print_json(&json_report);
    } else {
        println!("  {} agents, {} skills", report.agents, report.skills);
        if clean {
            println!(
                "  source package validates for .mars/ canonical store and native harness targets"
            );
        }
        println!();

        let has_non_info_diagnostics = diagnostics
            .iter()
            .any(|diag| diag.level != crate::diagnostic::DiagnosticLevel::Info);
        if clean && report.warnings.is_empty() && !has_non_info_diagnostics {
            output::print_diagnostics(&diagnostics);
            output::print_success("all checks passed");
        } else {
            for e in &report.errors {
                output::print_error(e);
            }
            for w in &report.warnings {
                output::print_warn(w);
            }
            output::print_diagnostics(&diagnostics);
            if !clean {
                println!();
                println!("  {} error(s) found", report.errors.len());
            }
        }
    }

    if clean { Ok(0) } else { Ok(1) }
}

fn requires_mars_warnings(
    prepared: &mars_schema::prepare::Prepared,
    lens: &crate::resolve::PublishLens,
) -> Vec<String> {
    let Some(config) = lens.manifest.as_ref() else {
        return Vec::new();
    };
    let Some(package) = config.package.as_ref() else {
        return Vec::new();
    };
    if crate::resolve::validate_package_requirement_syntax(package).is_err() {
        return Vec::new();
    }
    let requires = package.requires_mars.as_deref();
    let schema_1 = (prepared.classification == Classification::Schema1)
        .then(|| mars_schema::package::schema_1_requires_mars_warning(requires))
        .flatten();
    let plugin = (prepared.classification != Classification::Schema1 && config.plugin.is_some())
        .then(|| {
            mars_schema::package::requires_mars_floor_warning(
                requires,
                keys::since(Surface::Manifest, "plugin"),
                "older Mars versions may drop [plugin] during `mars version`",
            )
        })
        .flatten();
    let tools = (!package.tools.is_empty())
        .then(|| {
            mars_schema::package::requires_mars_floor_warning(
                requires,
                keys::since(Surface::Package, "tools"),
                "an older Mars installs this package's agents without the `[package.tools]` rules, with only a warning",
            )
        })
        .flatten();
    [schema_1, plugin, tools].into_iter().flatten().collect()
}

fn plugin_warnings(
    base: &Path,
    lens: &crate::resolve::PublishLens,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> Result<Vec<String>, MarsError> {
    let Some(config) = lens.manifest.as_ref() else {
        return Ok(Vec::new());
    };
    if config.plugin.is_none() {
        return Ok(Vec::new());
    }
    let mut warnings = Vec::new();
    let metadata = match crate::build::plugin::validate_plugin_build(config) {
        Ok(metadata) => Some(metadata),
        Err(error) => {
            warnings.push(error.to_string());
            None
        }
    };

    let Some(metadata) = metadata else {
        return Ok(warnings);
    };
    let state = crate::build::plugin::inspect_validated(base, &metadata)?;
    if let Some(message) = state.directory.message() {
        if matches!(
            state.directory,
            crate::build::plugin::PluginDirectoryState::Missing { .. }
        ) {
            diag.info("plugin-output-missing", message);
        } else {
            warnings.push(message);
        }
    }
    warnings.extend(state.inputs.iter().filter_map(|state| state.message()));
    warnings.extend(state.builder.iter().filter_map(|state| state.message()));
    warnings.extend(state.version.iter().filter_map(|state| state.message()));
    warnings.extend(state.root_files.iter().filter_map(|state| state.message()));

    Ok(warnings)
}

fn lossiness_diagnostics_for_check(
    lens: &crate::resolve::PublishLens,
    prepared: &mars_schema::prepare::Prepared,
    authored_root: &Path,
    verbose: bool,
) -> Result<Vec<crate::diagnostic::Diagnostic>, MarsError> {
    use crate::diagnostic::LossinessMode;

    let mode = if verbose {
        LossinessMode::Verbose
    } else {
        LossinessMode::Surface
    };
    crate::compiler::lossiness_preview::collect_prepared_source_lossiness_diagnostics(
        lens,
        prepared,
        authored_root,
        mode,
        "mars check --verbose",
    )
}

fn check_prepared(
    prepared: &mars_schema::prepare::Prepared,
    authored_root: &Path,
    lens: &crate::resolve::PublishLens,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> Result<CheckReport, MarsError> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if let Some(package) = lens
        .manifest
        .as_ref()
        .and_then(|config| config.package.as_ref())
        && let Err(error) = crate::resolve::validate_package_requirement_syntax(package)
    {
        errors.push(error.to_string());
    }

    let mut agent_names = HashMap::<String, PathBuf>::new();
    let mut skill_names = HashMap::<String, PathBuf>::new();
    for definition in &prepared.definitions {
        let path = authored_root.join(&definition.source_path);
        let linked_item = match definition.kind {
            mars_schema::item::DefinitionKind::Agent => path
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink()),
            mars_schema::item::DefinitionKind::Skill => path.parent().is_some_and(|root| {
                root.symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
            }),
        };
        if linked_item {
            warnings.push(format!(
                "skipping symlinked {} `{}` — source packages should not contain symlinks",
                definition.kind, definition.slug
            ));
            continue;
        }
        match &definition.reading {
            Reading::Agent(reading) => {
                let filename = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned();
                if let Some(profile) = &reading.fields {
                    let name = profile.name.clone().unwrap_or_else(|| filename.clone());
                    if profile.name.is_none() {
                        warnings.push(format!("agent `{filename}` has no `name` in frontmatter"));
                    }
                    if profile.description.is_none() {
                        warnings.push(format!("agent `{name}` has no `description`"));
                    }
                    if profile.name.is_some() && name != filename {
                        warnings.push(format!(
                            "agent filename `{filename}.md` doesn't match name `{name}` in frontmatter"
                        ));
                    }
                    if let Some(existing) = agent_names.insert(name.clone(), path.clone()) {
                        errors.push(format!(
                            "duplicate agent name `{name}` in {} and {}",
                            existing.display(),
                            path.display()
                        ));
                    }
                }
            }
            Reading::Skill(reading) => {
                let dirname = if definition.source_path == "SKILL.md" {
                    definition.slug.clone()
                } else {
                    path.parent()
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        .unwrap_or(&definition.slug)
                        .to_owned()
                };
                if let Some(profile) = &reading.fields {
                    let name = profile.name.clone().unwrap_or_else(|| dirname.clone());
                    if profile.name.is_some() && name != dirname {
                        warnings.push(format!(
                            "skill dirname `{dirname}` doesn't match name `{name}` in frontmatter"
                        ));
                    }
                    if let Some(existing) = skill_names.insert(name.clone(), path.clone()) {
                        errors.push(format!(
                            "duplicate skill name `{name}` in {} and {}",
                            existing.display(),
                            path.display()
                        ));
                    }
                }
            }
            Reading::Manifest(_) => {}
        }
    }

    let readable = !matches!(
        prepared.classification,
        Classification::Invalid | Classification::SchemaNewer { .. }
    );
    if readable && agent_names.is_empty() && skill_names.is_empty() {
        errors.push("no agents or skills found — is this a mars source package?".to_string());
    }
    check_skill_refs(prepared, lens, &mut errors, &mut warnings, diag);
    Ok(CheckReport {
        agents: agent_names.len(),
        skills: skill_names.len(),
        errors,
        warnings,
    })
}

pub(crate) fn check_dir(base: &Path) -> Result<CheckReport, MarsError> {
    let mut diag = crate::diagnostic::DiagnosticCollector::new();
    let (lens, prepared) = crate::resolve::PublishLens::load(base, &mut diag)?;
    let mut report = check_prepared(&prepared, base, &lens, &mut diag)?;
    for diagnostic in diag.drain() {
        let rendered = format!("[{}] {}", diagnostic.code, diagnostic.message);
        if diagnostic.level == crate::diagnostic::DiagnosticLevel::Error {
            report.errors.push(rendered);
        } else if diagnostic.level == crate::diagnostic::DiagnosticLevel::Warning {
            report.warnings.push(rendered);
        }
    }
    Ok(report)
}

///
/// Both are required to trigger graph-backed validation: `[package]` indicates
/// this is a publishable source package, and `[dependencies]` means there are
/// skills that could come from external packages.
fn has_package_dependencies(lens: &crate::resolve::PublishLens) -> bool {
    lens.manifest
        .as_ref()
        .is_some_and(|manifest| manifest.package.is_some() && !manifest.dependencies.is_empty())
}

/// Check answers with install's rule: the package is the root of one selection over its resolved
/// `[dependencies]`, selected whole, and its Agents' refs resolve through the package, its
/// dependencies and the rest of the graph. It judges a ref by the package alone and ignores
/// `.mars/provided.lock`, so a package passes or fails the same on every machine. A package with
/// `[dependencies]` fails closed; a package without them cannot know what the consumer adds, so an
/// unknown skill is a warning.
fn check_skill_refs(
    prepared: &mars_schema::prepare::Prepared,
    lens: &crate::resolve::PublishLens,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) {
    let has_dependencies = has_package_dependencies(lens);
    // Uses only `[dependencies]`: the lens drops `[local-dependencies]` and `mars.local.toml`.
    // Without a `[package]` they are not a publishable package's, and nothing is fetched.
    let mut effective = lens.effective.clone();
    if !has_dependencies {
        effective.dependencies.clear();
    }
    let options =
        crate::resolve::ResolveOptions::default().with_root(std::sync::Arc::new(prepared.clone()));
    let graph = match crate::resolve::resolve_publish_graph_from_config(
        lens, &effective, &options, diag,
    ) {
        Ok(graph) => graph,
        Err(error) => {
            errors.push(format!(
                "dependency graph resolution failed: {error}\n  hint: check network access, or use `mars version --force` to bypass the publish gate"
            ));
            return;
        }
    };
    let has_package = lens
        .manifest
        .as_ref()
        .is_some_and(|config| config.package.is_some());
    for missing in &graph.selection.missing_skills {
        let skill = &missing.skill;
        // An Agent of a dependency is that package's to fix, and the author's to work around.
        let agent = match &missing.package {
            Some(package) => format!("agent `{}` of dependency `{package}`", missing.agent),
            None => format!("agent `{}`", missing.agent),
        };
        let remedy = match &missing.package {
            Some(package) => {
                format!("use a version of `{package}` that ships it, or add a dependency that does")
            }
            None => "add the skill's source package as a dependency, or remove the skill reference"
                .to_string(),
        };
        match &missing.hint {
            Some(hint) if has_package => errors.push(format!(
                "{agent} references skill `{skill}`, which is not in this package or its dependencies; {hint}"
            )),
            _ if has_dependencies => errors.push(format!(
                "{agent} references skill `{skill}` not found in local package or dependencies\n  searched: {}\n  hint: {remedy}",
                format_searched_packages(&missing.searched, &graph)
            )),
            _ => warnings.push(format!(
                "external dependency: `{skill}` (referenced by: {})",
                missing.agent
            )),
        }
    }
}

fn format_searched_packages(searched: &[String], graph: &crate::resolve::ResolvedGraph) -> String {
    std::iter::once("(this package)".to_string())
        .chain(searched.iter().map(|name| {
            let version = graph
                .nodes
                .get(name.as_str())
                .and_then(|node| node.resolved_ref.version.as_ref());
            match version {
                Some(version) => format!("{name}@{version}"),
                None => name.clone(),
            }
        }))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    fn write_agent(path: &Path, filename: &str, skills: &[&str]) {
        let agents = path.join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        let skills_str = skills.join(", ");
        std::fs::write(
            agents.join(format!("{filename}.md")),
            format!(
                "---\nname: {filename}\ndescription: test agent\nskills: [{skills_str}]\n---\n# Agent"
            ),
        )
        .unwrap();
    }

    fn write_agent_content(path: &Path, filename: &str, content: &str) {
        let agents = path.join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join(format!("{filename}.md")), content).unwrap();
    }

    fn write_dep_package(path: &Path, name: &str, version: &str, skills: &[&str]) {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(
            path.join("mars.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nschema = 1\n\n[dependencies]\n"),
        )
        .unwrap();
        for skill_name in skills {
            let skill_dir = path.join("skills").join(skill_name);
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {skill_name}\ndescription: test skill\n---\n# Skill"),
            )
            .unwrap();
        }
    }

    fn toml_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    #[cfg(unix)]
    #[test]
    fn check_skips_symlinked_agent() {
        let dir = TempDir::new().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();

        std::fs::write(
            agents.join("real.md"),
            "---\nname: real\ndescription: real agent\n---\n# Real",
        )
        .unwrap();
        std::os::unix::fs::symlink(agents.join("real.md"), agents.join("linked.md")).unwrap();

        let args = super::CheckArgs {
            path: Some(dir.path().to_path_buf()),
            verbose: false,
        };
        let code = super::run(&args, None, true).unwrap();
        assert_eq!(code, 0);
    }

    #[cfg(unix)]
    #[test]
    fn check_skips_symlinked_skill() {
        let dir = TempDir::new().unwrap();
        let skills = dir.path().join("skills");
        let real_skill = skills.join("real-skill");
        std::fs::create_dir_all(&real_skill).unwrap();
        std::fs::write(
            real_skill.join("SKILL.md"),
            "---\nname: real-skill\ndescription: a skill\n---\n# Skill",
        )
        .unwrap();
        std::os::unix::fs::symlink(&real_skill, skills.join("linked-skill")).unwrap();

        let agents = dir.path().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("coder.md"),
            "---\nname: coder\ndescription: agent\n---\n# Coder",
        )
        .unwrap();

        let args = super::CheckArgs {
            path: Some(dir.path().to_path_buf()),
            verbose: false,
        };
        let code = super::run(&args, None, true).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn check_accepts_flat_skill_repo() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("SKILL.md"),
            "---\nname: flat-skill\ndescription: flat layout\n---\n# Flat skill",
        )
        .unwrap();

        let args = super::CheckArgs {
            path: Some(dir.path().to_path_buf()),
            verbose: false,
        };
        let code = super::run(&args, None, true).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn check_no_dependencies_warns_for_external_skill() {
        let dir = TempDir::new().unwrap();
        write_agent(dir.path(), "coder", &["missing-skill"]);

        let report = super::check_dir(dir.path()).unwrap();
        assert!(
            report.errors.is_empty(),
            "expected no errors in local-only mode: {:?}",
            report.errors
        );
        let has_warning = report
            .warnings
            .iter()
            .any(|w| w.contains("external dependency: `missing-skill`"));
        assert!(
            has_warning,
            "expected warning for missing-skill: {:?}",
            report.warnings
        );
    }

    #[test]
    fn check_errors_for_malformed_agent_model_policy() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\nschema = 1\n",
        )
        .unwrap();
        write_agent_content(
            dir.path(),
            "browser-tester",
            "---\nname: browser-tester\ndescription: browser test\nmodel-policies:\n  - match:\n      alias: gpt55\n      model: gpt-5.5\n---\n# Browser Tester",
        );

        let report = super::check_dir(dir.path()).unwrap();

        let joined = report.errors.join("\n");
        assert!(
            joined.contains("model-policies[0].match"),
            "expected model-policies match error: {joined}"
        );
    }

    #[test]
    fn check_with_unresolvable_dep_fails_closed_with_remediation_hint() {
        let dir = TempDir::new().unwrap();
        write_agent(dir.path(), "coder", &["some-skill"]);
        std::fs::write(
            dir.path().join("mars.toml"),
            "[package]\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep = { path = \"/nonexistent-mars-dep-xyz-abc\" }\n",
        )
        .unwrap();

        let report = super::check_dir(dir.path()).unwrap();
        assert!(
            !report.errors.is_empty(),
            "expected errors when dep cannot be resolved"
        );
        let joined = report.errors.join("\n");
        assert!(
            joined.contains("mars version --force"),
            "error must include remediation hint: {joined}"
        );
    }

    #[test]
    fn check_missing_skill_in_resolved_graph_is_error() {
        let dir = TempDir::new().unwrap();
        let dep_dir = TempDir::new().unwrap();

        write_dep_package(dep_dir.path(), "dep-pkg", "0.1.0", &["provided-skill"]);

        write_agent(dir.path(), "coder", &["missing-skill"]);
        std::fs::write(
            dir.path().join("mars.toml"),
            format!(
                "[package]\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep = {{ path = \"{}\" }}\n",
                toml_path(dep_dir.path())
            ),
        )
        .unwrap();

        let report = super::check_dir(dir.path()).unwrap();
        assert!(
            !report.errors.is_empty(),
            "expected error for missing skill, got: {:?}",
            report.errors
        );
        let joined = report.errors.join("\n");
        assert!(
            joined.contains("coder"),
            "error must name the agent: {joined}"
        );
        assert!(
            joined.contains("missing-skill"),
            "error must name the missing skill: {joined}"
        );
        assert!(
            joined.contains("searched:"),
            "error must list searched packages: {joined}"
        );
        assert!(
            joined.contains("hint:"),
            "error must include remediation guidance: {joined}"
        );
        let has_warning = report.warnings.iter().any(|w| w.contains("missing-skill"));
        assert!(
            !has_warning,
            "missing skill must be error, not warning: {:?}",
            report.warnings
        );
    }

    #[test]
    fn check_skill_provided_by_path_dep_passes() {
        let dir = TempDir::new().unwrap();
        let dep_dir = TempDir::new().unwrap();

        write_dep_package(dep_dir.path(), "dep-pkg", "0.1.0", &["ext-skill"]);
        write_agent(dir.path(), "coder", &["ext-skill"]);
        std::fs::write(
            dir.path().join("mars.toml"),
            format!(
                "[package]\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep = {{ path = \"{}\" }}\n",
                toml_path(dep_dir.path())
            ),
        )
        .unwrap();

        let report = super::check_dir(dir.path()).unwrap();
        assert!(
            report.errors.is_empty(),
            "expected no errors when skill is in dep: {:?}",
            report.errors
        );
    }

    #[test]
    fn check_local_dependency_skill_does_not_satisfy_ref() {
        let dir = TempDir::new().unwrap();
        let local_dep_dir = TempDir::new().unwrap();

        write_dep_package(local_dep_dir.path(), "local-dep", "0.1.0", &["local-skill"]);
        write_agent(dir.path(), "coder", &["local-skill"]);
        std::fs::write(
            dir.path().join("mars.toml"),
            format!(
                "[package]\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\n\n[local-dependencies]\nlocal-dep = {{ path = \"{}\" }}\n",
                toml_path(local_dep_dir.path())
            ),
        )
        .unwrap();

        let report = super::check_dir(dir.path()).unwrap();
        let has_warning = report.warnings.iter().any(|w| w.contains("local-skill"));
        assert!(
            has_warning,
            "local-skill from [local-dependencies] must not satisfy ref in publish gate — expected warning: {:?}",
            report.warnings
        );
    }

    #[test]
    fn check_local_dep_skill_not_available_when_regular_dep_present() {
        let dir = TempDir::new().unwrap();
        let regular_dep_dir = TempDir::new().unwrap();
        let local_dep_dir = TempDir::new().unwrap();

        write_dep_package(
            regular_dep_dir.path(),
            "regular-dep",
            "0.1.0",
            &["unrelated-skill"],
        );
        write_dep_package(
            local_dep_dir.path(),
            "local-dep",
            "0.1.0",
            &["local-only-skill"],
        );
        write_agent(dir.path(), "coder", &["local-only-skill"]);
        std::fs::write(
            dir.path().join("mars.toml"),
            format!(
                "[package]\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\nregular = {{ path = \"{}\" }}\n\n[local-dependencies]\nlocal = {{ path = \"{}\" }}\n",
                toml_path(regular_dep_dir.path()),
                toml_path(local_dep_dir.path())
            ),
        )
        .unwrap();

        let report = super::check_dir(dir.path()).unwrap();
        assert!(
            !report.errors.is_empty(),
            "skill from [local-dependencies] must not satisfy ref in publish gate — expected error: {:?}",
            report.errors
        );
        let joined = report.errors.join("\n");
        assert!(
            joined.contains("local-only-skill"),
            "error must name the missing skill: {joined}"
        );
    }

    #[test]
    fn check_invalid_config_reports_error_instead_of_falling_back_to_local_only() {
        let dir = TempDir::new().unwrap();
        write_agent(dir.path(), "coder", &["missing-skill"]);
        // Intentionally invalid TOML (Windows-style path escapes in basic string).
        std::fs::write(
            dir.path().join("mars.toml"),
            "[package]\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep = { path = \"C:\\Users\\dev\\dep\" }\n",
        )
        .unwrap();

        let error = super::check_dir(dir.path()).unwrap_err().to_string();
        assert!(
            error.contains("TOML parse error"),
            "an invalid mars.toml must fail the check, not fall back to local-only warnings: {error}"
        );
    }
}
