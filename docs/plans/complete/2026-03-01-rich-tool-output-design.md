# Rich Tool Output — Progress Notifications, Compact Summaries, ANSI Diff Viewer

**Date:** 2026-03-01
**Status:** Approved
**Scope:** All 31 tools — phased across three independent layers

## Problem

Tool results today are verbose JSON blobs or walls of text. Two issues:
1. **Slow tools give no feedback while running** — the spinner sits there silently; users don't know if indexing is progressing or hung.
2. **Write tool results are either silent (`"ok"`) or noisy** — no sense of what changed, but also no compact human-readable summary.

The goal: every tool has a single terse summary line, long tools stream progress while running, and writer tools show an ANSI-styled diff preview of what changed.

## Approach: Three Independent Layers

### Layer 1 — Progress Notifications (slow tools)
### Layer 2 — Compact 1-Line Summaries (all tools)
### Layer 3 — ANSI Diff Viewer (writer tools)

Layers are independent and can ship in any order. Layer 2 has the widest impact and lowest risk.

---

## Layer 1 — Progress Notification Infrastructure

### MCP Mechanism

The MCP spec's `notifications/progress` lets the server push status updates to the client while a tool is running. Claude Code renders these as live updates in the tool call spinner. The client must include a `progressToken` in `request._meta` for the server to send them.

### ToolContext Changes

Add a new optional `ProgressReporter` to `ToolContext`:

```rust
// src/tools/mod.rs (or src/tools/progress.rs)
pub struct ProgressReporter {
    peer: rmcp::service::ServerPeer,
    token: ProgressToken,
}

impl ProgressReporter {
    /// Send a progress update. Errors are silently swallowed — progress is best-effort.
    pub async fn report(&self, step: u32, total: Option<u32>) {
        let _ = self.peer.notify_progress(ProgressNotificationParam {
            progress_token: self.token.clone(),
            progress: step,
            total,
        }).await;
    }
}
```

`ToolContext` gains:
```rust
pub progress: Option<Arc<ProgressReporter>>,
```

### server.rs Changes

In `call_tool`, extract the progress token from `req.params._meta.progressToken`. If present, construct the reporter and inject it into `ToolContext`. Tools that don't report progress ignore the `None`.

### Tool Usage Pattern (opt-in)

```rust
// Tools call ctx.progress.as_ref() — no-op if None
if let Some(p) = &ctx.progress {
    p.report(0, Some(file_count as u32)).await;
}
for (i, file) in files.iter().enumerate() {
    process(file).await?;
    if let Some(p) = &ctx.progress {
        p.report(i as u32 + 1, Some(file_count as u32)).await;
    }
}
```

### Tools That Get Progress (First Pass)

| Tool | Progress scheme |
|---|---|
| `index_project` | Per-file-batch: `(batch, total_files)` |
| `index_library` | Same as `index_project` |
| `onboarding` | Per-step: "reading README", "scanning languages", "loading config" |
| `run_command` | Time-based heartbeat every 2s: `(elapsed_secs, None)` |
| `goto_definition` | "waiting for LSP…" if LSP is still initializing |
| `hover` | Same as `goto_definition` |

---

## Layer 2 — Compact 1-Line Summaries (All Tools)

### Format Convention

```
tool_name(key_param) → outcome  [metadata]
```

- Summary line contains **no ANSI** — it is LLM-clean and human-readable
- `·` separates metadata chunks
- `[brackets]` enclose contextual labels (language, type, status)
- For write tools, this replaces the current `json!("ok")` return

### Summary Lines by Category

**Read tools:**
```
read_file(src/main.rs)            → 312 lines [Rust] · 8 symbols
list_dir(src/tools/)              → 12 files · 4 dirs
list_symbols(src/server.rs)       → 15 symbols [Rust]
search_pattern("TODO")            → 7 matches · 5 files
find_symbol("OutputGuard")        → 3 matches · 2 files
find_file("**/*.rs")              → 47 files
```

**Write tools:**
```
create_file(src/foo.rs)           → created · 47 lines [Rust]
edit_file(src/server.rs)          → replaced L88-92 · -2 +3 lines
replace_symbol(OutputGuard/cap)   → replaced · L124-145 [42 lines]
remove_symbol(tests/old_test)     → removed · L201-215 [14 lines]
```

**Run:**
```
run_command(cargo test)           → ✓ exit 0 · 533 passed · 0 failed · 12.4s
run_command(git status)           → 3 modified · 1 untracked
run_command(ls src/)              → ✓ exit 0 · 3 lines
```

**LSP tools:**
```
goto_definition(src/server.rs:88) → src/tools/mod.rs:167 [Tool trait]
hover(src/server.rs:10)           → Arc<CodeExplorerServer>
find_references(Tool/call)        → 12 references · 4 files
rename_symbol(foo → bar)         → 8 sites renamed · 3 files
```

