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
        let with_markers: [&[u8]; 4] = [
            b"some content\n<<<<<<< local\nconflict\n=======\nother\n>>>>>>> theirs\n",
            // at the start of the file
            b"<<<<<<< local\nconflict\n=======\nother\n>>>>>>> theirs\n",
            // opening marker without a closing one
            b"content\n<<<<<<< local\nunresolved content\n",
            // non-UTF-8 content
            b"invalid: \xff\n<<<<<<< local\nunresolved content\n",
        ];
        for content in with_markers {
            assert!(has_conflict_markers(content), "{content:?}");
        }

        let without_markers: [&[u8]; 3] = [
            b"normal content\nwith lines\n",
            // partial marker
            b"content\n<<<<<< only six\n",
            // paired markers in the middle of lines
            b"content <<<<<<< not at line start\nconflict\ncontent >>>>>>> also mid-line\n",
        ];
        for content in without_markers {
            assert!(!has_conflict_markers(content), "{content:?}");
        }
    }
}
