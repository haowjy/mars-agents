//! `mars add <dependency>` — add or update a dependency, then sync.

use crate::error::{ConfigError, MarsError};
use crate::source::parse;
use crate::sync::{
    ConfigMutation, DependencyUpsertChange, ResolutionMode, SyncOptions, SyncRequest,
};
use crate::types::SourceName;
use mars_schema::dependency::{Entry, Source};
use mars_schema::filter::FilterConfig;
use mars_schema::names::{ItemName, SourceSubpath};

use super::output;

/// Arguments for `mars add`.
#[derive(Debug, clap::Args)]
pub struct AddArgs {
    /// Source specifiers (one or more): owner/repo, owner/repo@version, URL, or local path.
    #[arg(required = true)]
    pub sources: Vec<String>,

    /// Root the fetched source at a package subdirectory.
    #[arg(long)]
    pub subpath: Option<String>,

    /// Install one plugin from the Claude marketplace at this source.
    #[arg(long)]
    pub plugin: Option<String>,

    /// Only install specific agents from this source.
    #[arg(long, value_delimiter = ',')]
    pub agents: Vec<String>,

    /// Only install specific skills from this source.
    #[arg(long, value_delimiter = ',')]
    pub skills: Vec<String>,

    /// Exclude specific items from this source.
    #[arg(long, value_delimiter = ',')]
    pub exclude: Vec<String>,

    /// Install only skills from this source (no agents).
    #[arg(long)]
    pub only_skills: bool,

    /// Install only agents (plus their transitive skill deps) from this source.
    #[arg(long)]
    pub only_agents: bool,

    /// Ignore package `requires-mars` version constraints.
    #[arg(long)]
    pub ignore_requires_mars: bool,

    /// Ignore package `requires-meridian` version constraints.
    #[arg(long)]
    pub ignore_requires_meridian: bool,
}

/// Parsed dependency specifier.
#[derive(Debug)]
struct ParsedDependency {
    name: SourceName,
    entry: Entry,
}

/// Run `mars add`.
pub fn run(args: &AddArgs, ctx: &super::MarsContext, json: bool) -> Result<i32, MarsError> {
    // Validate: filters require exactly one source
    let has_filters = !args.agents.is_empty()
        || !args.skills.is_empty()
        || !args.exclude.is_empty()
        || args.only_skills
        || args.only_agents;

    if has_filters && args.sources.len() > 1 {
        return Err(MarsError::InvalidRequest {
            message: "filters may only be used when adding exactly one source".to_string(),
        });
    }
    if args.subpath.is_some() && args.sources.len() != 1 {
        return Err(MarsError::InvalidRequest {
            message: "--subpath requires exactly one source argument".to_string(),
        });
    }
    if args.plugin.is_some() && args.sources.len() != 1 {
        return Err(MarsError::InvalidRequest {
            message: "--plugin requires exactly one source argument".to_string(),
        });
    }

    let filter_config = build_filter_config(args);

    // Build mutations for all sources
    let mutations: Vec<(SourceName, Entry)> = args
        .sources
        .iter()
        .map(|source| {
            let parsed = parse_dependency_specifier(
                source,
                args.subpath.as_deref(),
                args.plugin.as_deref(),
            )?;
            let entry = Entry {
                filter: filter_config.clone(),
                ..parsed.entry
            };
            // The rules of `mars.toml`, under the dependency's real name, before anything is written.
            let (entry, _) = Entry::read(
                parsed.name.as_ref(),
                entry.into(),
                mars_schema::dependency::Strictness::Consumer,
            )
            .map_err(crate::error::ConfigError::from)?;
            Ok((parsed.name, entry))
        })
        .collect::<Result<Vec<_>, MarsError>>()?;

    // For single source, use direct mutation path
    // For multi-source, apply mutations sequentially then run one sync
    if mutations.len() == 1 {
        let (name, entry) = mutations.into_iter().next().unwrap();

        let request = SyncRequest {
            resolution: ResolutionMode::Normal,
            mutation: Some(ConfigMutation::UpsertDependency {
                name: name.clone(),
                entry: Box::new(entry),
            }),
            options: SyncOptions {
                ignore_requires_mars: args.ignore_requires_mars,
                ignore_requires_meridian: args.ignore_requires_meridian,
                ..SyncOptions::default()
            },
            recovery: Default::default(),
            lossiness_mode: crate::diagnostic::LossinessMode::Hidden,
        };

        let report = crate::sync::execute(ctx, &request)?;

        if !json {
            print_dependency_messages(&report.dependency_changes);
        }

        output::print_sync_report(&report, json, true);
        return Ok(0);
    }

    // Multi-source: send one batch mutation through sync pipeline.
    let request = SyncRequest {
        resolution: ResolutionMode::Normal,
        mutation: Some(ConfigMutation::BatchUpsert(mutations)),
        options: SyncOptions {
            ignore_requires_mars: args.ignore_requires_mars,
            ignore_requires_meridian: args.ignore_requires_meridian,
            ..SyncOptions::default()
        },
        recovery: Default::default(),
        lossiness_mode: crate::diagnostic::LossinessMode::Hidden,
    };

    let report = crate::sync::execute(ctx, &request)?;

    if !json {
        print_dependency_messages(&report.dependency_changes);
    }

    output::print_sync_report(&report, json, true);
    Ok(0)
}

