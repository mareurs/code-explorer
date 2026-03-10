//! Shared formatting helpers used by multiple tool format_compact implementations.

use serde_json::Value;

/// Format a line range like "L35-50" or "L35" if start == end.
pub(crate) fn format_line_range(start: u64, end: u64) -> String {
    if start == end || end == 0 {
        format!("L{start}")
    } else {
        format!("L{start}-{end}")
    }
}

/// Truncate a path to max_len chars, replacing the middle with "…".
#[allow(dead_code)] // Used by format_compact impls in tool modules.
pub(crate) fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }
    if max_len < 5 {
        let end = crate::tools::floor_char_boundary(path, max_len);
        return path[..end].to_string();
    }
    let keep_end = max_len / 2;
    let keep_start = max_len - keep_end - 1; // 1 for the ellipsis char
    let start = crate::tools::floor_char_boundary(path, keep_start);
    let tail_offset = crate::tools::floor_char_boundary(path, path.len() - keep_end);
    format!("{}…{}", &path[..start], &path[tail_offset..])
}

/// Format an overflow hint as a compact one-liner.
pub(crate) fn format_overflow(overflow: &Value) -> String {
    let shown = overflow["shown"].as_u64().unwrap_or(0);
    let total = overflow["total"].as_u64().unwrap_or(0);
    let hint = overflow["hint"].as_str().unwrap_or("");
    if total > shown {
        format!("  … showing {shown} of {total} — {hint}")
    } else {
        format!("  … showing first {shown} — {hint}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_range_single() {
        assert_eq!(format_line_range(35, 35), "L35");
    }

    #[test]
    fn line_range_span() {
        assert_eq!(format_line_range(35, 50), "L35-50");
    }

    #[test]
    fn line_range_zero_end() {
        assert_eq!(format_line_range(10, 0), "L10");
    }

    #[test]
    fn truncate_short_path() {
        assert_eq!(truncate_path("src/main.rs", 30), "src/main.rs");
    }

    #[test]
    fn truncate_long_path() {
        let long = "src/tools/very/deeply/nested/path/to/file.rs";
        let result = truncate_path(long, 25);
        assert!(
            result.chars().count() <= 25,
            "got len {} for '{}'",
            result.chars().count(),
            result
        );
        assert!(result.contains('…'));
    }

    #[test]
    fn truncate_path_unicode_does_not_panic() {
        // Each '─' (BOX DRAWINGS LIGHT HORIZONTAL) is 3 bytes.
        // Build a path whose byte length > max_len but where max_len falls
        // inside a multi-byte char — without floor_char_boundary this panics.
        let unicode_segment = "─".repeat(30); // 90 bytes
        let path = format!("src/tools/{}/file.rs", unicode_segment);
        // max_len=25: keep_start=12, keep_end=12 — both fall inside multi-byte chars.
        let result = truncate_path(&path, 25);
        assert!(result.contains('…'), "must contain ellipsis");
        // Must be valid UTF-8 (no panic = passes, but also verify well-formed)
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn overflow_with_total() {
        let ov = serde_json::json!({
            "shown": 50, "total": 234, "hint": "narrow with path="
        });
        let result = format_overflow(&ov);
        assert!(result.contains("50 of 234"));
        assert!(result.contains("narrow with path="));
    }

    #[test]
    fn overflow_without_total() {
        let ov = serde_json::json!({
            "shown": 50, "total": 50, "hint": "use more specific pattern"
        });
        let result = format_overflow(&ov);
        assert!(result.contains("first 50"));
    }
}
