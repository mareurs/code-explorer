//! Command type detection and smart output summarization.
//!
//! Detects whether a command is a test runner, a build tool, or something else,
//! then produces a structured summary appropriate for that command type.

use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Thresholds
// ---------------------------------------------------------------------------

/// Minimum total line count (stdout + stderr) before summarization kicks in.
pub(crate) const SUMMARY_LINE_THRESHOLD: usize = 50;
/// Inline line cap for buffer-only queries (e.g. `grep/sed @cmd_xxx`).
/// Kept separate from SUMMARY_LINE_THRESHOLD so "when to buffer" and "how much
/// to return from a buffer query" can be tuned independently.
pub(crate) const BUFFER_QUERY_INLINE_CAP: usize = 100;

/// Number of lines to keep from the top in generic summaries.
const HEAD_LINES: usize = 20;

/// Number of lines to keep from the bottom in generic summaries.
const TAIL_LINES: usize = 10;

// ---------------------------------------------------------------------------
// CommandType
// ---------------------------------------------------------------------------

/// Broad category of the command being run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandType {
    Test,
    Build,
    Generic,
}

// ---------------------------------------------------------------------------
// Regex patterns (compiled once via OnceLock)
// ---------------------------------------------------------------------------

fn test_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?x)
            (?:^|\s|/)
            (?:
                cargo\s+test
              | pytest
              | npm\s+test
              | npx\s+jest
              | jest
              | go\s+test
              | mvn\s+test
              | gradle\s+test
            )
            (?:\s|$)",
        )
        .expect("test regex")
    })
}

fn build_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?x)
            (?:^|\s|/)
            (?:
                cargo\s+(?:build|clippy|check)
              | npm\s+run\s+build
              | make(?:\s|$)
              | tsc(?:\s|$)
              | gcc(?:\s|$)
              | g\+\+(?:\s|$)
              | clang(?:\s|$)
              | javac(?:\s|$)
              | go\s+build
            )",
        )
        .expect("build regex")
    })
}

fn cargo_test_result_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(\d+)\s+passed;\s+(\d+)\s+failed;\s+(\d+)\s+ignored")
            .expect("cargo test regex")
    })
}

fn rust_error_code_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^error\[E\d+\]").expect("rust error regex"))
}

fn warning_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^warning(\[.+\])?:").expect("warning regex"))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Classify a command string into Test, Build, or Generic.
pub fn detect_command_type(command: &str) -> CommandType {
    // Test takes priority over build (e.g. "cargo test" is Test, not Build).
    if test_re().is_match(command) {
        CommandType::Test
    } else if build_re().is_match(command) {
        CommandType::Build
    } else {
        CommandType::Generic
    }
}

/// Returns `true` when the combined output is large enough to benefit from
/// summarization rather than raw output.
pub fn needs_summary(stdout: &str, stderr: &str) -> bool {
    let total_lines = count_lines(stdout) + count_lines(stderr);
    total_lines > SUMMARY_LINE_THRESHOLD
}

/// Produce a structured summary of test-runner output.
///
/// Parses cargo-test-style result lines, sums across multiple test binaries,
/// and extracts failure details.
pub fn summarize_test_output(stdout: &str, stderr: &str, exit_code: i32) -> Value {
    let mut passed: u64 = 0;
    let mut failed: u64 = 0;
    let mut ignored: u64 = 0;

    let combined = if stderr.is_empty() {
        stdout.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    };

    let re = cargo_test_result_re();
    for line in combined.lines() {
        if let Some(caps) = re.captures(line) {
            passed += caps[1].parse::<u64>().unwrap_or(0);
            failed += caps[2].parse::<u64>().unwrap_or(0);
            ignored += caps[3].parse::<u64>().unwrap_or(0);
        }
    }

    let failures = extract_test_failures(&combined);

    let mut result = json!({
        "type": "test",
        "exit_code": exit_code,
        "passed": passed,
        "failed": failed,
        "ignored": ignored,
    });

    if let Some(f) = failures {
        result["failures"] = Value::String(f);
    }

    result
}

