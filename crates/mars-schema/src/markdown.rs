//! Small Markdown syntax helpers shared by build and compiler paths.
//!
//! This lives beside `frontmatter` because fenced-block recognition is a
//! property of Markdown input, not of either consumer's orchestration layer.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fence {
    pub marker: char,
    pub length: usize,
    pub indentation: usize,
}

#[derive(Debug, Default)]
pub struct FenceTracker {
    open: Option<Fence>,
}

impl FenceTracker {
    /// Observe one source line, returning whether it belongs to a fenced block.
    pub fn visit(&mut self, line: &str) -> bool {
        if let Some(open) = self.open() {
            if is_closing_fence(line, open) {
                self.open = None;
            }
            return true;
        }

        if let Some(open) = opening_fence(line) {
            self.open = Some(open);
            return true;
        }

        false
    }

    pub fn open(&self) -> Option<Fence> {
        self.open
    }
}

fn opening_fence(line: &str) -> Option<Fence> {
    let (indentation, content) = after_fence_indentation(line)?;
    let marker = match content.as_bytes().first()? {
        b'`' => b'`',
        b'~' => b'~',
        _ => return None,
    };
    let length = content
        .as_bytes()
        .iter()
        .take_while(|candidate| **candidate == marker)
        .count();
    if length < 3 || (marker == b'`' && content[length..].contains('`')) {
        return None;
    }

    Some(Fence {
        marker: char::from(marker),
        length,
        indentation,
    })
}

fn is_closing_fence(line: &str, open: Fence) -> bool {
    let Some((_, content)) = after_fence_indentation(line) else {
        return false;
    };
    let marker = open.marker as u8;
    let length = content
        .as_bytes()
        .iter()
        .take_while(|candidate| **candidate == marker)
        .count();
    length >= open.length
        && content.as_bytes().first() == Some(&marker)
        && without_line_ending(&content[length..])
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t'))
}

fn after_fence_indentation(line: &str) -> Option<(usize, &str)> {
    let indentation = line
        .as_bytes()
        .iter()
        .take_while(|byte| **byte == b' ')
        .count();
    (indentation <= 3).then_some((indentation, &line[indentation..]))
}

fn without_line_ending(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}
