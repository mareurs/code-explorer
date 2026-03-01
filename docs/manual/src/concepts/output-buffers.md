# Output Buffers

## The Problem

MCP tools return their results directly into the AI's context window. For large
command output — a full `cargo test` run, a broad `grep`, a 2000-line file — that
means the entire output lands in context whether the AI needs all of it or not.
The result is a bloated context, wasted tokens, and an AI that has to skim walls
of text to find what it actually needs.

## How It Works

When `run_command` or `read_file` produces output above a size threshold,
code-explorer stores the full content in an in-memory buffer and returns a compact
summary + an `@id` handle instead:

```
run_command("cargo test")
→ {
    "summary": "47 passed, 2 failed — FAILED: test_parse, test_render",
    "output_id": "@cmd_a1b2c3",
    "exit_code": 1
  }
```

The full output is held in memory, keyed by the `@id`. The AI can then query it
with targeted follow-up `run_command` calls using standard Unix tools:

```
run_command("grep FAILED @cmd_a1b2c3")
run_command("sed -n '42,80p' @cmd_a1b2c3")
run_command("grep -A5 'thread.*panicked' @cmd_a1b2c3")
```

File reads work the same way — large files become `@file_id` references:

```
read_file("src/main.rs")
→ { "summary": "...", "file_id": "@file_abc456" }

run_command("grep 'fn.*async' @file_abc456")
```

Refs compose freely. You can `diff` two buffers, pipe one through `awk`, or pass
a `@file_id` to `grep` alongside a pattern from a `@cmd_id`:

```
run_command("diff @cmd_a1b2c3 @cmd_d4e5f6")
run_command("grep -F -f @file_abc456 @cmd_a1b2c3")
```

## Why It Matters

**Short output is always returned inline.** Only responses above the threshold
get buffered. The AI never has to think about whether to use `@refs` — the
tool handles the routing automatically.

**Each buffer query shows up as a distinct tool call in Claude Code's UI.**
Instead of one undifferentiated wall of text, the user sees the AI making
targeted, reviewable queries — `grep FAILED`, then `sed -n '42,80p'`, then
`grep -A5 'panicked'`. The exploration is transparent and auditable.

**The context window stays lean.** The AI holds a reference to large output
without paying the token cost of the full content. It pays only for what it
actually reads.

**Buffers survive across multiple turns.** A `@cmd_id` from a `cargo test` run
can be queried again later in the same session — no need to re-run the command
to look at a different part of the output.

## Buffer Lifecycle

Buffers are held in memory for the lifetime of the MCP server process. They use
an LRU eviction policy: when the buffer store fills up (default: 50 entries),
the least-recently-accessed entry is dropped. Accessing a buffer (even to query
it) refreshes its position in the eviction order.

Buffers are not persisted to disk. Restarting the server clears them.
