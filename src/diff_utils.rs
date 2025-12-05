use regex::Regex;

/// Normalize a diff by removing the `index` lines which can vary in hash abbreviation length
/// between git and GitHub API responses.
/// The index line format is: "index <hash>..<hash> <mode>"
pub fn normalize_diff(diff: &str) -> String {
    let index_line_re = Regex::new(r"^index [0-9a-f]+\.\.[0-9a-f]+( [0-9]+)?$").unwrap();
    diff.lines()
        .filter(|line| !index_line_re.is_match(line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_diff() {
        // Test that diffs with different index line hash lengths are normalized to be equal
        let diff_10_char_hash = "diff --git a/foo b/foo\n\
index 0123456789..0123456789 100644\n\
--- a/foo\n\
+++ b/foo\n\
@@ -1 +1 @@\n\
-old content\n\
+new content";

        let diff_11_char_hash = "diff --git a/foo b/foo\n\
index 0123456789a..0123456789a 100644\n\
--- a/foo\n\
+++ b/foo\n\
@@ -1 +1 @@\n\
-old content\n\
+new content";

        let normalized_10 = normalize_diff(diff_10_char_hash);
        let normalized_11 = normalize_diff(diff_11_char_hash);

        // The normalized diffs should be equal
        assert_eq!(normalized_10, normalized_11);

        // The normalized diff should not contain the index line
        assert!(!normalized_10.contains("index "));
        assert!(!normalized_11.contains("index "));

        // The normalized diff should still contain the actual content
        assert!(normalized_10.contains("diff --git a/foo b/foo"));
        assert!(normalized_10.contains("--- a/foo"));
        assert!(normalized_10.contains("+++ b/foo"));
        assert!(normalized_10.contains("-old content"));
        assert!(normalized_10.contains("+new content"));
    }

    #[test]
    fn test_normalize_diff_preserves_code_with_index_keyword() {
        // Test that lines containing "index " in actual code are preserved
        let diff_with_index_code = "diff --git a/src/main.rs b/src/main.rs\n\
index abc123..def456 100644\n\
--- a/src/main.rs\n\
+++ b/src/main.rs\n\
@@ -1,2 +1,2 @@\n\
-let index = 0;\n\
+let index = 1;";

        let normalized = normalize_diff(diff_with_index_code);

        // The git index line should be removed
        assert!(!normalized.contains("index abc123..def456"));

        // But the code line with "index " should be preserved
        assert!(normalized.contains("let index = 0;"));
        assert!(normalized.contains("let index = 1;"));
    }
}