**Semantic / index:**
```
semantic_search("embedding chunk") → 8 results (top: src/embed/chunker.rs:45)
index_project                      → 2,341 chunks · 147 files · 3.2s
index_status                       → indexed · 147 files · drift: 0.03
```

**Memory / config:**
```
write_memory(architecture)         → written · 234 chars
read_memory(architecture)          → 234 chars [last updated: 2026-03-01]
list_memories                      → 7 topics
activate_project(/path)            → activated [Rust · 31 tools]
```

### Implementation Notes

- Simple tools (memory, config, activate_project) return ONLY the summary — no detail section
- Complex tools (find_symbol, list_symbols, search_pattern) keep their existing detail output BELOW the summary
- The summary line is always the first line of the content text

---

## Layer 3 — ANSI Diff Viewer (Writer Tools)

For the 5 writer tools, the compact summary (Layer 2) is followed by an ANSI-styled diff preview. The diff uses **standard unified diff format** — fully readable without colors.

### Targets

`create_file`, `edit_file`, `replace_symbol`, `remove_symbol`, `run_command`

### Format: `create_file`

```
─── create_file: src/tools/foo.rs ──────────────────────────────
+++ new file · 47 lines · Rust

@@ +1,10 @@
+ use anyhow::Result;
+
+ pub struct Foo {
+     inner: String,
+ }
+
+ impl Foo {
+     pub fn new(s: &str) -> Self {
+         Self { inner: s.to_owned() }
+     }
  ···  (37 more lines)
```

### Format: `edit_file` / `replace_symbol`

```
─── edit_file: src/server.rs ───────────────────────────────────

@@ -88,5 +88,3 @@ fn call_tool
     let input: Value = req
-    let old_value = "foo";
-    let also_old = "bar";
+    let new_value = "baz";
     let tool = self.find_tool
```

### Format: `remove_symbol`

```
─── remove_symbol: tests/old_test ──────────────────────────────
--- removed · 14 lines

@@ -201,14 @@
- #[test]
- fn old_test() {
-     assert_eq!(1, 1);
- }
  ···
```

### Format: `run_command`

```
─── run_command: cargo test ─────────────────────────────────────
✓ exit 0 · 12.4s

running 533 tests
test result: ok. 533 passed; 0 failed; 0 ignored
  ···  (47 more lines — query with grep/tail @cmd_xxxx)
```

For failed commands:
```
─── run_command: cargo build ────────────────────────────────────
✗ exit 1 · 3.1s

error[E0308]: mismatched types
  --> src/foo.rs:12:5
   |
12 |     "hello"
   |     ^^^^^^^ expected i32, found &str
  ···  (12 more lines — query with grep/tail @cmd_xxxx)
```

### ANSI Color Scheme

| Element | ANSI |
|---|---|
| `─── header ───` | Bold cyan (`\033[1;36m`) |
| `+++ new file` | Bold green (`\033[1;32m`) |
| `--- removed` | Bold red (`\033[1;31m`) |
| `@@ ... @@` | Dim white (`\033[2m`) |
| `+` diff lines | Green (`\033[32m`) |
| `-` diff lines | Red (`\033[31m`) |
| `✓` / `✗` | Green / Red |
| Context lines | Default (no ANSI) |
| `···` elision | Dim (`\033[2m`) |
| **Summary line** | **NO ANSI** |

### LLM Compatibility

The summary line (Layer 2) is always ANSI-free — the LLM gets clear signal. The diff section uses standard unified diff format; Claude reads unified diffs fluently with or without color codes. ANSI escape sequences are kept minimal and don't appear on the semantic content lines (only on headers and structural markers).

### Elision Strategy for `create_file`

Show first 10 lines of a new file, then `···  (N more lines)`. For edits, show the full diff (typically small). For `run_command`, show first 20 lines of output and reference the `@cmd_xxxx` buffer for the rest.

---

## Rollout Strategy

| Layer | Scope | Risk | Effort |
|---|---|---|---|
| Layer 2 — compact summaries | All 31 tools | Low — pure formatting | Medium |
| Layer 3 — ANSI diff viewer | 5 writer tools | Low — additive | Low |
| Layer 1 — progress infra | ToolContext + 6 tools | Medium — new plumbing | Medium |

**Suggested order:** Layer 2 first (highest impact, lowest risk), then Layer 3 (visible payoff, 5 files), then Layer 1 (infrastructure).

---

## Out of Scope

- **Collapse on next call** — requires client-side Claude Code changes. Noted as a future opportunity.
- **Dual content blocks** (separate human/machine output) — deferred. Current ANSI approach is sufficient.
- **Markdown rendering in tool results** — Claude Code doesn't render markdown in MCP tool results natively; ANSI is the practical alternative.
