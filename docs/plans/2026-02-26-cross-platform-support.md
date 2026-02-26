# Cross-Platform Support (macOS + Windows) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make code-explorer compile, pass tests, and run correctly on macOS and Windows in addition to Linux.

**Architecture:** Five focused changes address all platform dependencies: (1) fix `home_dir()` to use platform-correct env vars, (2) fix `file://` URI ↔ path conversion via the `url` crate, (3) `cfg`-gate shell execution for Windows `cmd.exe`, (4) normalize path separators in the embedding index, (5) fix hardcoded Unix paths in tests. Then add CI matrix entries for all three platforms.

**Tech Stack:** Rust `std::path`, `url` crate (via `lsp-types`), `#[cfg]` attributes, GitHub Actions matrix

---

## Status Tracker

| Task | Status | Commit | Notes |
|------|--------|--------|-------|
| 1. Fix `home_dir()` | DONE | `1617ab7` | Falls back to USERPROFILE |
| 2. Fix URI ↔ path conversion | DONE | `13ee351` | Added `url` crate, fixed both files |
| 3. Platform-gate shell execution | DONE | `55a467a` | `sh` vs `cmd.exe` via cfg |
| 4. Normalize embed index paths | DONE | `795bb60` | `.replace('\\', "/")` |
| 5. Add platform-specific deny-list paths | DONE | `1a34908` | Linux/macOS/Windows deny-lists |
| 6. Fix hardcoded Unix paths in tests | DONE | `9665961` | 4 files, ~10 test fixes |
| 7. Add macOS + Windows CI matrix | DONE | `300e9ee` | 3-OS matrix with fail-fast: false |
| 8. Final cross-platform verification | DONE | — | fmt, clippy, test all pass |

---

### Task 1: Fix `home_dir()` for Windows

**Why:** The current `home_dir()` reads only `$HOME`, which doesn't exist on Windows. This makes the entire deny-list empty on Windows — a **security vulnerability** where `~/.ssh`, `~/.aws` etc. become readable by the LLM.

**Files:**
- Modify: `src/util/path_security.rs:69-71`
- Test: `src/util/path_security.rs` (existing `tests` module)

**Step 1: Write the failing test**

Add to the `tests` module in `src/util/path_security.rs`:

```rust
#[test]
fn home_dir_returns_some_on_all_platforms() {
    // home_dir() must return Some on every platform we support.
    // On Linux/macOS it reads $HOME, on Windows $USERPROFILE.
    let home = home_dir();
    assert!(
        home.is_some(),
        "home_dir() returned None — deny-list will be empty (security bug)"
    );
}
```

**Step 2: Run test to verify it passes on Linux (baseline)**

Run: `cargo test home_dir_returns_some -- --nocapture`
Expected: PASS on Linux (since `$HOME` is set)

**Step 3: Fix `home_dir()` to support Windows**

Replace `src/util/path_security.rs:69-71`:

```rust
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}
```

**Step 4: Run all path_security tests**

Run: `cargo test path_security -- --nocapture`
Expected: All PASS

**Step 5: Commit**

```
feat: support Windows home directory in path security

home_dir() now checks USERPROFILE when HOME is unset,
fixing a security gap where the deny-list was empty on Windows.
```

---

### Task 2: Fix URI ↔ path conversion for Windows

**Why:** Two separate `uri_to_path` / `path_to_uri` implementations hand-roll `file://` prefix stripping. On Windows, `file:///C:/foo` stripped to `/C:/foo` is an invalid path. The `url` crate (already available via `lsp-types`) handles this correctly.

**Files:**
- Modify: `src/lsp/client.rs:18-33` (the canonical implementation)
- Modify: `src/tools/symbol.rs:943-945` (duplicate — should call lsp/client version)
- Test: both files' `tests` modules

**Step 1: Write cross-platform test in `src/lsp/client.rs`**

Add to the `tests` module in `src/lsp/client.rs`:

