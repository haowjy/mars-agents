use std::path::Path;

/// Check if file content contains unresolved conflict markers.
pub fn has_conflict_markers(content: &[u8]) -> bool {
    if content.starts_with(b"<<<<<<<") {
        return true;
    }
    content
        .windows(8)
        .any(|w| w[0] == b'\n' && &w[1..] == b"<<<<<<<")
}

/// Check whether a file on disk contains unresolved conflict markers.
pub fn file_has_conflict_markers(path: &Path) -> bool {
    // Unreadable files have no reportable markers.
    std::fs::read(path)
        .map(|content| has_conflict_markers(&content))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_markers() {
        assert!(has_conflict_markers(
            b"some content\n<<<<<<< local\nconflict\n=======\nother\n>>>>>>> theirs\n"
        ));
    }

    #[test]
    fn detects_marker_at_start_of_file() {
        assert!(has_conflict_markers(
            b"<<<<<<< local\nconflict\n=======\nother\n>>>>>>> theirs\n"
        ));
    }

    #[test]
    fn detects_opening_marker_without_closing_marker() {
        assert!(has_conflict_markers(
            b"content\n<<<<<<< local\nunresolved content\n"
        ));
    }

    #[test]
    fn detects_markers_in_non_utf8_content() {
        assert!(has_conflict_markers(
            b"invalid: \xff\n<<<<<<< local\nunresolved content\n"
        ));
    }

    #[test]
    fn ignores_content_without_markers() {
        assert!(!has_conflict_markers(b"normal content\nwith lines\n"));
    }

    #[test]
    fn ignores_partial_marker() {
        assert!(!has_conflict_markers(b"content\n<<<<<< only six\n"));
    }

    #[test]
    fn ignores_paired_markers_in_middle_of_lines() {
        assert!(!has_conflict_markers(
            b"content <<<<<<< not at line start\nconflict\ncontent >>>>>>> also mid-line\n"
        ));
    }
}
