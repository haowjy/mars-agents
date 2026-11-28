use std::path::Path;

use crate::error::MarsError;

/// The callers read files Mars or a provider wrote — an installed Agent under `.mars/`. Returns the `skills` list, or an empty vec if there is no
/// frontmatter, no `skills` field, or malformed YAML, and reads only the frontmatter block
/// between `---` delimiters, not the full markdown body.
pub fn parse_agent_skills(agent_path: &Path) -> Result<Vec<String>, MarsError> {
    let content = std::fs::read_to_string(agent_path)?;
    Ok(mars_schema::agent::read(&content)
        .fields
        .map(|profile| profile.skills.all())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_agent(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(format!("{name}.md"));
        fs::write(&path, content).unwrap();
        path
    }
    #[test]
    fn parse_agent_skills_reads_frontmatter() {
        let dir = TempDir::new().unwrap();
        let path = write_agent(
            dir.path(),
            "coder",
            "---\nskills:\n  - planning\n  - review\n---\n# Coder\n",
        );

        let skills = parse_agent_skills(&path).unwrap();
        assert_eq!(skills, vec!["planning", "review"]);
    }
}
