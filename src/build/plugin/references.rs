use indexmap::IndexMap;

use crate::error::MarsError;

/// Qualify skill references in an already-lowered agent document.
pub(super) fn qualify_agent_frontmatter(
    content: &str,
    installed_skills: &IndexMap<String, String>,
) -> Result<String, MarsError> {
    crate::frontmatter::rewrite_content_skills(content, installed_skills)
        .map(|rewritten| rewritten.unwrap_or_else(|| content.to_string()))
        .map_err(|error| invalid_reference_error("agent frontmatter", error))
}

/// Rewrite shipped `/skill` references in prose while leaving fenced code alone.
///
/// A slash starts a reference only at the beginning of a line or after
/// whitespace/backtick. This deliberately excludes Markdown link targets and
/// path-like prose. Names are tried longest-first so one shipped name cannot
/// consume the prefix of another.
pub(super) fn qualify_body_references(
    content: &str,
    references: &IndexMap<String, String>,
) -> Result<String, MarsError> {
    if references.is_empty() {
        return Ok(content.to_string());
    }

    let mut frontmatter = crate::frontmatter::parse(content)
        .map_err(|error| invalid_reference_error("markdown frontmatter", error))?;
    let rewritten = rewrite_body(frontmatter.body(), references);
    if rewritten == frontmatter.body() {
        return Ok(content.to_string());
    }
    frontmatter.set_body(rewritten);
    Ok(frontmatter.render())
}

fn rewrite_body(body: &str, references: &IndexMap<String, String>) -> String {
    let mut names: Vec<&str> = references.keys().map(String::as_str).collect();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));

    let mut output = String::with_capacity(body.len());
    let mut fence: Option<(char, usize)> = None;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some((character, length)) = fence {
            if is_closing_fence(trimmed, character, length) {
                fence = None;
            }
            output.push_str(line);
        } else if let Some(opened) = opening_fence(trimmed) {
            fence = Some(opened);
            output.push_str(line);
        } else {
            output.push_str(&rewrite_line(line, &names, references));
        }
    }
    output
}

fn opening_fence(line: &str) -> Option<(char, usize)> {
    let character = line.chars().next()?;
    if !matches!(character, '`' | '~') {
        return None;
    }
    let length = line
        .chars()
        .take_while(|candidate| *candidate == character)
        .count();
    (length >= 3).then_some((character, length))
}

fn is_closing_fence(line: &str, character: char, opener_length: usize) -> bool {
    let length = line
        .chars()
        .take_while(|candidate| *candidate == character)
        .count();
    length >= opener_length && line[length..].trim().is_empty()
}

fn rewrite_line(line: &str, names: &[&str], references: &IndexMap<String, String>) -> String {
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;

    while let Some(relative) = line[cursor..].find('/') {
        let slash = cursor + relative;
        let allowed_start = slash == 0
            || line[..slash]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_whitespace() || ch == '`');
        if !allowed_start {
            let end = slash + 1;
            output.push_str(&line[cursor..end]);
            cursor = end;
            continue;
        }

        let tail = &line[slash + 1..];
        let matched = names.iter().find(|name| {
            tail.strip_prefix(**name).is_some_and(|after| {
                after
                    .chars()
                    .next()
                    .is_none_or(|ch| ch != '/' && !is_name_character(ch))
            })
        });
        let Some(name) = matched else {
            let end = slash + 1;
            output.push_str(&line[cursor..end]);
            cursor = end;
            continue;
        };

        output.push_str(&line[cursor..slash]);
        output.push('/');
        output.push_str(&references[*name]);
        cursor = slash + 1 + name.len();
    }

    output.push_str(&line[cursor..]);
    output
}

fn is_name_character(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-')
}

fn invalid_reference_error(
    location: &str,
    error: crate::frontmatter::FrontmatterError,
) -> MarsError {
    MarsError::Config(crate::error::ConfigError::Invalid {
        message: format!("failed to rewrite plugin {location}: {error}"),
    })
}