/// Build FilterConfig from CLI args.
fn build_filter_config(args: &AddArgs) -> FilterConfig {
    FilterConfig {
        agents: if args.agents.is_empty() {
            None
        } else {
            Some(
                args.agents
                    .iter()
                    .map(|v| ItemName::from(v.as_str()))
                    .collect(),
            )
        },
        skills: if args.skills.is_empty() {
            None
        } else {
            Some(
                args.skills
                    .iter()
                    .map(|v| ItemName::from(v.as_str()))
                    .collect(),
            )
        },
        exclude: if args.exclude.is_empty() {
            None
        } else {
            Some(
                args.exclude
                    .iter()
                    .map(|v| ItemName::from(v.as_str()))
                    .collect(),
            )
        },
        rename: None,
        only_skills: args.only_skills,
        only_agents: args.only_agents,
    }
}

/// Parse a dependency specifier string into a name + Entry.
///
/// Formats:
/// - `owner/repo` → GitHub shorthand (no `.` in first segment, exactly one `/`)
/// - `owner/repo@version` → GitHub shorthand with version
/// - `github.com/owner/repo` → full git URL
/// - `https://github.com/owner/repo.git` → full git URL
/// - `./path` or `../path` or `/absolute` → local path
fn parse_dependency_specifier(
    spec: &str,
    explicit_subpath: Option<&str>,
    plugin: Option<&str>,
) -> Result<ParsedDependency, MarsError> {
    let parsed = parse::parse(spec).map_err(|e| {
        MarsError::Config(ConfigError::Invalid {
            message: e.to_string(),
        })
    })?;

    let explicit_subpath = explicit_subpath
        .map(|value| {
            SourceSubpath::new(value).map_err(|e| {
                MarsError::Config(ConfigError::Invalid {
                    message: e.to_string(),
                })
            })
        })
        .transpose()?;
    let source = match (parsed.url.clone(), parsed.path.clone()) {
        (Some(url), None) => Source::Git {
            url: url.to_string(),
        },
        (None, Some(path)) => Source::Path { path },
        _ => {
            return Err(MarsError::InvalidRequest {
                message: format!("`{spec}` is neither a git source nor a path"),
            });
        }
    };
    let subpath = merge_subpath(parsed.subpath.clone(), explicit_subpath)?;
    let name = derive_dependency_name(&parsed, subpath.as_ref(), plugin)?;

    Ok(ParsedDependency {
        name: SourceName::from(name),
        entry: Entry {
            version: parsed.version,
            subpath,
            plugin: plugin.map(str::to_owned),
            ..Entry::from_source(source)
        },
    })
}

fn merge_subpath(
    parsed_subpath: Option<SourceSubpath>,
    explicit_subpath: Option<SourceSubpath>,
) -> Result<Option<SourceSubpath>, MarsError> {
    match (parsed_subpath, explicit_subpath) {
        (Some(parsed), Some(explicit)) if parsed != explicit => Err(MarsError::InvalidRequest {
            message: format!(
                "conflicting subpath input: source provides `{parsed}` but --subpath provides `{explicit}`"
            ),
        }),
        (Some(parsed), Some(_)) => Ok(Some(parsed)),
        (Some(parsed), None) => Ok(Some(parsed)),
        (None, Some(explicit)) => Ok(Some(explicit)),
        (None, None) => Ok(None),
    }
}

