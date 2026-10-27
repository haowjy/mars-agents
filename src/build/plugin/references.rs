use indexmap::IndexMap;

use crate::error::MarsError;

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

    let mut frontmatter = mars_schema::frontmatter::parse(content)
        .map_err(|error| invalid_reference_error("markdown frontmatter", error))?;
    if !qualify_document(&mut frontmatter, references) {
        return Ok(content.to_string());
    }
    Ok(frontmatter.render())
}

/// `qualify_body_references` on a parsed document; returns whether the body changed.
pub(super) fn qualify_document(
    document: &mut mars_schema::frontmatter::Frontmatter,
    references: &IndexMap<String, String>,
) -> bool {
    if references.is_empty() {
        return false;
    }
    let rewritten = rewrite_body(document.body(), references);
    if rewritten == document.body() {
        return false;
    }
    document.set_body(rewritten);
    true
}

fn rewrite_body(body: &str, references: &IndexMap<String, String>) -> String {
    let mut names: Vec<&str> = references.keys().map(String::as_str).collect();
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));

    let mut output = String::with_capacity(body.len());
    let mut fences = mars_schema::markdown::FenceTracker::default();
    for line in body.split_inclusive('\n') {
        if fences.visit(line) {
            output.push_str(line);
        } else {
            output.push_str(&rewrite_line(line, &names, references));
        }
    }
    output
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
    ch.is_alphanumeric() || matches!(ch, '_' | '-' | ':')
}

fn invalid_reference_error(
    location: &str,
    error: mars_schema::frontmatter::FrontmatterError,
) -> MarsError {
    MarsError::Config(crate::error::ConfigError::Invalid {
        message: format!("failed to rewrite plugin {location}: {error}"),
    })
}
