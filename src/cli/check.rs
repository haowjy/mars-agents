//! `mars check [PATH]` — validate a source package before publishing.
//!
//! Scans a directory as a mars source package
//! (`agents/*.md`, `skills/*/SKILL.md`, or a flat root `SKILL.md`) and reports what is wrong with
//! it. What its files say is `mars_schema::check::check`'s, which `@meridian-flow/mars-schema`
//! exports too: a rule about a package's files goes there, not here. This command adds what needs
//! a machine: fetching `[dependencies]`, the `requires-mars` floor, hooks, plugin output, model
//! aliases against `mars.lock` and the harness field-loss preview. No config or lock file needed.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::MarsError;
use crate::package_prepare::{DiskTreeSource, FileFindings, source_label};

use mars_schema::check::Check;
use mars_schema::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use mars_schema::item::DefinitionKind;
use mars_schema::keys;
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

/// A finding in a file of the package, as `--json` lists it under `diagnostics`.
#[derive(Debug, Serialize)]
struct FileFinding {
    source_path: String,
    /// The dependency whose Agent it is, for `skill-ref.dependency`.
    #[serde(skip_serializing_if = "Option::is_none")]
    dependency: Option<String>,
    code: String,
    severity: Severity,
    path: Vec<PathSegment>,
    message: String,
    /// What `mars check` prints, with the code and the file. `None` for a finding the
    /// diagnostic collector prints (it merges one unknown key across files).
    #[serde(skip)]
    line: Option<String>,
}

#[derive(Debug)]
pub(crate) struct CheckReport {
    agents: usize,
    skills: usize,
    /// What `mars_schema::check::check` found, in its order.
    findings: Vec<FileFinding>,
    /// What only the binary can check.
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl CheckReport {
    /// Every error: the files' first, then the binary's.
    pub(crate) fn errors(&self) -> Vec<&str> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
            .filter_map(|finding| finding.line.as_deref())
            .chain(self.errors.iter().map(String::as_str))
            .collect()
    }
}

#[derive(Serialize)]
struct JsonReport<'a> {
    agents: usize,
    skills: usize,
    errors: Vec<&'a str>,
    warnings: Vec<String>,
    diagnostics: &'a [FileFinding],
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
    let (lens, prepared, mut report) = examine(&authored_base, &mut publish_diag)?;
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
    let hooks = crate::compiler::hooks::check_hooks(&authored_base, &mut publish_diag)?;
    if let Some(manifest) = authored_manifest(&lens)?
        && let Some(floor) = package_floor(&manifest, &prepared, &hooks.portable)
    {
        report.errors.extend(requires_mars_shortfall(&lens, &floor));
    }
    report.errors.extend(hooks.errors);
    report.warnings.extend(hooks.warnings);
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
    let errors = report.errors();
    let clean = errors.is_empty();

    if json {
        let mut warnings = report.warnings.clone();
        warnings.extend(diagnostics.iter().map(ToString::to_string));
        output::print_json(&JsonReport {
            agents: report.agents,
            skills: report.skills,
            errors,
            warnings,
            diagnostics: &report.findings,
        });
    } else {
        println!("  {} agents, {} skills", report.agents, report.skills);
        if clean {
            println!(
                "  source package validates for .mars/ canonical store and native harness targets"
            );
        }
        println!();

        for finding in &report.findings {
            let Some(line) = &finding.line else { continue };
            match finding.severity {
                Severity::Error => output::print_error(line),
                Severity::Warning => output::print_warn(line),
                Severity::Info => output::print_note(line),
            }
        }
        for e in &report.errors {
            output::print_error(e);
        }
        for w in &report.warnings {
            output::print_warn(w);
        }
        output::print_diagnostics(&diagnostics, true);
        if !clean {
            println!();
            println!("  {} error(s) found", errors.len());
        } else if report.warnings.is_empty()
            && !report
                .findings
                .iter()
                .any(|finding| finding.severity == Severity::Warning)
            && !diagnostics.iter().any(|diag| diag.shown(false))
        {
            output::print_success("all checks passed");
        }
    }

    if clean { Ok(0) } else { Ok(1) }
}

/// The Mars the package needs, from the keys it uses. `None` for a tree with no `[package]`:
/// `requires-mars` is a `[package]` key. `manifest` is the `mars.toml` text, as written for
/// `mars check`.
pub(crate) fn package_floor(
    manifest: &str,
    prepared: &mars_schema::prepare::Prepared,
    portable_hooks: &[String],
) -> Option<keys::Floor> {
    let manifest: toml::Table = manifest.parse().ok()?;
    manifest.get("package")?;
    let mut uses = keys::used(&manifest, keys::Document::Package);
    // Only a schema 1 package states its tool rules in a form an older Mars can misread.
    if uses.iter().any(|used| used.key == "schema") {
        for definition in &prepared.definitions {
            let Reading::Agent(reading) = &definition.reading else {
                continue;
            };
            for rule in reading.fields.iter().flat_map(|agent| &agent.tools.rules) {
                uses.push(keys::Used::profile_rule(
                    &definition.slug,
                    &rule.render(),
                    rule.action.as_str(),
                ));
            }
        }
    }
    uses.extend(portable_hooks.iter().map(|name| keys::Used::hook(name)));
    keys::floor(uses)
}

