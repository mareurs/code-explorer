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

### A. Better error when `initialize` hangs

**Status: 🔎 UNDER REVIEW** — Occurred consistently early on (2026-02-28) when competing
kotlin-lsp instances were present. Has not reproduced since cleanup. May only trigger when
IntelliJ or a stale process holds the workspace lock. Monitor before investing in a fix.

The 300s `init_timeout` becomes a 5-minute user-visible hang when the workspace is locked.
If it recurs, options:
1. Shorten `init_timeout` for kotlin-lsp specifically (e.g. 60s)
2. Detect if another kotlin-lsp is running and surface a clear error immediately
3. Use `fuser`/`lsof` to detect the lock before spawning

### D. Error surfacing for -32800 cancellations

The -32800 hint is inside the JSON body of a `RecoverableError`. If the hang (item A) is
the more common failure mode, the hint may never be shown. The real UX problem is the
300s timeout with no user feedback.

---

## Related code

- `src/lsp/client.rs` — `request()` (retry scaffold), `did_open()` (duplicate guard), `did_close()`
- `src/lsp/servers/mod.rs` — kotlin `init_timeout: jvm_timeout` (300s)
