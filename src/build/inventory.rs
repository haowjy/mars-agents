use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::compiler::agents::{AgentMode, HarnessKind, ModelPolicyMatchType, parse_agent_content};
use crate::compiler::native_agent_manifest::{
    agent_is_native_for_harness, read_native_agent_manifest,
};
use crate::error::{ConfigError, MarsError};

#[derive(Debug, Clone)]
struct ParsedAgentInventory {
    name: String,
    description: String,
    model: Option<String>,
    fanout: Vec<String>,
    mode: AgentMode,
}

pub fn build_inventory_prompt(
    mars_dir: &Path,
    subagents_filter: &[String],
    harness: &str,
    warnings: &mut Vec<String>,
) -> Result<String, MarsError> {
    let agents_dir = mars_dir.join("agents");
    if !agents_dir.is_dir() {
        return Ok(String::new());
    }

    let read_dir = match std::fs::read_dir(&agents_dir) {
        Ok(entries) => entries,
        Err(err) => {
            warnings.push(format!(
                "failed to read agent inventory from {}: {err}",
                agents_dir.display()
            ));
            return Ok(String::new());
        }
    };

    let mut agent_paths: Vec<PathBuf> = read_dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    agent_paths.sort();

    let mut primary_agents = Vec::new();
    let mut subagent_agents = Vec::new();

    for path in agent_paths {
        match parse_inventory_agent(&path) {
            Ok((Some(agent), agent_warnings)) => {
                warnings.extend(agent_warnings);
                if agent.mode == AgentMode::Primary {
                    primary_agents.push(agent);
                } else {
                    subagent_agents.push(agent);
                }
            }
            Ok((None, agent_warnings)) => warnings.extend(agent_warnings),
            Err(err) => {
                return Err(MarsError::Config(ConfigError::Invalid { message: err }));
            }
        }
    }

    if !subagents_filter.is_empty() {
        primary_agents.retain(|agent| {
            subagents_filter
                .iter()
                .any(|f| f.eq_ignore_ascii_case(&agent.name))
        });
        subagent_agents.retain(|agent| {
            subagents_filter
                .iter()
                .any(|f| f.eq_ignore_ascii_case(&agent.name))
        });
    }

    if primary_agents.is_empty() && subagent_agents.is_empty() {
        return Ok(String::new());
    }

    primary_agents.sort_by(|left, right| left.name.cmp(&right.name));
    subagent_agents.sort_by(|left, right| left.name.cmp(&right.name));

    let manifest = read_native_agent_manifest(mars_dir);
    let is_native_for_harness = |agent: &ParsedAgentInventory| -> bool {
        agent_is_native_for_harness(&manifest, &agent.name, harness)
    };

    let mut meridian_primary = Vec::new();
    let mut meridian_subagent = Vec::new();
    let mut native_agents = Vec::new();

    for agent in primary_agents {
        if is_native_for_harness(&agent) {
            native_agents.push(agent);
        } else {
            meridian_primary.push(agent);
        }
    }
    for agent in subagent_agents {
        if is_native_for_harness(&agent) {
            native_agents.push(agent);
        } else {
            meridian_subagent.push(agent);
        }
    }
    native_agents.sort_by(|left, right| left.name.cmp(&right.name));

    let mut lines = vec![
        "# Meridian Agents".to_string(),
        "".to_string(),
        "Write prompts to `/tmp/<name>.md`.".to_string(),
        "Use `--bg` + `meridian spawn wait` for parallel work.".to_string(),
        "Use `/handoff` when passing control back to the user.".to_string(),
    ];

    if !meridian_subagent.is_empty() {
        lines.extend(["".to_string(), "## Subagent".to_string()]);
        for agent in &meridian_subagent {
            lines.push(render_meridian_agent_line(agent));
        }
    }

    if !meridian_primary.is_empty() {
        lines.extend(["".to_string(), "## Primary".to_string()]);
        for agent in &meridian_primary {
            lines.push(render_meridian_agent_line(agent));
        }
    }

    if !native_agents.is_empty() {
        lines.extend(render_native_section_heading(harness));
        for agent in &native_agents {
            lines.push(render_native_agent_line(agent));
        }
    }

    Ok(lines.join("\n").trim().to_string())
}

