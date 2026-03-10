# Hardware-Aware Embedding Model Selection in Onboarding

**Date:** 2026-03-10
**Status:** Approved
**Branch:** experiments

---

## Problem

`onboarding()` always writes `ollama:mxbai-embed-large` to `project.toml` regardless
of what hardware or services the user has available. This is wrong in two ways:

1. **mxbai is a poor default for code.** It is a general text model with a 512-token
   context window (~1,300 chars). Many Rust/Kotlin functions exceed this, causing silent
   truncation and degraded embeddings. Code-specific models (JinaEmbeddingsV2BaseCode)
   and longer-context models (nomic-embed-text: 2048 tok, bge-m3: 8192 tok) produce
   better code retrieval.

2. **No CPU path exists.** Users without Ollama are given a model they cannot run, and
   the local fastembed path (`local:...`) is never surfaced during onboarding.

**Key research finding:** GPU vs CPU does not determine embedding *quality* — it
determines *indexing speed*. A CPU-only user running `local:JinaEmbeddingsV2BaseCode`
(code-specific, 768d, 8192-token context) outperforms a GPU user running
`ollama:mxbai-embed-large` (general, 1024d, 512-token context) on CodeSearchNet
benchmarks. The right split is **Ollama available** (fast inference path) vs
**CPU-only** (local fastembed path).

---

## Design

### Approach: Hardware detection in Rust, confirmation via onboarding prompt

`onboarding()` probes hardware at call time, returns structured results, writes the
recommended model to `project.toml`, and a new **Phase 0.5** section in the onboarding
prompt presents a ranked 3-option menu to the user. The user confirms or picks
an alternative; the LLM patches `project.toml` via `edit_file` if needed. No new MCP
tools. No mid-flow restart.

---

## Section 1: Hardware Detection

New function `detect_hardware_context()` in `src/tools/workflow.rs`.
All probes run in parallel with a 2-second timeout. All failures are graceful —
no probe failure blocks onboarding.

```rust
struct HardwareContext {
    ollama_available: bool,
    ollama_host: String,       // from $OLLAMA_HOST or "http://localhost:11434"
    gpu: Option<GpuInfo>,
    ram_gb: u64,
    cpu_cores: u32,
}

enum GpuInfo {
    Nvidia { name: String, vram_mb: u64 },
    Amd    { name: String, vram_mb: Option<u64> },
}
```

**Probes:**

| Signal | Command / Method |
|---|---|
| Ollama | `GET $OLLAMA_HOST/api/tags`, 2s timeout |
| NVIDIA GPU | `nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits` |
| AMD GPU | `rocm-smi --showproductname` + `rocm-smi --showmeminfo vram` |
| RAM | `/proc/meminfo` (Linux) / `sysctl hw.memsize` (macOS) |
| CPU cores | `std::thread::available_parallelism()` |

---

## Section 2: Model Options & Ranking

From `HardwareContext`, derive exactly 3 ranked `ModelOption`s:

```rust
struct ModelOption {
    id: String,
    label: String,
    dims: u32,
    context_tokens: u32,
    reason: String,
    available: bool,
    recommended: bool,
}
```

**Decision tree:**

```
Ollama available?
├── YES
│   ├── ★ ollama:nomic-embed-text         768d, 2048 tok, 137MB  — "fast, good general baseline"
│   ├──   ollama:bge-m3                  1024d, 8192 tok, 1.2GB  — "best quality, slower indexing"
│   └──   local:JinaEmbeddingsV2BaseCode  768d, 8192 tok, ~300MB — "code-specific, no Ollama needed"
│
└── NO
    ├── ★ local:JinaEmbeddingsV2BaseCode  768d, 8192 tok, ~300MB — "code-specific, beats general models on CodeSearchNet"
    ├──   local:AllMiniLML6V2Q            384d,  256 tok,   22MB — "lightest, good for constrained machines"
    └──   ollama:nomic-embed-text         768d, 2048 tok (unavailable — "run `ollama serve` to enable")
```

The recommended model (`model_options[0].id`) is written to `project.toml`
immediately when the config is created. If the user picks an alternative, the LLM
calls `edit_file` on `.codescout/project.toml` to change the `model = "..."` line.

---

## Section 3: Onboarding Flow Changes

### 3a. Rust — `Onboarding::call()` changes

