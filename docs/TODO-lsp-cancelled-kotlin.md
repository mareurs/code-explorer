# LSP issues on kotlin-lsp

## Confirmed root cause (2026-02-28)

**Competing kotlin-lsp instances holding the workspace database.**

kotlin-lsp uses a workspace-scoped on-disk database that only one process can own
at a time. When another instance holds the lock (IDE open, stale terminal session,
orphaned process from a previous codescout run), a new instance either:

- **Hangs indefinitely during `initialize`** — never sends back an LSP
  `InitializeResult`, so codescout sits at the 300s JVM `init_timeout`
- **Returns -32800 "cancelled"** — server starts but immediately cancels all
  requests because the database is locked

### Test results (2026-02-28)

| Condition | Result |
|-----------|--------|
| Two competing kotlin-lsp processes running | `initialize` hangs, no response |
| No competing processes | `initialize` in 13s, `list_symbols` works correctly |

**Environment during testing:**
- IntelliJ IDEA running (since feb26)
- PID 1144325: stale kotlin-lsp for `backend-kotlin` (running since 15:43, binary upgraded/deleted)
- PID 1582490: kotlin-lsp for `backend-kotlin-clone`
- After killing both + removing the community `kotlin-language-server` binary: **works**.

### Cleanup performed
- Killed competing kotlin-lsp instances (PIDs 1144325, 1582490)
- Removed community `kotlin-language-server` binary and lib dir from `~/.local/`
  (was never used by codescout, caused confusion)

---

## Implemented (pre-investigation)

### 1. Duplicate `didOpen` guard (`src/lsp/client.rs`)

Added `open_files: StdMutex<HashSet<PathBuf>>` to `LspClient`.
`did_open()` silently no-ops if the file is already tracked as open.

**Rationale**: The LSP spec prohibits sending `textDocument/didOpen` for an
already-open document without an intervening `didClose`. Some servers error or
cancel on duplicate opens.

**Known gap — fixed (2026-02-28)**: `did_close()` now removes the path from
`open_files` before sending the notification, so the guard resets correctly on
close/reopen cycles.

### 2. Retry scaffold in `request()` (`src/lsp/client.rs`)

```rust
const RETRY_ON_CANCELLED: bool = false;  // disabled
const MAX_RETRIES: usize = 3;
const RETRY_DELAY_MS: u64 = 300;
```

**Decision after investigation**: keep disabled. The lock-conflict failure is
structural (locked DB), not transient — retrying just delays the timeout by
`RETRY_DELAY_MS × attempt`. No evidence of transient -32800 on a healthy server.
Consider removing the dead code unless a future server is found to produce
transient cancellations.

---

## Remaining open items

### A. Better error when `initialize` hangs (highest priority)

The 300s `init_timeout` becomes a 5-minute user-visible hang when the workspace
is locked. Options:
1. Shorten `init_timeout` for kotlin-lsp specifically (e.g. 60s)
2. Detect if another kotlin-lsp is running for the same project and surface a
   clear error immediately: "Another kotlin-lsp instance is already running for
   this workspace. Close IntelliJ/VS Code or kill the existing process."
3. Use `fuser`/`lsof` to detect the lock before spawning

### B. `did_close()` missing `open_files` removal

See above. One-liner fix.

### C. Startup delay — not yet measured on first cold run

On the test machine, `initialize` took 13s with a warm disk cache. First run
(no cached index) may be longer. Worth measuring once to set user expectations.

### D. Error surfacing

The -32800 hint is inside the JSON body of a `RecoverableError`. If the hang is
the more common failure mode (not -32800), the hint may never be shown. The real
UX problem is the timeout with no feedback — address via item A above.

---

## Related code

- `src/lsp/client.rs` — `request()` (retry scaffold), `did_open()` (duplicate guard), `did_close()` (missing removal)
- `src/lsp/servers/mod.rs` — kotlin `init_timeout: jvm_timeout` (300s)
- `src/prompts/server_instructions.md` — may need a note about the workspace lock issue