/// Produce a structured summary of compiler / build-tool output.
///
/// Counts errors (with error codes) and warnings, and extracts the first
/// error block for quick diagnosis.
pub fn summarize_build_output(stdout: &str, stderr: &str, exit_code: i32) -> Value {
    let combined = if stderr.is_empty() {
        stdout.to_string()
    } else if stdout.is_empty() {
        stderr.to_string()
    } else {
        format!("{stdout}\n{stderr}")
    };

    let mut errors: u64 = 0;
    let mut warnings: u64 = 0;
    let mut first_error: Option<String> = None;

    let err_re = rust_error_code_re();
    let warn_re = warning_re();
    let lines: Vec<&str> = combined.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if err_re.is_match(line) {
            errors += 1;
            if first_error.is_none() {
                first_error = Some(extract_error_block(&lines, i));
            }
        } else if warn_re.is_match(line) {
            warnings += 1;
        }
    }

    let mut result = json!({
        "type": "build",
        "exit_code": exit_code,
        "errors": errors,
        "warnings": warnings,
    });

    if let Some(err) = first_error {
        result["first_error"] = Value::String(err);
    }

    result
}

/// Produce a head+tail summary for generic command output.
///
/// If stdout fits within HEAD_LINES + TAIL_LINES, it is returned verbatim.
/// Otherwise the middle is replaced with an "N lines omitted" marker.
pub fn summarize_generic(stdout: &str, stderr: &str, exit_code: i32) -> Value {
    let stdout_lines: Vec<&str> = stdout.lines().collect();
    let total_stdout_lines = stdout_lines.len();

    let summarized_stdout = if total_stdout_lines > HEAD_LINES + TAIL_LINES {
        let head: Vec<&str> = stdout_lines[..HEAD_LINES].to_vec();
        let tail: Vec<&str> = stdout_lines[total_stdout_lines - TAIL_LINES..].to_vec();
        let omitted = total_stdout_lines - HEAD_LINES - TAIL_LINES;
        format!(
            "{}\n--- {} lines omitted ---\n{}",
            head.join("\n"),
            omitted,
            tail.join("\n")
        )
    } else {
        stdout.to_string()
    };

    let mut result = json!({
        "type": "generic",
        "exit_code": exit_code,
        "stdout": summarized_stdout,
    });

    if !stderr.is_empty() {
        result["stderr"] = Value::String(stderr.to_string());
    }

    result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn count_lines(s: &str) -> usize {
    if s.is_empty() {
        0
    } else {
        s.lines().count()
    }
}

/// Truncate `text` to at most `max_lines` lines.
///
/// Returns `(truncated_text, lines_shown, lines_total)`.
/// When `lines_total <= max_lines`, `text` is returned unchanged and
/// `lines_shown == lines_total`.
#[allow(dead_code)]
pub(crate) fn truncate_lines(text: &str, max_lines: usize) -> (String, usize, usize) {
    let total = count_lines(text);
    if total <= max_lines {
        return (text.to_string(), total, total);
    }
    let truncated = text.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    (truncated, max_lines, total)
}

/// Extract text between the first `failures:` section markers in cargo test output.
fn extract_test_failures(output: &str) -> Option<String> {
    // Cargo test outputs failures between two "failures:" markers.
    // The first "failures:" is followed by stdout of failing tests.
    // The second "failures:" is followed by test names.
    let lines: Vec<&str> = output.lines().collect();
    let mut start = None;
    let mut end = None;

    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "failures:" {
            if start.is_none() {
                start = Some(i);
            } else {
                end = Some(i);
                break;
            }
        }
    }

    // If we found at least one "failures:" marker, collect everything after it
    // up to the second marker (or end of output).
    if let Some(s) = start {
        let e = end.unwrap_or(lines.len());
        // Include from start through the second failures block
        let section: Vec<&str> = if let Some(end_idx) = end {
            // Find the end of the second failures block (next "test result:" line or EOF)
            let block_end = lines[end_idx..]
                .iter()
                .position(|l| l.starts_with("test result:"))
                .map(|p| end_idx + p)
                .unwrap_or(lines.len());
            lines[s..block_end].to_vec()
        } else {
            lines[s..e].to_vec()
        };

        let text = section.join("\n").trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    } else {
        None
    }
}

