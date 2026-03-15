# Tool Misbehaviors — Observed in the Wild

This is a living document. **Update it whenever you observe unexpected, wrong, or dangerous
behavior from codescout's own tools while working on the codebase.** Each entry should
capture: what you did, what you expected, what actually happened, and a reproduction hint.

---

## Prompt for future sessions

> Before starting any task on this codebase, re-read this file. While you work, watch for
> unexpected tool behavior: wrong edits, corrupt output, silent failures, misleading errors.
> When you find something, **add an entry here before continuing** — even a one-liner is
> enough to capture it while it's fresh. The goal is to build a corpus of real failure cases
> to drive test and UX improvements.

---

## Observed Bugs

### BUG-021 — `edit_file`: parallel calls cause partial state + MCP server "crash"

**Date:** 2026-03-03
**Severity:** High — leaves files in inconsistent partial state; server exit requires `/mcp` restart
**Status:** 🔍 ROOT CAUSE IDENTIFIED (2026-03-03) — two independent issues, one fixable

**What happened:**
Dispatched two `edit_file` calls in the same parallel response (targeting two different source
files: `src/embed/local.rs` and `src/config/project.rs`). The Claude Code permission system
handles each call independently: the first call was approved and returned `"ok"` (edit applied
to `local.rs`); the second call was rejected by the user and returned an error. This left the
two files in an inconsistent state — one edited, one not. Immediately after, the codescout
MCP server crashed and became unavailable, requiring a manual `/mcp` reconnect.

**Reproduction hint:**
1. Dispatch two `edit_file` tool calls in a single parallel response to different source files.
2. Approve the first permission prompt, reject (or let timeout) the second.
3. Observe: first file edited, second file unchanged — inconsistent partial state.
4. codescout MCP server crashes; subsequent tool calls fail until `/mcp` restart.

**Root cause (investigated 2026-03-03 — two separate issues):**

**Issue A — Partial state: inherent to independent parallel writes.**
When two `edit_file` calls target different files, they run as independent `tokio::spawn` tasks
inside rmcp's `serve_inner`. There is no transaction semantics across them. If one is denied
(permission dialog) while the other succeeds, the files are left in a partially-applied state.
This is NOT a bug in our code — it's the correct behavior for two independent operations. The
fix is operational: never dispatch parallel write tool calls.

**Issue B — "Crash" is actually Claude Code closing the stdio pipe (rmcp cancellation race).**
Static analysis of the full code path confirms there are NO panic paths in our production code
that could crash the server:
- All `lock().unwrap()` calls in the hot path (`open_files`, `OutputBuffer`) have trivial
  critical sections (HashSet ops only) — mutex cannot be poisoned by normal use.
- `call_tool_inner` routes ALL errors through `route_tool_error`; no unhandled panics.
- rmcp 0.1.5 spawns each request as `tokio::spawn` with the JoinHandle **dropped** — task
  panics are absorbed by the detached task and never propagate to the serve loop.
- The serve loop in `serve_inner` has no `unwrap()`/`expect()` in its event handler.

The "crash" is the server process exiting cleanly after the **stdio pipe closes**. This maps to
`service.waiting()` returning `QuitReason::Closed` → error propagates via `?` in `run()`.

**Why does Claude Code close the pipe?** Most likely a cancellation race in rmcp 0.1.5:
When Claude Code denies a parallel call, it may send a `notifications/cancelled` for the
in-flight request. rmcp cancels the `CancellationToken` but the spawned task has **no check**
for `context.ct.is_cancelled()` — it runs to completion and sends a response back through
`sink_proxy_tx`. The main loop then writes that response to stdout. Claude Code receives an
unexpected response for an already-cancelled request ID, which may cause it to close the
connection (a Claude Code MCP client bug, not ours).

**Fix:**
- **Operational** (immediate): never dispatch parallel write tool calls. Always finish one
  `edit_file` / `replace_symbol` / `insert_code` / `create_file` before starting the next.
- **rmcp limitation**: rmcp 0.1.5 does not suppress responses for cancelled requests.
  This cannot be fixed in our code without forking rmcp. Upgrading rmcp if a newer version
  respects cancellation tokens in the task-spawn path would help.
- **Defence-in-depth** (applied): `[profile.release] panic = "abort"` in Cargo.toml ensures
  any future panic kills the process cleanly rather than leaving a zombie server.

---

## Template for new entries

```
### BUG-XXX — `<tool name>`: <one-line description>

**Date:** YYYY-MM-DD
**Severity:** Low / Medium / High
**Status:** Open

**What happened:**
<what you did, what you expected, what happened instead>

**Reproduction hint:**
<minimal steps or context to reproduce>

**Root cause hypothesis:**
<your best guess at why it happened>

**Fix ideas:**
<options for fixing it in the tool or in its UX>

---
```