```rust
#[test]
fn path_to_uri_roundtrip() {
    // Use a temp dir to get a real absolute path on any platform
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.rs");
    std::fs::write(&file, "").unwrap();

    let uri = path_to_uri(&file).unwrap();
    let uri_str = uri.as_str();
    assert!(uri_str.starts_with("file:///"), "URI should start with file:///: {}", uri_str);

    let back = uri_to_path(&uri);
    assert_eq!(back, file, "roundtrip should preserve the path");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test path_to_uri_roundtrip -- --nocapture`
Expected: FAIL — current `uri_to_path` returns wrong result because it strips `file://` instead of `file:///`

**Step 3: Rewrite both functions using `url::Url`**

In `src/lsp/client.rs`, replace lines 18-33:

```rust
fn uri_to_path(uri: &lsp_types::Uri) -> PathBuf {
    // lsp_types::Uri is a url::Url under the hood.
    // to_file_path() handles Windows drive letters and percent-encoding.
    uri.to_file_path().unwrap_or_else(|_| PathBuf::from(uri.path().as_str()))
}

fn path_to_uri(path: &Path) -> Result<lsp_types::Uri> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    lsp_types::Uri::from_file_path(&abs)
        .map_err(|_| anyhow::anyhow!("cannot convert path to URI: {}", abs.display()))
}
```

Check whether `lsp_types::Uri` has `from_file_path` and `to_file_path` — if it delegates to `url::Url`, these should be available. If not, add `url = "2"` to `Cargo.toml` and use `url::Url` directly, then convert.

**Step 4: Fix `src/tools/symbol.rs:943-945`**

Replace the standalone `uri_to_path`:

```rust
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    uri.parse::<url::Url>()
        .ok()
        .and_then(|u| u.to_file_path().ok())
}
```

This may require adding `url = "2"` to `[dependencies]` in `Cargo.toml` (check if `lsp-types` already re-exports it).

**Step 5: Update test in `src/tools/symbol.rs`**

Replace `uri_to_path_strips_prefix` test:

```rust
#[test]
fn uri_to_path_parses_unix_uri() {
    let p = uri_to_path("file:///home/user/code.rs").unwrap();
    assert_eq!(p, PathBuf::from("/home/user/code.rs"));
}

#[cfg(windows)]
#[test]
fn uri_to_path_parses_windows_uri() {
    let p = uri_to_path("file:///C:/Users/user/code.rs").unwrap();
    assert_eq!(p, PathBuf::from("C:\\Users\\user\\code.rs"));
}
```

**Step 6: Run tests**

Run: `cargo test uri_to_path -- --nocapture && cargo test path_to_uri -- --nocapture`
Expected: All PASS

**Step 7: Commit**

```
fix: use url crate for file URI ↔ path conversion

Hand-rolled file:// prefix stripping broke on Windows where
URIs use file:///C:/... format. Now uses url::Url which handles
drive letters, UNC paths, and percent-encoding correctly.
```

---

### Task 3: Platform-gate shell execution

**Why:** `execute_shell_command` spawns `sh -c <command>`. Windows doesn't have `sh` — it needs `cmd.exe /C`.

**Files:**
- Modify: `src/tools/workflow.rs:193-197`
- Test: `src/tools/workflow.rs` (existing tests)

**Step 1: Write a cross-platform test**

Add to `tests` module in `src/tools/workflow.rs`:

```rust
#[tokio::test]
async fn execute_shell_command_echo_cross_platform() {
    let (_dir, ctx) = project_ctx().await;
    // "echo hello" works on both sh and cmd.exe
    let result = ExecuteShellCommand
        .call(json!({ "command": "echo hello", "timeout_secs": 5 }), &ctx)
        .await
        .unwrap();
    let stdout = result["stdout"].as_str().unwrap();
    assert!(stdout.contains("hello"), "stdout should contain 'hello': {}", stdout);
}
```

**Step 2: Run to confirm it passes on Linux (baseline)**

Run: `cargo test execute_shell_command_echo_cross_platform -- --nocapture`
Expected: PASS

**Step 3: Replace the hardcoded `sh -c` with platform-conditional code**

