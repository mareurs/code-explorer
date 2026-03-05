# Source File Access Check — Quote-Aware Splitting Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix `check_source_file_access` so it doesn't false-positive on git commit messages (or any quoted argument) that happen to contain source-reading command names and file extensions.

**Architecture:** Add a `split_outside_quotes` helper that splits a command string on `&&`, `||`, `;`, `|` only when those separators appear *outside* quoted strings. Replace the naive `command.split('|')` in `check_source_file_access` with this helper, and change the per-segment match from "does the full segment contain a blocked command?" to "is the *first token* of the segment a blocked command?"

**Tech Stack:** Rust, `regex` crate (already a dependency). All changes in `src/util/path_security.rs`.

---

### Task 1: `split_outside_quotes` helper

**Files:**
- Modify: `src/util/path_security.rs` (add private helper function + tests)

**Step 1: Write the failing tests**

Add to the `tests` module at the bottom of `src/util/path_security.rs`:

```rust
#[test]
fn split_outside_quotes_no_separators() {
    let parts = split_outside_quotes("git status", &["&&", "||", ";", "|"]);
    assert_eq!(parts, vec!["git status"]);
}

#[test]
fn split_outside_quotes_pipe() {
    let parts = split_outside_quotes("cat foo.rs | grep fn", &["&&", "||", ";", "|"]);
    assert_eq!(parts, vec!["cat foo.rs", "grep fn"]);
}

#[test]
fn split_outside_quotes_ampersand() {
    let parts = split_outside_quotes("./build.sh && cat src/main.rs", &["&&", "||", ";", "|"]);
    assert_eq!(parts, vec!["./build.sh", "cat src/main.rs"]);
}

#[test]
fn split_outside_quotes_ampersand_inside_double_quotes() {
    // The && inside "..." must NOT split
    let parts = split_outside_quotes(
        r#"git commit -m "fix && cat src/main.rs""#,
        &["&&", "||", ";", "|"],
    );
    assert_eq!(parts, vec![r#"git commit -m "fix && cat src/main.rs""#]);
}

#[test]
fn split_outside_quotes_pipe_inside_single_quotes() {
    // The | inside '...' must NOT split
    let parts = split_outside_quotes("sed -n '1|2p' foo.rs", &["&&", "||", ";", "|"]);
    assert_eq!(parts, vec!["sed -n '1|2p' foo.rs"]);
}

#[test]
fn split_outside_quotes_double_pipe_before_single_pipe() {
    // "||" must be matched as one token, not split into two "|" segments
    let parts = split_outside_quotes("cmd1 || cmd2", &["&&", "||", ";", "|"]);
    assert_eq!(parts, vec!["cmd1", "cmd2"]);
}

#[test]
fn split_outside_quotes_semicolon() {
    let parts = split_outside_quotes("echo done; cat src/main.rs", &["&&", "||", ";", "|"]);
    assert_eq!(parts, vec!["echo done", "cat src/main.rs"]);
}

#[test]
fn split_outside_quotes_escaped_quote() {
    // \" inside a double-quoted string must not close the string
    let parts = split_outside_quotes(r#"echo "say \"hi\" && bye" && ls"#, &["&&", "||", ";", "|"]);
    assert_eq!(parts.len(), 2);
    assert!(parts[0].contains("say"));
    assert_eq!(parts[1].trim(), "ls");
}

#[test]
fn split_outside_quotes_empty_segments_skipped() {
    // Trailing semicolon — empty last segment is dropped
    let parts = split_outside_quotes("echo hi;", &["&&", "||", ";", "|"]);
    assert_eq!(parts, vec!["echo hi"]);
}
```

**Step 2: Run to verify they fail**

```bash
cargo test split_outside_quotes 2>&1
```
Expected: compile error — `split_outside_quotes` not defined yet.

**Step 3: Implement `split_outside_quotes`**

Add this private function to `src/util/path_security.rs`, just before `check_source_file_access` (before L430):

```rust
/// Split `s` on any separator in `seps` that appears *outside* single- or
/// double-quoted strings. Separators are checked in order — put longer
/// multi-char separators (e.g. `"&&"`) before their prefix (e.g. `"|"`) to
/// avoid a prefix match stealing the first character.
///
/// Backslash escaping outside single quotes is respected (`\"` does not close
/// a double-quoted string). Unclosed quotes are treated as closed at end-of-string.
/// Empty segments are silently dropped.
fn split_outside_quotes<'a>(s: &str, seps: &[&'a str]) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut seg_start = 0usize; // byte offset of current segment start
    let mut in_single = false;
    let mut in_double = false;
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut i = 0usize;

    'outer: while i < chars.len() {
        let (byte_pos, c) = chars[i];

        // Backslash: skip next char (escape) — only outside single quotes.
        if c == '\\' && !in_single {
            i += 2;
            continue;
        }

        // Toggle quote state.
        if c == '\'' && !in_double {
            in_single = !in_single;
            i += 1;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }

        // Outside quotes: check separators in order.
        if !in_single && !in_double {
            let remaining = &s[byte_pos..];
            for sep in seps {
                if remaining.starts_with(sep) {
                    let seg = s[seg_start..byte_pos].trim();
                    if !seg.is_empty() {
                        segments.push(seg.to_string());
                    }
                    let sep_char_count = sep.chars().count();
                    i += sep_char_count;
                    seg_start = chars.get(i).map(|(b, _)| *b).unwrap_or(s.len());
                    continue 'outer;
                }
            }
        }

        i += 1;
    }

    // Remaining segment after the last separator.
    let last = s[seg_start..].trim();
    if !last.is_empty() {
        segments.push(last.to_string());
    }

    segments
}
```

**Step 4: Run tests to verify they pass**

```bash
cargo test split_outside_quotes 2>&1
```
Expected: all 9 tests pass.

**Step 5: Run full suite**

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test 2>&1
```
Expected: all existing tests still pass.