1. Run `detect_hardware_context()` in parallel with existing language/file detection.
2. Compute `model_options` from `HardwareContext`.
3. When creating `project.toml`, use `model_options[0].id` instead of `Default::default()`
   (which previously hardcoded `ollama:mxbai-embed-large`).
4. Add `hardware` and `model_options` fields to the returned JSON.

```json
{
  "hardware": {
    "ollama_available": true,
    "ollama_host": "http://localhost:11434",
    "gpu": { "vendor": "nvidia", "name": "RTX 3080", "vram_mb": 10240 },
    "ram_gb": 32,
    "cpu_cores": 16
  },
  "model_options": [
    { "id": "ollama:nomic-embed-text", "dims": 768, "context_tokens": 2048,
      "reason": "fast, good general baseline", "available": true, "recommended": true },
    { "id": "ollama:bge-m3", "dims": 1024, "context_tokens": 8192,
      "reason": "best quality, slower indexing (~1.2GB)", "available": true, "recommended": false },
    { "id": "local:JinaEmbeddingsV2BaseCode", "dims": 768, "context_tokens": 8192,
      "reason": "code-specific, no Ollama needed (~300MB download)", "available": true, "recommended": false }
  ]
}
```

### 3b. Onboarding prompt — Phase 0.5 (new section)

Inserted between config creation and the existing Phase 0 (index check).

```
## Phase 0.5: Embedding Model Selection

Present this to the user:

> **Choose an embedding model for semantic search.**
>
> Based on your system ({hardware summary}), here are your options:
>
> 1. ★ `{model_options[0].id}` — {dims}d, {context_tokens}-token context
>    {reason}  ← **Recommended**
> 2.   `{model_options[1].id}` — {dims}d, {context_tokens}-token context
>    {reason}
> 3.   `{model_options[2].id}` — {dims}d, {context_tokens}-token context
>    {reason}  {if !available: "(not currently available — requires Ollama)"}
>
> Press Enter to accept [1], or type 2 or 3 to choose.

Wait for the user's response.

- Confirm [1] / Enter: proceed — config is already written with this model.
- Pick 2 or 3: call `edit_file` on `.codescout/project.toml`,
  change the `model = "..."` line under `[embeddings]` to the chosen model id.
- Custom string: use that directly in the same `edit_file` call.

Then proceed to Phase 0 (index check).
```

### 3c. Session restart suggestion (end of onboarding)

Appended to "After Everything Is Created" in `onboarding_prompt.md`:

```
Finally, inform the user:

> Onboarding is complete. To activate the new project configuration in this
> session, restart Claude Code or run `/mcp` to reconnect the MCP server.
```

---

## Section 4: Code Changes

### Files touched

| File | Change |
|---|---|
| `src/tools/workflow.rs` | Add `HardwareContext`, `GpuInfo`, `ModelOption` structs; `detect_hardware_context()` fn; update `Onboarding::call()` |
| `src/prompts/onboarding_prompt.md` | Insert Phase 0.5; append restart suggestion |
| `src/embed/mod.rs` | Add `"nomic-embed-text"` (2048 tok) and `"bge-m3"` (8192 tok) to `chunk_size_for_model()` |
| `src/config/project.rs` | No change — `default_embed_model()` stays as mxbai (correct fallback for manual config creation outside onboarding) |

### What is NOT changing

- No new MCP tools
- No changes to `create_embedder()`, `local.rs`, or `remote.rs`
- `project.toml` format unchanged
- No mid-flow MCP restart required

### Tests to add

| Test | File | Asserts |
|---|---|---|
| `onboarding_includes_hardware_context` | `workflow.rs` | `hardware.ollama_available`, `model_options` len == 3 |
| `onboarding_recommends_nomic_when_ollama_available` | `workflow.rs` | `model_options[0].id == "ollama:nomic-embed-text"` |
| `onboarding_recommends_jina_when_cpu_only` | `workflow.rs` | `model_options[0].id == "local:JinaEmbeddingsV2BaseCode"` |
| `onboarding_writes_recommended_model_to_config` | `workflow.rs` | `project.toml` contains recommended model id, not mxbai |
| `chunk_size_nomic_and_bge_m3` | `embed/mod.rs` | nomic→2048 tok window, bge-m3→8192 tok window |

---

## Estimated Scope

~150–200 lines of new Rust, ~40 lines of new prompt text, 5 new tests.
No migration needed for existing `project.toml` files — they keep whatever model
they already have; hardware detection only runs on fresh onboarding.