In `src/tools/workflow.rs`, replace the `Command::new("sh")` block (around line 193):

```rust
#[cfg(unix)]
let child = tokio::process::Command::new("sh")
    .arg("-c")
    .arg(command)
    .current_dir(&root)
    .output();

#[cfg(windows)]
let child = tokio::process::Command::new("cmd")
    .arg("/C")
    .arg(command)
    .current_dir(&root)
    .output();
```

**Step 4: Run all workflow tests**

Run: `cargo test workflow -- --nocapture`
Expected: All PASS (Linux uses the `sh` branch as before)

**Step 5: Fix test that uses Unix-only commands**

The `execute_shell_command_output_truncated` test uses `seq 1 100000` which doesn't exist on Windows. Guard it:

```rust
#[cfg(unix)]
#[tokio::test]
async fn execute_shell_command_output_truncated() {
    // ... existing test unchanged ...
}
```

Add a Windows-compatible variant if desired (using `for /L` or PowerShell), or skip for now.

Similarly, `execute_shell_command_exit_code_preserved` uses `exit 42` which works in both `sh` and `cmd.exe` — no change needed.

**Step 6: Run all tests**

Run: `cargo test -- --nocapture`
Expected: All PASS

**Step 7: Commit**

```
feat: support Windows cmd.exe in execute_shell_command

Uses cfg(unix)/cfg(windows) to select sh -c vs cmd /C.
Guards Unix-specific test commands behind cfg(unix).
```

---

### Task 4: Normalize path separators in embedding index

**Why:** On Windows, `path.strip_prefix().to_string_lossy()` produces backslash-separated relative paths like `src\tools\file.rs`. These should be stored as forward-slash paths in SQLite for consistency and cross-platform portability.

**Files:**
- Modify: `src/embed/index.rs:270` (in `build_index`)
- Test: `src/embed/index.rs` (existing tests module)

**Step 1: Write the test**

Add to the `tests` module in `src/embed/index.rs`:

```rust
#[test]
fn normalize_rel_path_uses_forward_slashes() {
    // Simulate what build_index does: strip prefix + to_string_lossy
    let root = PathBuf::from(if cfg!(windows) { "C:\\project" } else { "/project" });
    let file = root.join("src").join("tools").join("file.rs");
    let rel = file
        .strip_prefix(&root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    assert_eq!(rel, "src/tools/file.rs");
}
```

**Step 2: Run to confirm it passes**

Run: `cargo test normalize_rel_path -- --nocapture`
Expected: PASS (the `.replace('\\', "/")` is the fix itself)

**Step 3: Apply the fix in `build_index`**

In `src/embed/index.rs`, around line 270, change:

```rust
let rel = path
    .strip_prefix(project_root)?
    .to_string_lossy()
    .replace('\\', "/");
```

**Step 4: Run all embed tests**

Run: `cargo test embed -- --nocapture`
Expected: All PASS

**Step 5: Commit**

```
fix: normalize path separators in embedding index

Stored paths now always use forward slashes regardless of OS,
ensuring index portability and consistent semantic search results.
```

---

### Task 5: Add platform-specific deny-list paths

**Why:** The deny-list currently has `/etc/shadow` and `/etc/gshadow` which only exist on Linux. macOS uses `/etc/master.passwd`. Windows has sensitive paths in `%SYSTEMROOT%\System32\config\`.

**Files:**
- Modify: `src/util/path_security.rs:23-25` (the `DEFAULT_DENIED_EXACT` constant)
- Modify: `src/util/path_security.rs:84-100` (the `denied_read_paths` function)
- Test: `src/util/path_security.rs` (tests module)

**Step 1: Replace the platform-hardcoded constant with `cfg` blocks**

Replace `DEFAULT_DENIED_EXACT` at line 23:

```rust
#[cfg(target_os = "linux")]
const DEFAULT_DENIED_EXACT: &[&str] = &["/etc/shadow", "/etc/gshadow"];

#[cfg(target_os = "macos")]
const DEFAULT_DENIED_EXACT: &[&str] = &["/etc/master.passwd"];