/// What `mars version` writes into `requires-mars`: the floor, when the package states less.
pub(crate) fn requires_mars_to_write(base: &Path) -> Result<Option<keys::Floor>, MarsError> {
    let mut diag = crate::diagnostic::DiagnosticCollector::new();
    let (lens, prepared) =
        crate::resolve::PublishLens::load(base, FileFindings::Leave, &mut diag)?;
    let hooks = crate::compiler::hooks::check_hooks(base, &mut diag)?;
    let Some(manifest) = authored_manifest(&lens)? else {
        return Ok(None);
    };
    Ok(package_floor(&manifest, &prepared, &hooks.portable)
        .filter(|floor| requires_mars_shortfall(&lens, floor).is_some()))
}

fn authored_manifest(lens: &crate::resolve::PublishLens) -> Result<Option<String>, MarsError> {
    if lens.manifest.is_none() {
        return Ok(None);
    }
    Ok(Some(std::fs::read_to_string(
        lens.package_root().join("mars.toml"),
    )?))
}

fn requires_mars_shortfall(
    lens: &crate::resolve::PublishLens,
    floor: &keys::Floor,
) -> Option<String> {
    let package = lens.manifest.as_ref()?.package.as_ref()?;
    mars_schema::package::requires_mars_shortfall(package.requires_mars.as_deref(), floor)
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

/// Prepare the package, fetch its dependencies when it has any, and ask
/// `mars_schema::check::check` what is wrong with it.
fn examine(
    base: &Path,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> Result<
    (
        crate::resolve::PublishLens,
        mars_schema::prepare::Prepared,
        CheckReport,
    ),
    MarsError,
> {
    let (lens, prepared) = crate::resolve::PublishLens::load(base, FileFindings::Leave, diag)?;
    let mut errors = Vec::new();

    // Both are required to fetch: `[package]` makes this a publishable package, and
    // `[dependencies]` are the packages a skill could come from. Uses only `[dependencies]`: the
    // lens drops `[local-dependencies]` and `mars.local.toml`.
    let has_dependencies = lens
        .manifest
        .as_ref()
        .is_some_and(|manifest| manifest.package.is_some() && !manifest.dependencies.is_empty());
    let mut dependencies = std::collections::BTreeMap::new();
    let mut fetched = false;
    if has_dependencies {
        let options = crate::resolve::ResolveOptions::default()
            .with_root(std::sync::Arc::new(prepared.clone()));
        match crate::resolve::resolve_publish_graph_from_config(&lens, &lens.effective, &options, diag)
        {
            Ok(graph) => {
                fetched = true;
                dependencies = graph
                    .nodes
                    .iter()
                    .map(|(name, node)| {
                        (name.to_string(), crate::resolve::portable_facts(&node.prepared))
                    })
                    .collect();
            }
            Err(error) => errors.push(format!(
                "dependency graph resolution failed: {error}\n  hint: check network access, or use `mars version --force` to bypass the publish gate"
            )),
        }
    }

    let source = DiskTreeSource::new(base)?;
    let mut root = prepared.clone();
    root.selection = crate::resolve::portable_facts(&prepared);
    let result = mars_schema::check::check(&source, &root, &dependencies);
    if let Some(error) = source.take_error() {
        return Err(error);
    }
    // The resolver fetched the whole graph, so a package the check lacks means the two disagree
    // about it, and `mars check` never passes with a ref unjudged.
    if fetched {
        errors.extend(result.missing_dependencies.iter().map(|missing| {
            format!(
                "dependency `{}` was not in the resolved graph, so skill references were not checked",
                missing.name
            )
        }));
    }
    let readable = !matches!(
        prepared.classification,
        Classification::Invalid | Classification::SchemaNewer { .. }
    );
    if readable && prepared.definitions.is_empty() {
        errors.push("no agents or skills found — is this a mars source package?".to_string());
    }

    let count = |kind| {
        prepared
            .definitions
            .iter()
            .filter(|definition| definition.kind == kind)
            .count()
    };
    let report = CheckReport {
        agents: count(DefinitionKind::Agent),
        skills: count(DefinitionKind::Skill),
        findings: file_findings(&result, base, diag),
        errors,
        warnings: Vec::new(),
    };
    Ok((lens, prepared, report))
}

/// The findings of `check`, each named by its file. A dependency's own Agent is named by the
/// dependency.
fn file_findings(
    result: &Check,
    base: &Path,
    diag: &mut crate::diagnostic::DiagnosticCollector,
) -> Vec<FileFinding> {
    let label = source_label(base);
    let mut findings = Vec::new();
    let mut add = |source_path: &str,
                   dependency: Option<&str>,
                   label: String,
                   diagnostic: &SchemaDiagnostic| {
        // One note per unknown key, across the files that carry it.
        let line = if diagnostic.code == "field.unknown" {
            diag.schema(diagnostic, &label, false);
            None
        } else {
            Some(format!("[{}] {label}: {}", diagnostic.code, diagnostic.message))
        };
        findings.push(FileFinding {
            source_path: source_path.to_owned(),
            dependency: dependency.map(str::to_owned),
            code: diagnostic.code.clone(),
            severity: diagnostic.severity,
            path: diagnostic.path.clone(),
            message: diagnostic.message.clone(),
            line,
        });
    };
    for (path, diagnostics) in &result.diagnostics {
        for diagnostic in diagnostics {
            add(path, None, label(path), diagnostic);
        }
    }
    for (dependency, files) in &result.dependency_diagnostics {
        for (path, diagnostics) in files {
            for diagnostic in diagnostics {
                add(
                    path,
                    Some(dependency),
                    format!("dependency `{dependency}` {path}"),
                    diagnostic,
                );
            }
        }
    }
    findings
}

pub(crate) fn check_dir(base: &Path) -> Result<CheckReport, MarsError> {
    let mut diag = crate::diagnostic::DiagnosticCollector::new();
    let (_, _, mut report) = examine(base, &mut diag)?;
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use mars_schema::diagnostic::Severity;
    use tempfile::TempDir;

    fn has_finding(report: &super::CheckReport, code: &str, severity: Severity) -> bool {
        report
            .findings
            .iter()
            .any(|finding| finding.code == code && finding.severity == severity)
    }

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
            report.errors().is_empty(),
            "expected no errors in local-only mode: {:?}",
            report.errors
        );
        assert!(
            has_finding(&report, "skill-ref.external", Severity::Warning),
            "expected warning for missing-skill: {:?}",
            report.findings
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

        let joined = report.errors().join("\n");
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
            "[package]\nschema = 1\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep = { path = \"/nonexistent-mars-dep-xyz-abc\" }\n",
        )
        .unwrap();

        let report = super::check_dir(dir.path()).unwrap();
        assert!(
            !report.errors().is_empty(),
            "expected errors when dep cannot be resolved"
        );
        let joined = report.errors().join("\n");
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
                "[package]\nschema = 1\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep = {{ path = \"{}\" }}\n",
                toml_path(dep_dir.path())
            ),
        )
        .unwrap();

        let report = super::check_dir(dir.path()).unwrap();
        assert!(
            !report.errors().is_empty(),
            "expected error for missing skill, got: {:?}",
            report.errors
        );
        let joined = report.errors().join("\n");
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
        assert!(
            !has_finding(&report, "skill-ref.external", Severity::Warning),
            "missing skill must be error, not warning: {:?}",
            report.findings
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
                "[package]\nschema = 1\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep = {{ path = \"{}\" }}\n",
                toml_path(dep_dir.path())
            ),
        )
        .unwrap();

        let report = super::check_dir(dir.path()).unwrap();
        assert!(
            report.errors().is_empty(),
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
                "[package]\nschema = 1\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\n\n[local-dependencies]\nlocal-dep = {{ path = \"{}\" }}\n",
                toml_path(local_dep_dir.path())
            ),
        )
        .unwrap();

        let report = super::check_dir(dir.path()).unwrap();
        assert!(
            has_finding(&report, "skill-ref.external", Severity::Warning),
            "local-skill from [local-dependencies] must not satisfy ref in publish gate — expected warning: {:?}",
            report.findings
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
                "[package]\nschema = 1\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\nregular = {{ path = \"{}\" }}\n\n[local-dependencies]\nlocal = {{ path = \"{}\" }}\n",
                toml_path(regular_dep_dir.path()),
                toml_path(local_dep_dir.path())
            ),
        )
        .unwrap();

        let report = super::check_dir(dir.path()).unwrap();
        assert!(
            !report.errors().is_empty(),
            "skill from [local-dependencies] must not satisfy ref in publish gate — expected error: {:?}",
            report.errors
        );
        let joined = report.errors().join("\n");
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
            "[package]\nschema = 1\nname = \"test-pkg\"\nversion = \"0.1.0\"\n\n[dependencies]\ndep = { path = \"C:\\Users\\dev\\dep\" }\n",
        )
        .unwrap();

        let error = super::check_dir(dir.path()).unwrap_err().to_string();
        assert!(
            error.contains("TOML parse error"),
            "an invalid mars.toml must fail the check, not fall back to local-only warnings: {error}"
        );
    }

    #[test]
    fn check_reports_each_file_finding_once() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("mars.toml"),
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\nschema = 1\n",
        )
        .unwrap();
        write_agent_content(
            dir.path(),
            "a",
            "---\nname: dup\ndescription: first\napproval: nonsense\n---\n# A",
        );
        write_agent_content(
            dir.path(),
            "b",
            "---\nname: dup\ndescription: second\n---\n# B",
        );

        let report = super::check_dir(dir.path()).unwrap();

        let errors = report.errors();
        assert_eq!(
            errors
                .iter()
                .filter(|error| error.starts_with("[field.value]"))
                .count(),
            1,
            "a `prepare` error prints once: {errors:?}"
        );
        assert_eq!(
            errors
                .iter()
                .filter(|error| error.starts_with("[definition.duplicate-name]"))
                .count(),
            2,
            "a duplicate name prints once per file: {errors:?}"
        );
    }
}