fn parse_inventory_agent(
    path: &Path,
) -> Result<(Option<ParsedAgentInventory>, Vec<String>), String> {
    let content = std::fs::read_to_string(path).map_err(|err| {
        format!(
            "failed to read agent inventory file {}: {err}",
            path.display()
        )
    })?;

    let mut parse_diags = Vec::new();
    let (profile, _frontmatter) =
        parse_agent_content(&content, &mut parse_diags).map_err(|err| {
            format!(
                "failed to parse agent inventory file {}: {err}",
                path.display()
            )
        })?;

    let mut warnings = Vec::new();
    for diag in parse_diags {
        if diag.is_error() {
            return Err(format!(
                "agent inventory file {} has invalid frontmatter: {}",
                path.display(),
                diag.message()
            ));
        }
        warnings.push(format!(
            "agent inventory parse warning in {}: {}",
            path.display(),
            diag.message()
        ));
    }
    if !profile.model_invocable {
        return Ok((None, warnings));
    }

    let fallback_name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-agent")
        .to_string();
    let fanout = fallback_model_policies_for_inventory(&profile);
    let name = profile.name.unwrap_or(fallback_name);
    let description = profile.description.unwrap_or_default();
    let mode = profile.mode.clone().unwrap_or(AgentMode::Subagent);

    Ok((
        Some(ParsedAgentInventory {
            name,
            description,
            model: profile.model,
            fanout,
            mode,
        }),
        warnings,
    ))
}

fn fallback_model_policies_for_inventory(
    profile: &crate::compiler::agents::AgentProfile,
) -> Vec<String> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    // Limitation: this deduplicates exact fallback labels only. Alias-to-model
    // canonical dedupe requires alias catalog context not currently loaded here.
    for policy in &profile.model_policies {
        if policy.no_fallback {
            continue;
        }
        if !matches!(
            policy.match_type,
            ModelPolicyMatchType::Alias | ModelPolicyMatchType::Model
        ) {
            continue;
        }
        let value = policy.match_value.trim();
        if value.is_empty() {
            continue;
        }
        if seen.insert(value.to_string()) {
            entries.push(value.to_string());
        }
    }

    entries
}

fn render_meridian_agent_line(agent: &ParsedAgentInventory) -> String {
    let description = agent.description.trim();
    let mut line = if description.is_empty() {
        format!("- `meridian spawn -a {}`", agent.name)
    } else {
        format!("- `meridian spawn -a {}`: {}", agent.name, description)
    };

    if let Some(model) = agent.model.as_ref().map(|value| value.trim())
        && !model.is_empty()
    {
        line.push_str(" | Model: ");
        line.push_str(model);
    }

    if !agent.fanout.is_empty() {
        line.push_str(" | Fan-out: ");
        line.push_str(&agent.fanout.join(", "));
    }

    line
}

fn render_native_agent_line(agent: &ParsedAgentInventory) -> String {
    let description = agent.description.trim();
    if description.is_empty() {
        format!("- {}", agent.name)
    } else {
        format!("- {}: {}", agent.name, description)
    }
}