#[cfg(windows)]
const DEFAULT_DENIED_EXACT: &[&str] = &[];
```

**Step 2: Add Windows-specific denied paths via env-var expansion**

For Windows, sensitive paths like `C:\Windows\System32\config\SAM` aren't under `~` so `expand_home` won't help. Add a platform helper to `denied_read_paths`:

```rust
fn denied_read_paths(config: &PathSecurityConfig) -> Vec<PathBuf> {
    let mut denied = Vec::new();
    for p in DEFAULT_DENIED_PREFIXES
        .iter()
        .chain(DEFAULT_DENIED_EXACT.iter())
    {
        if let Some(expanded) = expand_home(p) {
            denied.push(expanded);
        }
    }
    // Windows-specific system paths
    #[cfg(windows)]
    {
        if let Ok(sysroot) = std::env::var("SYSTEMROOT") {
            denied.push(PathBuf::from(&sysroot).join("System32").join("config"));
        }
    }
    for p in &config.denied_read_patterns {
        if let Some(expanded) = expand_home(p) {
            denied.push(expanded);
        }
    }
    denied
}
```

**Step 3: Fix the `etc_shadow_denied` test to be platform-aware**

The existing test directly checks `/etc/shadow`. Make it platform-conditional:

Find the test that asserts `/etc/shadow` is denied and wrap it:

```rust
#[cfg(target_os = "linux")]
#[test]
fn etc_shadow_denied() {
    let result = validate_read_path("/etc/shadow", None, &default_config());
    assert!(result.is_err());
}
```

**Step 4: Run tests**

Run: `cargo test path_security -- --nocapture`
Expected: All PASS

**Step 5: Commit**

```
feat: platform-specific deny-list for path security

Linux: /etc/shadow, /etc/gshadow
macOS: /etc/master.passwd
Windows: %SYSTEMROOT%\System32\config
Home-relative paths (~/.ssh etc.) work on all platforms.
```

---

### Task 6: Fix hardcoded Unix paths in tests

**Why:** Several tests use `/tmp/...`, `/home/user/...`, and `/etc/shadow` literally in assertions. These fail on Windows where these paths don't exist or have different structure.

**Files:**
- Modify: `src/tools/file.rs` (test module)
- Modify: `src/tools/symbol.rs` (test module)
- Modify: `src/lsp/client.rs` (test module)
- Modify: `src/util/path_security.rs` (test module)

**Step 1: Audit and fix `src/tools/symbol.rs` tests**

The `uri_to_path_strips_prefix` test was already replaced in Task 2. Verify no other tests use hardcoded Unix paths:

Search: `grep -n '"/tmp\|"/home\|"/etc' src/tools/symbol.rs`

For any remaining matches in the test module, either:
- Use `tempfile::tempdir()` for real paths
- Guard with `#[cfg(unix)]`

**Step 2: Fix `src/tools/file.rs` tests**

Tests using `/tmp/x` and `/tmp/test.txt` as out-of-project paths for security validation. These should work because the tests verify rejection of arbitrary paths. On Windows, `/tmp/x` is still not under the project root, so the assertion may still hold. **Verify by reading the test logic** — if the test creates a temp project dir and checks that `/tmp/x` is rejected as "outside project", this works on Linux but on Windows `/tmp/x` isn't valid.

Fix: Use `std::env::temp_dir().join("x")` instead of literal `/tmp/x`:

```rust
let outside = std::env::temp_dir().join("nonexistent_file.txt");
let outside_str = outside.to_str().unwrap();
// Use outside_str instead of "/tmp/x"
```

**Step 3: Fix `src/lsp/client.rs` tests**

Tests at lines 814 and 839 use `"/tmp/test.rs"` and `"file:///tmp/test.rb"`. These are unit tests for URI parsing, not filesystem operations. Guard the Unix-specific ones and add cross-platform equivalents:

