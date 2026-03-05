# TODO: Code Review Findings (2026-03-05)

Review of 96 commits across 8 workstreams (2026-03-03 to 2026-03-05).
Full analysis in `docs/review-2026-03-05.md`.

---

## Must-Fix

- [x] **MF-1: `extract_title` panics on multi-byte UTF-8 at byte 80**
  `src/tools/memory.rs:218-230` — `content[..end]` can split a multi-byte char.
  Same class as BUG-024. Use `floor_char_boundary` from `src/tools/mod.rs:185`
  or extract a shared `safe_truncate(s, max_bytes)` utility.

- [x] **MF-2: `search_memories` bucket filter post-filters KNN results**
  `src/embed/index.rs:340-346` — `WHERE m.bucket = ?3` runs after KNN top-k,
  giving 0-5 results instead of best matches in target bucket. Over-fetch in
  inner query (e.g. `limit * 4`) then re-limit in outer query.

- [x] **MF-3: `ensure_vec_memories` no-ops on fresh databases, `remember` crashes**
  `src/embed/index.rs:242-245` — When `embedding_dims` is absent, returns
  `Ok(())` without creating `vec_memories`. Then `insert_memory` hits "no such
  table". Either return `RecoverableError` with hint to run `index_project`, or
  store memory without embedding and skip `vec_memories` insert.

## Should-Fix

- [x] **SF-1: `GithubRepo::create` and `fork` pass `--json` without field names**
  `src/tools/github.rs` (~L1101) — `gh repo create name --json` needs field
  list like `name,url,description,visibility`. Without it, `gh` may error.

- [x] **SF-2: `push_files` vs `create_or_update` encoding inconsistency**
  `src/tools/github.rs` (GithubFile) — `create_or_update` says "base64-encoded"
  but `push_files` passes plaintext via Trees API. Align descriptions or handle
  encoding internally.

- [x] **SF-3: Tmpfile path is predictable (symlink risk)**
  `src/tools/workflow.rs:897` — `/tmp/codescout-unfiltered-{nanos:016x}` uses
  nanosecond timestamp only. Use `tempfile::NamedTempFile` instead for
  atomic creation with random names.

- [x] **SF-4: Tmpfile path not shell-escaped in tee injection**
  `src/tools/workflow.rs:898-903` — Path interpolated directly into shell
  command via `format!`. Safe today (hex-only path) but fragile. Apply
  `shell_escape::escape()` or document the invariant.

- [x] **SF-5: Partial line range silently ignored in `read_file`**
  `src/tools/file.rs` (~L330) — `read_file(path, start_line=10)` without
  `end_line` falls through to exploring-mode cap with no error. Return
  `RecoverableError` requiring both params.

## Suggestions

- [ ] **SG-1: Remove dead code from tool restructure**
  `memory.rs`, `ast.rs`, `semantic.rs`, `git.rs`, `library.rs`, `usage.rs` —
  10 unregistered tool structs still `pub`. `tests/integration.rs:174` still
  imports old API. Remove or mark `pub(crate)`/`#[deprecated]`.

- [x] **SG-2: Capture original error instead of re-running `validate_symbol_range`**
  `src/tools/symbol.rs:833` — Re-runs validation purely for error message
  reconstruction. TOCTOU gap if file changes between calls. Store the original
  error and propagate directly.

- [x] **SG-3: Extract `require_number` helper in GitHub tools**
  `src/tools/github.rs` — `number.as_deref().ok_or_else(...)` pattern repeated
  ~25 times. Extract a one-liner helper.

- [x] **SG-4: `delete_memory`/`forget` silently succeeds on nonexistent IDs**
  `src/embed/index.rs:352-356` — No `changes()` check after DELETE. Return
  `RecoverableError` if zero rows affected.

- [x] **SG-5: CWD test uses process-global `set_current_dir`**
  `src/util/path_security.rs:679-691` — Affects entire process during parallel
  tests. Add `#[serial]` or restructure to avoid changing process CWD.

- [x] **SG-6: `detect_terminal_filter` undocumented `|&` behavior**
  `src/tools/command_summary.rs` — Bash `|&` falls through to `None` correctly
  but is untested. Add test case with comment.

- [x] **SG-7: Drift temp table not cleaned up on error path**
  `src/embed/drift.rs:146` — `DROP TABLE` only runs after query loop. If loop
  fails with `?`, temp table leaks. Use `scopeguard`/`defer!`.

- [x] **SG-8: `ProjectStatus` drift items use `json!({})` not ordered maps**
  `src/tools/config.rs` (~L183) — Inconsistent with `symbol_to_json` and other
  code that switched to `Map::new()` for field ordering.

- [x] **SG-9: `command_summary.rs` not in CLAUDE.md project structure**
  `CLAUDE.md` — New file not listed. Add entry under `src/tools/`.

- [ ] **SG-10: `as_u64_lenient` should be a shared utility**
  `src/tools/file.rs:12-15` — Handles MCP integer-as-string quirk. Move to
  shared location when a second use site appears.

## Cross-Cutting Recommendations

- [x] **CC-1: Create shared `safe_truncate(s, max_bytes) -> &str` utility**
  Three UTF-8 boundary issues surfaced: BUG-024 (`truncate_compact`), MF-1
  (`extract_title`), and `pipe_pos` slice (safe but undocumented). Centralize
  in `src/tools/mod.rs` or `src/util/`.

- [ ] **CC-2: Document re-buffering anti-pattern in progressive discoverability**
  `docs/PROGRESSIVE_DISCOVERABILITY.md` — The `call_content` auto-buffering
  conflicts with tools needing large inline responses. Scattered fixes in
  `read_file` (proactive buffering), `run_command` (byte budget), `onboarding`
  (custom override). Document as "tools producing large output" section.

- [ ] **CC-3: Clean up dual API surface from tool restructure**
  Schedule sprint to migrate tests from old structs (`WriteMemory.call(...)`)
  to new consolidated APIs (`Memory.call(json!({"action":"write"}))`), then
  remove or `pub(crate)` the 10 old tool structs.