/// Extract an error block: the error line plus continuation lines until
/// the next blank line or next error/warning.
fn extract_error_block(lines: &[&str], start: usize) -> String {
    let err_re = rust_error_code_re();
    let warn_re = warning_re();
    let mut block = vec![lines[start]];
    for line in &lines[start + 1..] {
        // Stop at blank lines or next top-level diagnostic
        if line.is_empty()
            || err_re.is_match(line)
            || warn_re.is_match(line)
            || line.starts_with("error:")
        {
            break;
        }
        block.push(line);
    }
    block.join("\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- detect_command_type --

    #[test]
    fn detect_test_command() {
        assert_eq!(detect_command_type("cargo test"), CommandType::Test);
        assert_eq!(
            detect_command_type("cargo test --release"),
            CommandType::Test
        );
        assert_eq!(detect_command_type("pytest tests/"), CommandType::Test);
        assert_eq!(detect_command_type("npm test"), CommandType::Test);
        assert_eq!(detect_command_type("npx jest"), CommandType::Test);
        assert_eq!(detect_command_type("go test ./..."), CommandType::Test);
    }

    #[test]
    fn detect_build_command() {
        assert_eq!(detect_command_type("cargo build"), CommandType::Build);
        assert_eq!(
            detect_command_type("cargo clippy -- -D warnings"),
            CommandType::Build
        );
        assert_eq!(detect_command_type("npm run build"), CommandType::Build);
        assert_eq!(detect_command_type("make"), CommandType::Build);
        assert_eq!(detect_command_type("tsc"), CommandType::Build);
        assert_eq!(detect_command_type("gcc main.c"), CommandType::Build);
    }

    #[test]
    fn detect_generic_command() {
        assert_eq!(detect_command_type("echo hello"), CommandType::Generic);
        assert_eq!(detect_command_type("ls -la"), CommandType::Generic);
        assert_eq!(detect_command_type("cat file.txt"), CommandType::Generic);
    }

    // -- needs_summary --

    #[test]
    fn short_output_not_summarized() {
        assert!(!needs_summary("hello\nworld\n", ""));
    }

    #[test]
    fn long_output_needs_summary() {
        let stdout: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
        assert!(needs_summary(&stdout, ""));
    }

    // -- summarize_test_output --

    #[test]
    fn summarize_cargo_test_all_pass() {
        let stdout = "running 5 tests\ntest a ... ok\ntest b ... ok\ntest c ... ok\ntest d ... ok\ntest e ... ok\n\ntest result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s\n";
        let summary = summarize_test_output(stdout, "", 0);
        assert_eq!(summary["passed"], 5);
        assert_eq!(summary["failed"], 0);
        assert!(summary.get("failures").is_none() || summary["failures"].is_null());
    }

    #[test]
    fn summarize_cargo_test_with_failures() {
        let stdout = "running 3 tests\ntest ok_test ... ok\ntest failing_test ... FAILED\ntest another ... ok\n\nfailures:\n\n---- failing_test stdout ----\nthread 'failing_test' panicked at 'assertion failed'\n\nfailures:\n    failing_test\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";
        let summary = summarize_test_output(stdout, "", 1);
        assert_eq!(summary["passed"], 2);
        assert_eq!(summary["failed"], 1);
        let failures = summary["failures"].as_str().unwrap();
        assert!(failures.contains("failing_test"));
    }

    #[test]
    fn summarize_cargo_test_multiple_binaries() {
        let stdout = "\
running 3 tests
test a ... ok
test b ... ok
test c ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

running 2 tests
test d ... ok
test e ... FAILED

failures:

---- e stdout ----
assertion failed

failures:
    e

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
";
        let summary = summarize_test_output(stdout, "", 1);
        // Sums across both binaries
        assert_eq!(summary["passed"], 4);
        assert_eq!(summary["failed"], 1);
    }

    // -- summarize_build_output --

    #[test]
    fn summarize_build_errors() {
        let stderr = "error[E0308]: mismatched types\n --> src/main.rs:5:20\n  |\n5 |     let x: String = 42;\n  |                     ^^ expected `String`, found integer\n\nwarning: unused variable: `y`\n --> src/main.rs:3:9\n  |\n3 |     let y = 1;\n  |         ^ help: consider prefixing with an underscore: `_y`\n\nerror: aborting due to 1 previous error; 1 warning emitted\n";
        let summary = summarize_build_output("", stderr, 1);
        assert_eq!(summary["errors"], 1); // only error[E...], not "error: aborting"
        assert_eq!(summary["warnings"], 1);
        assert!(summary["first_error"].as_str().unwrap().contains("E0308"));
    }

    #[test]
    fn summarize_build_no_errors() {
        let stderr = "warning: unused variable: `x`\n --> src/main.rs:2:9\n";
        let summary = summarize_build_output("", stderr, 0);
        assert_eq!(summary["errors"], 0);
        assert_eq!(summary["warnings"], 1);
        assert!(summary.get("first_error").is_none() || summary["first_error"].is_null());
    }

    // -- summarize_generic --

    #[test]
    fn summarize_generic_head_tail() {
        let lines: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
        let summary = summarize_generic(&lines, "", 0);
        let output = summary["stdout"].as_str().unwrap();
        assert!(output.contains("line 1"));
        assert!(output.contains("line 20"));
        assert!(output.contains("lines omitted"));
        assert!(output.contains("line 100"));
    }

    #[test]
    fn summarize_generic_short_output_verbatim() {
        let stdout = "line 1\nline 2\nline 3\n";
        let summary = summarize_generic(stdout, "", 0);
        let output = summary["stdout"].as_str().unwrap();
        assert_eq!(output, stdout);
        assert!(!output.contains("omitted"));
    }

    #[test]
    fn summarize_generic_includes_stderr() {
        let summary = summarize_generic("out\n", "err\n", 1);
        assert!(summary.get("stderr").is_some());
        assert_eq!(summary["stderr"].as_str().unwrap(), "err\n");
    }

    #[test]
    fn summarize_generic_omits_empty_stderr() {
        let summary = summarize_generic("out\n", "", 0);
        assert!(summary.get("stderr").is_none() || summary["stderr"].is_null());
    }

    // -- helpers --

    #[test]
    fn count_lines_empty() {
        assert_eq!(count_lines(""), 0);
    }

    #[test]
    fn count_lines_normal() {
        assert_eq!(count_lines("a\nb\nc"), 3);
    }

    #[test]
    fn truncate_lines_short_returns_unchanged() {
        let text = "a\nb\nc";
        let (out, shown, total) = truncate_lines(text, 10);
        assert_eq!(out, text);
        assert_eq!(shown, 3);
        assert_eq!(total, 3);
    }

    #[test]
    fn truncate_lines_exact_limit_not_truncated() {
        let text: String = (1..=5)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (out, shown, total) = truncate_lines(&text, 5);
        assert_eq!(shown, 5);
        assert_eq!(total, 5);
        assert_eq!(out, text);
    }

    #[test]
    fn truncate_lines_long_truncates_correctly() {
        let text: String = (1..=10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (out, shown, total) = truncate_lines(&text, 3);
        assert_eq!(shown, 3);
        assert_eq!(total, 10);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "line 1");
        assert_eq!(lines[2], "line 3");
    }

    #[test]
    fn truncate_lines_empty_string() {
        let (out, shown, total) = truncate_lines("", 10);
        assert_eq!(out, "");
        assert_eq!(shown, 0);
        assert_eq!(total, 0);
    }
}