/// The repository name, then what picks a package inside it: a `subpath` or a marketplace
/// plugin (the two never combine), so each plugin of one marketplace has its own name.
fn derive_dependency_name(
    parsed: &parse::ParsedSourceSpec,
    subpath: Option<&SourceSubpath>,
    plugin: Option<&str>,
) -> Result<String, MarsError> {
    let root_name = parsed.name.split('/').next().ok_or_else(|| {
        MarsError::Config(ConfigError::Invalid {
            message: format!("cannot derive dependency name from `{}`", parsed.raw),
        })
    })?;

    Ok(match (subpath, plugin) {
        (Some(subpath), _) => format!("{root_name}/{}", subpath.as_str()),
        (None, Some(plugin)) => format!("{root_name}/{plugin}"),
        (None, None) => root_name.to_string(),
    })
}

fn print_dependency_messages(changes: &[DependencyUpsertChange]) {
    for change in changes {
        if change.already_exists {
            output::print_warn(&format!(
                "dependency `{}` already exists — updated",
                change.name
            ));
            if let Some(old_filter) = &change.old_filter
                && old_filter != &change.new_filter
            {
                output::print_info(&format!(
                    "filters changed: {} → {}",
                    format_filter(old_filter),
                    format_filter(&change.new_filter)
                ));
            }
        } else {
            output::print_info(&format!("added dependency `{}`", change.name));
        }
    }
}

fn format_filter(filter: &FilterConfig) -> String {
    if filter.only_skills {
        return "only_skills=true".to_string();
    }
    if filter.only_agents {
        return "only_agents=true".to_string();
    }

    let mut parts = Vec::new();
    if let Some(agents) = &filter.agents {
        parts.push(format!("agents=[{}]", format_item_names(agents)));
    }
    if let Some(skills) = &filter.skills {
        parts.push(format!("skills=[{}]", format_item_names(skills)));
    }
    if let Some(exclude) = &filter.exclude {
        parts.push(format!("exclude=[{}]", format_item_names(exclude)));
    }

    if parts.is_empty() {
        "all".to_string()
    } else {
        parts.join(", ")
    }
}

fn format_item_names(items: &[ItemName]) -> String {
    items
        .iter()
        .map(|item| item.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parse_github_shorthand_with_version() {
        // (specifier, name, url, version)
        for (specifier, name, url, version) in [
            (
                "meridian-flow/meridian-base@v0.5.0",
                "meridian-base",
                "https://github.com/meridian-flow/meridian-base",
                Some("v0.5.0"),
            ),
            (
                "github.com/meridian-flow/meridian-dev-workflow@v2",
                "meridian-dev-workflow",
                "https://github.com/meridian-flow/meridian-dev-workflow",
                Some("v2"),
            ),
            (
                "https://github.com/someone/cool-agents.git",
                "cool-agents",
                "https://github.com/someone/cool-agents",
                None,
            ),
            (
                "git@github.com:someone/cool-agents.git",
                "cool-agents",
                "git@github.com:someone/cool-agents.git",
                None,
            ),
            (
                "git@github.com:someone/cool-agents.git@v2",
                "cool-agents",
                "git@github.com:someone/cool-agents.git",
                Some("v2"),
            ),
        ] {
            let parsed = parse_dependency_specifier(specifier, None, None).unwrap();
            assert_eq!(parsed.name, name, "{specifier}");
            assert_eq!(parsed.entry.url(), Some(url), "{specifier}");
            assert_eq!(parsed.entry.version.as_deref(), version, "{specifier}");
        }
    }

    #[test]
    fn parse_local_paths() {
        for (specifier, name) in [("./my-agents", "my-agents"), ("/home/dev/agents", "agents")] {
            let parsed = parse_dependency_specifier(specifier, None, None).unwrap();
            assert_eq!(parsed.name, name);
            assert!(parsed.entry.url().is_none());
            assert_eq!(parsed.entry.path(), Some(Path::new(specifier)));
        }
    }

    #[test]
    fn parse_source_embedded_subpath() {
        // (specifier, explicit subpath, name)
        for (specifier, subpath, name) in [
            ("owner/repo/plugins/foo", None, "repo/plugins/foo"),
            (
                "gitlab:group/subgroup/repo",
                Some("plugins/foo"),
                "repo/plugins/foo",
            ),
        ] {
            let parsed = parse_dependency_specifier(specifier, subpath, None).unwrap();
            assert_eq!(parsed.name, name);
            assert_eq!(
                parsed.entry.subpath.as_ref().map(SourceSubpath::as_str),
                Some("plugins/foo")
            );
        }

        let err = parse_dependency_specifier("owner/repo/plugins/foo", Some("plugins/bar"), None)
            .unwrap_err();
        assert!(matches!(err, MarsError::InvalidRequest { .. }));
    }
}