```rust
#[cfg(unix)]
#[test]
fn uri_to_path_unix() {
    let uri: Uri = "file:///tmp/test.rb".parse().unwrap();
    let path = uri_to_path(&uri);
    assert_eq!(path, PathBuf::from("/tmp/test.rb"));
}
```

**Step 4: Fix `src/util/path_security.rs` tests**

- `etc_shadow_denied`: Already fixed in Task 5
- `custom_denied_pattern`: Uses `/tmp/secret` — use `std::env::temp_dir()` instead
- `write_outside_project_denied`: Uses `../../../tmp/evil.rs` — works on any platform since it's relative path traversal
- Symlink tests: Already `#[cfg(unix)]` — no changes needed

**Step 5: Run all tests**

Run: `cargo test -- --nocapture`
Expected: All PASS

**Step 6: Commit**

```
fix: replace hardcoded Unix paths in tests with portable alternatives

Tests now use tempfile::tempdir() and std::env::temp_dir() instead
of /tmp/... literals. Unix-specific URI tests guarded with cfg(unix).
```

---

### Task 7: Add macOS + Windows CI matrix

**Why:** The current CI only runs on `ubuntu-latest`. Adding macOS and Windows catches platform regressions automatically.

**Files:**
- Modify: `.github/workflows/ci.yml`

**Step 1: Update the `test` job to use a platform matrix**

Replace the `test` job in `.github/workflows/ci.yml`:

```yaml
  test:
    name: Test (${{ matrix.os }} / ${{ matrix.name }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
        include:
          - name: default
            flags: ""
          - name: local-embed
            flags: "--features local-embed --no-default-features"
          - name: no-features
            flags: "--no-default-features"
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.os }}-${{ matrix.name }}
      - run: cargo test ${{ matrix.flags }}
```

Note: The `matrix` here creates os × include combinations. If the full 3×3 matrix is too many CI minutes, start with just `os: [ubuntu-latest, macos-latest]` and add Windows once all tests pass locally.

**Step 2: Keep `fmt`, `clippy`, and `msrv` on Linux only**

These are platform-independent checks — no need to triple the CI time for formatting validation.

**Step 3: Commit**

```
ci: add macOS and Windows to test matrix

Tests now run on all three platforms. Format, clippy, and MSRV
checks remain Linux-only since they're platform-independent.
```

---

### Task 8: Final cross-platform verification

**Step 1: Run full check locally**

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

Expected: All PASS, no warnings.

**Step 2: Verify no remaining platform assumptions**

Search for any remaining Unix-isms that slipped through:

```bash
# Should return 0 results in src/ (excluding tests already cfg-gated)
grep -rn '"/tmp/' src/ --include='*.rs' | grep -v '#\[cfg(unix)\]' | grep -v '// Windows'
grep -rn 'env::var("HOME")' src/ --include='*.rs'
grep -rn 'Command::new("sh")' src/ --include='*.rs' | grep -v '#\[cfg(unix)\]'
```

**Step 3: Commit any remaining fixes**

```
chore: final cross-platform cleanup
```

---

## Appendix: Items NOT addressed (post-v1)

These are low-risk or low-priority items that don't block cross-platform support:

1. **`fastembed` on Windows/macOS** — The ONNX Runtime dependency may have platform-specific build requirements. It's behind a feature flag (`local-embed`) and not the default. Test separately.

2. **LSP server availability** — The `default_config` in `src/lsp/servers/mod.rs` uses the same binary names on all platforms (e.g., `rust-analyzer`, `pyright-langserver`). These happen to be correct since the tools use the same names on Windows/macOS when installed via standard package managers. No code change needed, but documentation should note installation requirements per platform.

3. **Case-insensitive filesystems** — macOS default (HFS+/APFS) is case-insensitive. This could cause subtle issues with glob matching or path comparison. Deferred — no known bugs.

4. **Windows UNC paths** — Paths like `\\server\share\...` are not tested. The `url` crate handles them, but no test coverage. Low priority since MCP servers typically run locally.

5. **Windows long path support** — Paths > 260 chars. Rust's `std::path` handles this on modern Windows (with manifest). Not an immediate concern.