fn render_native_section_heading(harness: &str) -> Vec<String> {
    match HarnessKind::from_str(harness) {
        Some(HarnessKind::Claude) => vec![
            "".to_string(),
            "## Claude Agents (use `Agent({subagent_type: \"...\"})` tool)".to_string(),
        ],
        Some(_) => vec![
            "".to_string(),
            "## Native Agents".to_string(),
            "Use your native subagent tool for agents listed here.".to_string(),
        ],
        None => vec![
            "".to_string(),
            "## Native Agents".to_string(),
            "Use your native subagent tool for agents listed here.".to_string(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::agents::HarnessKind;
    use crate::compiler::write_native_agent_manifest_from_lock;
    use crate::lock::{ItemKind, LockFile, LockedItemV2, OutputRecord};
    use std::fs;
    use tempfile::TempDir;

    fn write_agent(mars_dir: &Path, name: &str, content: &str) {
        let agents_dir = mars_dir.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join(format!("{name}.md")), content).unwrap();
    }

    fn sample_agent_content(name: &str, mode: &str, description: &str) -> String {
        format!(
            "---\nname: {name}\ndescription: {description}\nmode: {mode}\nmodel: test-model\n---\nBody."
        )
    }

    fn lock_with_native_agent_paths(
        agent_key: &str,
        canonical_dest: &str,
        native_dest: &str,
        harness: HarnessKind,
    ) -> LockFile {
        let mut lock = LockFile::empty();
        lock.items.insert(
            agent_key.to_string(),
            LockedItemV2 {
                source: "test".into(),
                kind: ItemKind::Agent,
                version: None,
                source_checksum: "sha256:src".into(),
                outputs: vec![
                    OutputRecord {
                        target_root: ".mars".to_string(),
                        dest_path: canonical_dest.into(),
                        installed_checksum: "sha256:mars".into(),
                    },
                    OutputRecord {
                        target_root: harness.target_dir().to_string(),
                        dest_path: native_dest.into(),
                        installed_checksum: "sha256:native".into(),
                    },
                ],
            },
        );
        lock
    }

    fn lock_with_native_agent(agent_name: &str, harness: HarnessKind) -> LockFile {
        let canonical = format!("agents/{agent_name}.md");
        lock_with_native_agent_paths(
            &format!("agent/{agent_name}"),
            &canonical,
            &canonical,
            harness,
        )
    }

    #[test]
    fn inventory_without_manifest_renders_all_as_meridian() {
        let temp = TempDir::new().unwrap();
        let mars_dir = temp.path().join(".mars");
        write_agent(
            &mars_dir,
            "coder",
            &sample_agent_content("coder", "subagent", "Features and refactors"),
        );
        write_agent(
            &mars_dir,
            "product-lead",
            &sample_agent_content("product-lead", "primary", "Intent capture"),
        );

        let mut warnings = Vec::new();
        let inventory = build_inventory_prompt(&mars_dir, &[], "claude", &mut warnings).unwrap();

        assert!(inventory.contains("Write prompts to `/tmp/<name>.md`."));
        assert!(inventory.contains("## Subagent"));
        assert!(
            inventory.contains(
                "- `meridian spawn -a coder`: Features and refactors | Model: test-model"
            )
        );
        assert!(inventory.contains("## Primary"));
        assert!(
            inventory
                .contains("- `meridian spawn -a product-lead`: Intent capture | Model: test-model")
        );
        assert!(!inventory.contains("## Claude Agents"));
        assert!(!inventory.contains("## Native Agents"));
    }

    #[test]
    fn inventory_with_manifest_renders_split_sections_for_claude() {
        let temp = TempDir::new().unwrap();
        let project_root = temp.path();
        let mars_dir = project_root.join(".mars");
        write_agent(
            &mars_dir,
            "coder",
            &sample_agent_content("coder", "subagent", "Features and refactors"),
        );
        write_agent(
            &mars_dir,
            "explorer",
            &sample_agent_content("explorer", "subagent", "Codebase structure"),
        );
        write_agent(
            &mars_dir,
            "frontend-coder",
            &sample_agent_content("frontend-coder", "subagent", "Frontend implementation"),
        );

        write_native_agent_manifest_from_lock(
            project_root,
            &lock_with_native_agent("frontend-coder", HarnessKind::Claude),
        )
        .unwrap();

        let mut warnings = Vec::new();
        let inventory = build_inventory_prompt(&mars_dir, &[], "claude", &mut warnings).unwrap();

        assert!(
            inventory.contains(
                "- `meridian spawn -a coder`: Features and refactors | Model: test-model"
            )
        );
        assert!(
            inventory
                .contains("- `meridian spawn -a explorer`: Codebase structure | Model: test-model")
        );
        assert!(
            inventory.contains("## Claude Agents (use `Agent({subagent_type: \"...\"})` tool)")
        );
        assert!(inventory.contains("- frontend-coder: Frontend implementation"));
        assert!(!inventory.contains("meridian spawn -a frontend-coder"));
    }

    #[test]
    fn inventory_with_renamed_profile_lands_in_native_section() {
        let temp = TempDir::new().unwrap();
        let project_root = temp.path();
        let mars_dir = project_root.join(".mars");
        write_agent(
            &mars_dir,
            "my-file",
            &sample_agent_content("logical-name", "subagent", "Renamed profile agent"),
        );

        write_native_agent_manifest_from_lock(
            project_root,
            &lock_with_native_agent_paths(
                "agent/my-file",
                "agents/my-file.md",
                "agents/logical-name.md",
                HarnessKind::Claude,
            ),
        )
        .unwrap();

        let mut warnings = Vec::new();
        let inventory = build_inventory_prompt(&mars_dir, &[], "claude", &mut warnings).unwrap();

        assert!(
            inventory.contains("## Claude Agents (use `Agent({subagent_type: \"...\"})` tool)")
        );
        assert!(inventory.contains("- logical-name: Renamed profile agent"));
        assert!(!inventory.contains("meridian spawn -a logical-name"));
    }

    #[test]
    fn inventory_with_manifest_on_other_harness_keeps_native_agents_in_meridian_section() {
        let temp = TempDir::new().unwrap();
        let project_root = temp.path();
        let mars_dir = project_root.join(".mars");
        write_agent(
            &mars_dir,
            "frontend-coder",
            &sample_agent_content("frontend-coder", "subagent", "Frontend implementation"),
        );

        write_native_agent_manifest_from_lock(
            project_root,
            &lock_with_native_agent("frontend-coder", HarnessKind::Claude),
        )
        .unwrap();

        let mut warnings = Vec::new();
        let inventory = build_inventory_prompt(&mars_dir, &[], "codex", &mut warnings).unwrap();

        assert!(inventory.contains(
            "- `meridian spawn -a frontend-coder`: Frontend implementation | Model: test-model"
        ));
        assert!(!inventory.contains("## Native Agents"));
        assert!(!inventory.contains("## Claude Agents"));
    }
}