**Step 6: Commit**

```bash
git add src/util/path_security.rs
git commit -m "feat: add split_outside_quotes helper for quote-aware command splitting"
```

---

### Task 2: Update `check_source_file_access` + new tests

**Files:**
- Modify: `src/util/path_security.rs:430-465` (`check_source_file_access` body)
- Test: `src/util/path_security.rs` (tests module)

**Step 1: Write the failing tests**

Add to the `tests` module (after the existing `check_source_file_access` tests, around L1101):

```rust
// --- quote-aware splitting ---

#[test]
fn git_commit_with_tail_in_message_not_blocked() {
    // "tail" and ".rs" appear inside the commit message — must NOT block
    assert!(check_source_file_access(
        r#"git commit -m "feat: tail-50 of log, output_buffer.rs, workflow.rs""#
    )
    .is_none());
}

#[test]
fn git_commit_with_ampersand_and_source_in_message_not_blocked() {
    // "&&" and "cat src/main.rs" inside the quoted message — must NOT block
    assert!(check_source_file_access(
        r#"git commit -m "fix && cat src/main.rs was broken""#
    )
    .is_none());
}

#[test]
fn compound_and_then_cat_blocked() {
    // cat src/main.rs is a real command after &&
    assert!(check_source_file_access("./build.sh && cat src/main.rs").is_some());
}

#[test]
fn semicolon_then_cat_blocked() {
    assert!(check_source_file_access("echo done; cat src/main.rs").is_some());
}

#[test]
fn or_then_tail_blocked() {
    assert!(check_source_file_access("cargo build || tail src/lib.rs").is_some());
}

#[test]
fn pipe_chain_with_source_blocked() {
    // tail is the first token of its segment — blocked
    assert!(check_source_file_access("tail src/main.rs | grep error").is_some());
}
```

**Step 2: Run to verify they fail**

```bash
cargo test git_commit_with_tail compound_and_then_cat semicolon_then_cat or_then_tail pipe_chain_with 2>&1
```
Expected: `git_commit_with_tail_in_message_not_blocked` and `git_commit_with_ampersand_and_source_in_message_not_blocked` **fail** (currently blocked); the "blocked" tests may pass or fail depending on current behavior.

**Step 3: Replace `check_source_file_access` body**

Replace the entire function body (L430-465) with:

```rust
pub fn check_source_file_access(command: &str) -> Option<String> {
    let cmd_re = Regex::new(SOURCE_ACCESS_COMMANDS).ok()?;
    let ext_re = Regex::new(SOURCE_EXTENSIONS).ok()?;

    // Split on compound-command operators and pipes, respecting quoted strings.
    // Order: "&&"/"||" before "|" so that "||" is not mis-split as two "|" tokens.
    let segments = split_outside_quotes(command, &["&&", "||", ";", "|"]);

    let blocked = segments.iter().find(|seg| {
        // Heredoc: the command reads from stdin, not a source file.
        if seg.contains("<<") {
            return false;
        }
        // Only the *first token* of a segment is the actual command being executed.
        // Matching against the first token (not the full segment string) prevents
        // false positives from quoted arguments containing command names, e.g.:
        //   git commit -m "feat: tail-50 of log, output_buffer.rs"
        let first_token = seg.split_whitespace().next().unwrap_or("");
        if !cmd_re.is_match(first_token) {
            return false;
        }
        // Check the full segment for a source extension so that quoted file paths
        // (e.g. `cat "src/main.rs"`) are still caught.
        ext_re.is_match(seg.as_str())
    })?;

    // Derive the hint from the specific command that triggered the block.
    let first_cmd = blocked.split_whitespace().next().unwrap_or("");
    let hint = match first_cmd {
        "sed" | "awk" => {
            "use read_file(path, start_line, end_line), list_symbols(path), \
             find_symbol(name, include_body=true), or search_pattern(regex) instead. \
             Re-run with acknowledge_risk: true if you need raw shell access."
        }
        _ => {
            "use read_file(path, start_line, end_line) or list_symbols(path) + \
             find_symbol(name, include_body=true) instead. \
             Re-run with acknowledge_risk: true if you need raw shell access."
        }
    };

    Some(hint.to_string())
}
```

**Step 4: Run the new tests**

```bash
cargo test git_commit_with_tail compound_and_then_cat semicolon_then_cat or_then_tail pipe_chain_with 2>&1
```
Expected: all 6 pass.

**Step 5: Run the full test suite — all existing tests must still pass**

```bash
cargo test check_source_file_access 2>&1
```
Expected: all source-file-access tests pass (new + existing).

**Step 6: Run complete suite**

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test 2>&1
```
Expected: 1049+ pass, 0 fail.

**Step 7: Commit**

```bash
git add src/util/path_security.rs
git commit -m "fix: quote-aware splitting in check_source_file_access

Prevents false positives from git commit messages (or any quoted
argument) that contain source-reading command names and file
extensions. Uses split_outside_quotes to split on &&/||/;/| only
outside quoted strings, then checks the first token of each segment
(not the full string) against SOURCE_ACCESS_COMMANDS.

Fixes: git commit -m '...tail-50...output_buffer.rs...' being blocked"
```

---

## Verification

After implementation, test the original failing case end-to-end via the MCP server (after `/mcp` restart):

```
run_command("git reset --soft HEAD~1 && git commit -m 'feat: tail-50 of log, output_buffer.rs'")
```

Expected: executes normally (dangerous-command ack may fire for `git reset --hard`, but the source-file check must NOT block).

Also verify the block still works for a true case:
```
run_command("cat src/tools/workflow.rs")
```
Expected: blocked with "shell access to source files is blocked".
