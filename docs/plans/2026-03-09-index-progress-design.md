# Design: index_project Progress & ETA

**Date:** 2026-03-09  
**Status:** Approved  
**Scope:** Lightweight heuristic progress reporting for `index_project`, surfaced via `index_status`.

## Problem

`index_project` runs in the background and immediately returns `{"status":"started"}`. The only way to know it finished was to poll `index_status`, which returned `{"indexing":"running"}` — no indication of how many files remain or how long until completion.

## Goal

When polling `index_status` during an active index run, the LLM sees how many files have been embedded and a heuristic ETA — without adding any computational overhead.

## Design

### 1. `IndexingState::Running` — struct variant

**File:** `src/agent.rs`

```rust
Running {
    done: usize,
    total: usize,
    eta_secs: Option<u64>,
},
```

- `done=0, total=0` on entry (Phase 1 — file walk — is still running)
- `total` becomes non-zero once `build_index` has resolved the work list
- `eta_secs: None` until the first file completes (can't estimate with zero elapsed)
- `eta_secs: None` when `done == total` (remaining is 0; DB commit phase still follows)

### 2. `build_index` — progress callback

**File:** `src/embed/index.rs`

New signature:

```rust
pub async fn build_index(
    project_root: &Path,
    force: bool,
    progress_cb: Option<Box<dyn Fn(usize, usize, Option<u64>) + Send>>,
) -> Result<IndexReport>
```

Callback fires after each `join_next()` completion in Phase 2:

```rust
let total_to_embed = works.len();
let embed_start = std::time::Instant::now();
let mut done = 0usize;

while let Some(res) = tasks.join_next().await {
    results.push(res.map_err(|e| anyhow::anyhow!(e))??);
    done += 1;
    if let Some(cb) = &progress_cb {
        let remaining = total_to_embed - done;
        let eta_secs = (done > 0 && remaining > 0).then(|| {
            let elapsed = embed_start.elapsed().as_secs_f64();
            (elapsed / done as f64 * remaining as f64) as u64
        });
        cb(done, total_to_embed, eta_secs);
    }
}
```

ETA algorithm: simple moving average — `(elapsed / done) * remaining`. Two float ops per file, self-correcting as more completions arrive. Cost: one `Instant::now()` capture + trivial arithmetic per file.

`build_library_index` is unchanged (separate function, not in scope).

### 3. `index_project::call` — wiring

**File:** `src/tools/semantic.rs`

Initial state set on entry:

```rust
*state = IndexingState::Running { done: 0, total: 0, eta_secs: None };
```

Callback built before spawning the background task:

```rust
let state_arc_cb = ctx.agent.indexing.clone();
let progress_cb: Option<Box<dyn Fn(usize, usize, Option<u64>) + Send>> =
    Some(Box::new(move |done, total, eta_secs| {
        let mut s = state_arc_cb.lock().unwrap_or_else(|e| e.into_inner());
        *s = IndexingState::Running { done, total, eta_secs };
    }));
```

The existing MCP progress notifications (step 0 at start, step 1/1 at end via `ctx.progress`) are untouched.

### 4. `index_status` — rendering

**File:** `src/tools/semantic.rs`

```rust
IndexingState::Running { done, total, eta_secs } => {
    result["indexing"] = json!({
        "status": "running",
        "done": done,
        "total": total,
        "eta_secs": eta_secs,
    });
}
```

Poll output at each stage:

| Phase | `indexing` field |
|---|---|
| Phase 1 (scanning) | `{"status":"running","done":0,"total":0,"eta_secs":null}` |
| Phase 2, mid-run | `{"status":"running","done":23,"total":87,"eta_secs":45}` |
| Phase 2, last file | `{"status":"running","done":87,"total":87,"eta_secs":null}` |
| Complete | `{"status":"done","files_indexed":87,...}` |

`total=0` unambiguously signals "still scanning". `eta_secs:null` on the last file is intentional — "0s remaining" would be misleading since the DB commit phase still follows.

## Files Changed

| File | Change |
|---|---|
| `src/agent.rs` | `Running` → struct variant with `done`, `total`, `eta_secs` |
| `src/embed/index.rs` | `build_index` gains `progress_cb` param; Phase 2 loop fires it |
| `src/tools/semantic.rs` | `index_project::call` sets initial state + wires callback; `index_status` renders new `Running` fields |

## Out of Scope

- `build_library_index` (separate function, different use pattern)
- MCP progress notification enrichment (already works as start/end bookends)
- Phase 1 timing (walk is fast; `total=0` is sufficient signal)
