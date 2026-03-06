# Pre-Onboarding Index Gate

**Date:** 2026-03-06
**Status:** Approved

## Problem

Onboarding currently tells the LLM "Do NOT run `index_project` during onboarding"
and falls back to `search_pattern` for concept exploration (Step 6). This means
most onboardings miss the semantic search capability entirely, producing shallower
exploration and a system prompt that references `semantic_search` without the user
ever having built the index.

## Decisions

| # | Question | Choice |
|---|----------|--------|
| 1 | Where does the gate live? | **(C) Hybrid** — Rust probes index status, prompt handles UX |
| 2 | What happens when index is missing? | **(C) Recommend but allow skip** — user chooses: build now, build from CLI, or skip |
| 3 | Do exploration steps change? | **(A) Same steps, different tools** — Step 6 uses `semantic_search` or `search_pattern` |
| 4 | Does skip affect system prompt? | **(A) Silent** — system prompt always references full capabilities |

## Design

### Rust: `Onboarding::call` (`src/tools/workflow.rs`)

After `gather_project_context(&root)`, probe the embedding index:

```rust
let index_status = {
    let db_path = root.join(".codescout").join("embeddings.db");
    if db_path.exists() {
        match crate::embed::index::open_db(&root)
            .and_then(|conn| crate::embed::index::index_stats(&conn))
        {
            Ok(stats) => json!({
                "ready": stats.chunk_count > 0,
                "files": stats.file_count,
                "chunks": stats.chunk_count,
            }),
            Err(_) => json!({ "ready": false, "files": 0, "chunks": 0 }),
        }
    } else {
        json!({ "ready": false, "files": 0, "chunks": 0 })
    }
};
```

Add `"index_status": index_status` to the response JSON.

### Prompt: `src/prompts/onboarding_prompt.md`

Insert **Phase 0: Semantic Index Check** between the Iron Law and Phase 1.

- If `index_status.ready` is true → announce index is available, proceed.
- If false → present three options to the user:
  1. **Build now** — call `index_project`, poll `index_status` every 15s
  2. **Build from CLI** — `codescout index --project .` in another terminal
  3. **Skip** — proceed with `search_pattern` fallback
- Wait for user choice before continuing.
- If build fails, fall back to option 3.

### `build_onboarding_prompt` (`src/prompts/mod.rs`)

Add `index_ready: bool`, `index_files: usize`, `index_chunks: usize` parameters.
Inject into the Gathered Project Data section:

```
- Semantic index: ready (142 files, 891 chunks)
- Semantic index: not built
```

### Testing

- Assert `onboarding_prompt` contains "Phase 0" / "Semantic Index Check"
- Test `build_onboarding_prompt` renders index status for both ready/not-ready
- Verify `Onboarding::call` JSON includes `index_status` field

## Files Changed

| File | Change | ~Lines |
|------|--------|--------|
| `src/tools/workflow.rs` | Index probe + JSON field | ~15 |
| `src/prompts/onboarding_prompt.md` | Phase 0 section | ~35 |
| `src/prompts/mod.rs` | New params + gathered data injection | ~10 |
| `src/prompts/mod.rs` (tests) | Phase 0 + index status assertions | ~15 |

**Total: ~75 lines across 3 files.**

## Not Changed

- `server_instructions.md` — no impact
- `build_system_prompt_draft` — always references `semantic_search` (decision #4)
- `semantic.rs` — embedding pipeline untouched
- `config/project.rs` — `indexing_enabled` already exists as opt-out
