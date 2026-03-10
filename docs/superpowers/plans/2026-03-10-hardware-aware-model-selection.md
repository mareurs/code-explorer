# Hardware-Aware Embedding Model Selection Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `onboarding()` detect hardware (Ollama, GPU, RAM) and propose a ranked model menu so users get the right embedding model from the start instead of a hardcoded `mxbai-embed-large`.

**Architecture:** Hardware detection runs in Rust at `onboarding()` call time via `detect_hardware_context()` (TCP probe + shell commands, all parallel, 2s timeout each). A pure `model_options_for_hardware()` function derives a ranked 3-option `Vec<ModelOption>` from the hardware facts. The recommended model is written to `project.toml`; the onboarding prompt's new Phase 0.5 presents the menu and instructs the LLM to `edit_file` if the user picks an alternative.

**Tech Stack:** Rust, tokio (process + net + time — already in scope), serde_json (already in scope). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-03-10-hardware-aware-model-selection-design.md`

---

## Chunk 1: Fix bge-m3 context window in `chunk_size_for_model`

**Files:**
- Modify: `src/embed/mod.rs` (L67–91 `tokens_for_bare`, L228–277 tests module)

### Task 1: Add bge-m3 test (TDD)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/embed/mod.rs`, after `chunk_size_nomic_embed_text` (L239):

```rust
#[test]
fn chunk_size_bge_m3() {
    // bge-m3 has 8192-token context. Formula: 8192 × 0.85 × 3 = 20889.
    let sz = super::chunk_size_for_model("ollama:bge-m3");
    assert_eq!(sz, 20889);
}
```

- [ ] **Step 2: Run to confirm it fails**

```bash
cargo test chunk_size_bge_m3
```

Expected: FAIL — `left: 1305, right: 20889` (bge-m3 currently hits the 512-token fallback).

- [ ] **Step 3: Fix `tokens_for_bare` in `src/embed/mod.rs`**

In `tokens_for_bare` (L68), extend the first `if` to include `bge-m3`:

```rust
// 8 192-token models
if l.contains("nomic-embed") || l.contains("jina") || l.contains("bge-m3") {
    return 8192;
}
```

- [ ] **Step 4: Run to confirm it passes**

```bash
cargo test chunk_size
```

Expected: all `chunk_size_*` tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/embed/mod.rs
git commit -m "fix: bge-m3 context window in chunk_size_for_model (8192 tokens, not 512)"
```

---

## Chunk 2: Hardware structs + pure model options logic

**Files:**
- Modify: `src/tools/workflow.rs` — add structs and `model_options_for_hardware()` near top of file (before `Onboarding` struct)

### Task 2: Add types + pure logic with tests

- [ ] **Step 1: Write failing tests for model options logic**

Add to the `tests` module at the bottom of `src/tools/workflow.rs`:

```rust
// ---------- hardware detection ----------

#[test]
fn model_options_ollama_available_recommends_nomic() {
    let ctx = super::HardwareContext {
        ollama_available: true,
        ollama_host: "http://localhost:11434".into(),
        gpu: None,
        ram_gb: 16,
        cpu_cores: 8,
    };
    let opts = super::model_options_for_hardware(&ctx);
    assert_eq!(opts.len(), 3);
    assert_eq!(opts[0].id, "ollama:nomic-embed-text");
    assert!(opts[0].recommended);
    assert!(!opts[1].recommended);
    assert!(!opts[2].recommended);
}

#[test]
fn model_options_cpu_only_recommends_jina() {
    let ctx = super::HardwareContext {
        ollama_available: false,
        ollama_host: "http://localhost:11434".into(),
        gpu: None,
        ram_gb: 8,
        cpu_cores: 4,
    };
    let opts = super::model_options_for_hardware(&ctx);
    assert_eq!(opts[0].id, "local:JinaEmbeddingsV2BaseCode");
    assert!(opts[0].recommended);
    // Third option is Ollama but marked unavailable
    assert_eq!(opts[2].id, "ollama:nomic-embed-text");
    assert!(!opts[2].available);
}

#[test]
fn model_options_exactly_one_recommended() {
    let ctx_with_ollama = super::HardwareContext {
        ollama_available: true,
        ollama_host: "http://localhost:11434".into(),
        gpu: Some(super::GpuInfo::Nvidia { name: "RTX 3080".into(), vram_mb: 10240 }),
        ram_gb: 32,
        cpu_cores: 16,
    };
    let opts = super::model_options_for_hardware(&ctx_with_ollama);
    let recommended_count = opts.iter().filter(|o| o.recommended).count();
    assert_eq!(recommended_count, 1);
}
```

- [ ] **Step 2: Run to confirm tests fail (types don't exist yet)**

```bash
cargo test model_options
```

Expected: FAIL — compile error, `HardwareContext` not found.

- [ ] **Step 3: Add structs and `model_options_for_hardware` to `src/tools/workflow.rs`**

Find the `use` imports section at the top of `src/tools/workflow.rs`. After the last `use` statement (before the first `struct` or `fn`), insert:

> **Note on `context_tokens: 8192` for nomic-embed-text:** The spec's decision tree labels nomic as "2048 tok" but the existing `chunk_size_for_model` function already groups `nomic-embed` under the 8192-token branch (matching Nomic's published model spec). The plan uses `context_tokens: 8192` to stay consistent with the live codebase. The spec table entry is a documentation error.

```rust
// ── Hardware detection ────────────────────────────────────────────────────────

/// System facts gathered at onboarding time for model selection.
#[derive(Debug, serde::Serialize)]
pub struct HardwareContext {
    pub ollama_available: bool,
    pub ollama_host: String,
    pub gpu: Option<GpuInfo>,
    pub ram_gb: u64,
    pub cpu_cores: u32,
}

/// GPU vendor and VRAM info (best-effort; None means no GPU detected).
#[derive(Debug, serde::Serialize)]
#[serde(tag = "vendor", rename_all = "lowercase")]
pub enum GpuInfo {
    Nvidia { name: String, vram_mb: u64 },
    Amd    { name: String, vram_mb: Option<u64> },
}

/// One entry in the ranked model recommendation list.
#[derive(Debug, serde::Serialize)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
    pub dims: u32,
    pub context_tokens: u32,
    pub reason: String,
    pub available: bool,
    pub recommended: bool,
}

/// Pure function: derive a ranked 3-option model list from hardware facts.
/// Always returns exactly 3 entries; the first is the recommended default.
pub fn model_options_for_hardware(ctx: &HardwareContext) -> Vec<ModelOption> {
    if ctx.ollama_available {
        vec![
            ModelOption {
                id: "ollama:nomic-embed-text".into(),
                label: "nomic-embed-text".into(),
                dims: 768,
                context_tokens: 8192,
                reason: "fast, good general baseline via Ollama (137MB)".into(),
                available: true,
                recommended: true,
            },
            ModelOption {
                id: "ollama:bge-m3".into(),
                label: "bge-m3".into(),
                dims: 1024,
                context_tokens: 8192,
                reason: "best quality, slower indexing (~1.2GB pull)".into(),
                available: true,
                recommended: false,
            },
            ModelOption {
                id: "local:JinaEmbeddingsV2BaseCode".into(),
                label: "JinaEmbeddingsV2BaseCode".into(),
                dims: 768,
                context_tokens: 8192,
                reason: "code-specific, CPU-only, no Ollama needed (~300MB download)".into(),
                available: true,
                recommended: false,
            },
        ]
    } else {
        vec![
            ModelOption {
                id: "local:JinaEmbeddingsV2BaseCode".into(),
                label: "JinaEmbeddingsV2BaseCode".into(),
                dims: 768,
                context_tokens: 8192,
                reason: "code-specific, beats general models on CodeSearchNet (~300MB download)".into(),
                available: true,
                recommended: true,
            },
            ModelOption {
                id: "local:AllMiniLML6V2Q".into(),
                label: "AllMiniLML6V2Q".into(),
                dims: 384,
                context_tokens: 256,
                reason: "lightest option, good for constrained machines (~22MB)".into(),
                available: true,
                recommended: false,
            },
            ModelOption {
                id: "ollama:nomic-embed-text".into(),
                label: "nomic-embed-text".into(),
                dims: 768,
                context_tokens: 8192,
                reason: "not available — run `ollama serve` to enable".into(),
                available: false,
                recommended: false,
            },
        ]
    }
}
```

- [ ] **Step 4: Run tests to confirm they pass**

```bash
cargo test model_options
```

Expected: all 3 tests PASS.

- [ ] **Step 5: Clippy + fmt**

```bash
cargo fmt && cargo clippy -- -D warnings
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: add HardwareContext + model_options_for_hardware (pure logic)"
```

---

## Chunk 3: I/O hardware detection probes

**Files:**
- Modify: `src/tools/workflow.rs` — add `detect_hardware_context()` and helpers after the structs from Chunk 2

### Task 3: Implement `detect_hardware_context`

> Note: No unit tests for the I/O probes — they shell out to system tools that may or may not be installed. Covered by integration via the onboarding tests in Chunk 4.

- [ ] **Step 1: Add helper `ollama_tcp_addr`**

After `model_options_for_hardware` (end of Chunk 2 code), add:

```rust
/// Extract a `host:port` string suitable for `TcpStream::connect` from an
/// Ollama host URL like `http://localhost:11434`.
fn ollama_tcp_addr(host: &str) -> String {
    let stripped = host
        .strip_prefix("https://")
        .or_else(|| host.strip_prefix("http://"))
        .unwrap_or(host);
    if stripped.contains(':') {
        stripped.to_string()
    } else {
        format!("{stripped}:11434")
    }
}
```

- [ ] **Step 2: Add individual probe helpers**

```rust
/// Returns true if a TCP connection to Ollama's port succeeds within 2s.
async fn probe_ollama(tcp_addr: &str) -> bool {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(tcp_addr),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

/// Probe NVIDIA GPU via nvidia-smi. Returns None if not available.
async fn probe_nvidia() -> Option<GpuInfo> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::process::Command::new("nvidia-smi")
            .args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"])
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next()?;
    let mut parts = line.splitn(2, ',');
    let name = parts.next()?.trim().to_string();
    let vram_mb: u64 = parts.next()?.trim().parse().ok()?;
    Some(GpuInfo::Nvidia { name, vram_mb })
}

/// Probe AMD GPU via rocm-smi. Returns None if not available.
async fn probe_amd() -> Option<GpuInfo> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::process::Command::new("rocm-smi")
            .arg("--showproductname")
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // rocm-smi output contains lines like "Card series:  AMD Radeon RX 7900 XTX"
    let name = stdout
        .lines()
        .find(|l| {
            let l = l.to_lowercase();
            l.contains("card series") || l.contains("card model") || l.contains("radeon")
        })
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_else(|| "AMD GPU".into());
    Some(GpuInfo::Amd { name, vram_mb: None })
}

/// Read total system RAM in GiB. Returns 0 on failure (non-fatal).
async fn probe_ram() -> u64 {
    // Linux: /proc/meminfo
    if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
        for line in content.lines() {
            if line.starts_with("MemTotal:") {
                let kb: u64 = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                return kb / 1024 / 1024;
            }
        }
    }
    // macOS
    if let Ok(output) = tokio::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .await
    {
        if let Ok(s) = String::from_utf8(output.stdout) {
            if let Ok(bytes) = s.trim().parse::<u64>() {
                return bytes / 1024 / 1024 / 1024;
            }
        }
    }
    0
}
```

- [ ] **Step 3: Add the main `detect_hardware_context` function**

```rust
/// Probe the local system for hardware capabilities relevant to embedding
/// model selection. All probes run in parallel with a 2-second timeout;
/// any failure produces a safe zero/None default — never panics.
pub async fn detect_hardware_context() -> HardwareContext {
    let ollama_host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
    let tcp_addr = ollama_tcp_addr(&ollama_host);

    let cpu_cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);

    let (ollama_available, nvidia, amd, ram_gb) = tokio::join!(
        probe_ollama(&tcp_addr),
        probe_nvidia(),
        probe_amd(),
        probe_ram(),
    );

    // NVIDIA wins if both somehow respond (shouldn't happen, but be defensive)
    let gpu = nvidia.or(amd);

    HardwareContext {
        ollama_available,
        ollama_host,
        gpu,
        ram_gb,
        cpu_cores,
    }
}
```

- [ ] **Step 4: Confirm `ollama_tcp_addr` parses correctly — add a unit test**

In the `tests` module:

```rust
#[test]
fn ollama_tcp_addr_strips_http_prefix() {
    assert_eq!(super::ollama_tcp_addr("http://localhost:11434"), "localhost:11434");
    assert_eq!(super::ollama_tcp_addr("https://remote:11434"), "remote:11434");
    assert_eq!(super::ollama_tcp_addr("localhost:11434"), "localhost:11434");
    assert_eq!(super::ollama_tcp_addr("myhost"), "myhost:11434");
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test ollama_tcp_addr
```

Expected: PASS.

- [ ] **Step 6: Clippy + fmt**

```bash
cargo fmt && cargo clippy -- -D warnings
```

Expected: clean. Fix any `unused import` or `dead_code` warnings — the probe helpers are only called from `detect_hardware_context`, which is `pub`, so clippy should be satisfied.

> **Note on `probe_ram`:** The `std::fs::read_to_string("/proc/meminfo")` call is blocking I/O inside an async fn. `/proc/meminfo` is a kernel virtual file that completes in microseconds, so it won't cause visible stalls, but to comply with the codebase rule ("Never block inside async functions"), wrap it in `spawn_blocking`:
>
> ```rust
> let content = tokio::task::spawn_blocking(|| std::fs::read_to_string("/proc/meminfo"))
>     .await
>     .ok()
>     .and_then(|r| r.ok());
> if let Some(content) = content {
>     for line in content.lines() { ... }
> }
> ```
>
> The `sysctl` call on macOS already uses `tokio::process::Command` (non-blocking), so no change needed there.

- [ ] **Step 7: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: detect_hardware_context — parallel Ollama/GPU/RAM probes"
```

---

## Chunk 4: Wire detection into `Onboarding::call()` + tests

**Files:**
- Modify: `src/tools/workflow.rs` — update `Onboarding::call()` (L463–670) and add tests

### Task 4: Integration — call detect, inject model, return fields

- [ ] **Step 1: Write failing integration tests**

Add to `tests` module in `src/tools/workflow.rs`:

```rust
#[tokio::test]
async fn onboarding_includes_hardware_and_model_options() {
    let (_dir, ctx) = project_ctx().await;
    let result = Onboarding.call(json!({}), &ctx).await.unwrap();

    // hardware field is present and has a positive cpu_cores count
    let cores = result["hardware"]["cpu_cores"].as_u64().unwrap();
    assert!(cores > 0, "expected cpu_cores > 0, got {cores}");

    // model_options is a 3-element array with exactly one recommended entry
    let opts = result["model_options"].as_array().unwrap();
    assert_eq!(opts.len(), 3);
    let recommended = opts.iter().filter(|o| o["recommended"].as_bool().unwrap_or(false)).count();
    assert_eq!(recommended, 1);
}

#[tokio::test]
async fn onboarding_writes_recommended_model_to_config() {
    let (dir, ctx) = project_ctx().await;
    // Remove any pre-existing config so onboarding creates a fresh one
    let _ = std::fs::remove_file(dir.path().join(".codescout/project.toml"));

    let result = Onboarding.call(json!({}), &ctx).await.unwrap();

    let toml = std::fs::read_to_string(dir.path().join(".codescout/project.toml")).unwrap();
    // In test env, Ollama is not running, so recommended is Jina
    let recommended_id = result["model_options"][0]["id"].as_str().unwrap();
    assert!(
        toml.contains(recommended_id),
        "project.toml should contain the recommended model id '{recommended_id}'\ntoml:\n{toml}"
    );
    // Specifically: should NOT be the old hardcoded default
    assert!(
        !toml.contains("mxbai-embed-large"),
        "project.toml should not contain mxbai-embed-large when a better model is recommended"
    );
}
```

- [ ] **Step 2: Run to confirm they fail**

```bash
cargo test onboarding_includes_hardware onboarding_writes_recommended
```

Expected: FAIL — `hardware` field missing from result.

- [ ] **Step 3: Update `Onboarding::call()` to run hardware detection**

In `Onboarding::call()` (around L515 where language detection and file walking begin), add `detect_hardware_context()` as a concurrent task alongside the existing work.

Find the section:
```rust
// Detect languages by walking files
let mut languages = std::collections::BTreeSet::new();
let walker = ignore::WalkBuilder::new(&root)
```

Replace it with:

```rust
// Run hardware detection concurrently with language detection
let hw_future = detect_hardware_context();

// Detect languages by walking files
let mut languages = std::collections::BTreeSet::new();
let walker = ignore::WalkBuilder::new(&root)
```

Then, before the `// Create .codescout/project.toml if it doesn't exist` block, resolve the future and derive model options:

```rust
let hw = hw_future.await;
let model_options = model_options_for_hardware(&hw);
let recommended_model = model_options[0].id.clone();
```

- [ ] **Step 4: Inject `recommended_model` into config creation**

Find the config construction block (around L575):
```rust
let config = crate::config::project::ProjectConfig {
    project: crate::config::project::ProjectSection { ... },
    embeddings: Default::default(),
    ...
};
```

Change `embeddings: Default::default()` to:
```rust
embeddings: crate::config::project::EmbeddingsSection {
    model: recommended_model,
    ..Default::default()
},
```

- [ ] **Step 5: Add `hardware` and `model_options` to the returned JSON**

Find the final `Ok(json!({ ... }))` at the end of `call()` (around L650). Add two new fields:

```rust
"hardware": serde_json::to_value(&hw).unwrap_or(serde_json::Value::Null),
"model_options": serde_json::to_value(&model_options).unwrap_or(serde_json::Value::Null),
```

- [ ] **Step 6: Run the new tests**

```bash
cargo test onboarding_includes_hardware onboarding_writes_recommended
```

Expected: both PASS.

- [ ] **Step 7: Run the full test suite**

```bash
cargo test
```

Expected: all tests PASS. Pay attention to `onboarding_creates_config` — it should still pass since the config is still created; the only change is which model is written to it.

- [ ] **Step 8: Clippy + fmt**

```bash
cargo fmt && cargo clippy -- -D warnings
```

Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add src/tools/workflow.rs
git commit -m "feat: onboarding detects hardware and writes recommended embedding model to config"
```

---

## Chunk 5: Onboarding prompt — Phase 0.5 + restart suggestion

**Files:**
- Modify: `src/prompts/onboarding_prompt.md`

### Task 5: Insert Phase 0.5 and restart note

- [ ] **Step 1: Insert Phase 0.5 before Phase 0**

In `src/prompts/onboarding_prompt.md`, find the `## Phase 0: Semantic Index Check` heading (L31). Insert the following block **immediately before** it (i.e., between the `## THE IRON LAW` section end and `## Phase 0`):

```markdown
## Phase 0.5: Embedding Model Selection

The `onboarding` tool has already written a recommended model to `.codescout/project.toml`
based on your system hardware. Present the options to the user now, before indexing starts.

Use the `model_options` array from the Gathered Project Data below to build the menu.
Use the `hardware` field for the one-line system summary.

Present this to the user:

> **Choose an embedding model for semantic search.**
>
> Based on your system ({hardware.cpu_cores} CPU cores
> {if hardware.gpu: ", {hardware.gpu.name}"}
> {if hardware.ollama_available: ", Ollama running" else: ", no Ollama detected"}):
>
> 1. {if model_options[0].recommended: "★ "}`{model_options[0].id}` — {model_options[0].dims}d, {model_options[0].context_tokens}-token context
>    {model_options[0].reason} ← **Recommended**
> 2. `{model_options[1].id}` — {model_options[1].dims}d, {model_options[1].context_tokens}-token context
>    {model_options[1].reason}
> 3. `{model_options[2].id}` — {model_options[2].dims}d, {model_options[2].context_tokens}-token context
>    {model_options[2].reason}{if not model_options[2].available: " *(not currently available)*"}
>
> Press Enter to accept [1], or type 2 or 3 to choose a different model.

Wait for the user's response, then:

- **User presses Enter or types 1:** The config is already correct — proceed to Phase 0.
- **User types 2:** Call `edit_file` on `.codescout/project.toml`.
  Change the line `model = "{model_options[0].id}"` to `model = "{model_options[1].id}"`.
  Confirm the edit, then proceed to Phase 0.
- **User types 3:** Same as above but use `model_options[2].id`.
  If `model_options[2].available` is false, remind the user how to enable it
  (e.g., "install Ollama and run `ollama serve`") before making the edit.
- **User types a custom model string:** Use that string directly in the `edit_file` call.

Then proceed to Phase 0 (Semantic Index Check).

---
```

- [ ] **Step 2: Append restart suggestion to "After Everything Is Created"**

Find the `## After Everything Is Created` heading (L466). At the very end of that section (before `## Gathered Project Data`), append:

```markdown
Finally, inform the user:

> **Onboarding complete.** To activate the new project configuration in this session,
> restart Claude Code or run `/mcp` to reconnect the MCP server.
```

- [ ] **Step 3: Verify the prompt renders correctly**

Read the updated file and confirm:
- Phase 0.5 appears before Phase 0
- Restart note appears at the end of "After Everything Is Created"
- No duplicate headings or broken markdown

```bash
grep -n "^## " src/prompts/onboarding_prompt.md
```

Expected output (order):
```
L4:## THE IRON LAW
L31:## Phase 0.5: Embedding Model Selection   ← new
L??:## Phase 0: Semantic Index Check
L??:## Phase 1: Explore the Code
...
L??:## After Everything Is Created
L??:## Gathered Project Data
L??:## Optional: Private Memories
L??:## Optional: Semantic Memories
```

- [ ] **Step 4: Run full test suite one final time**

```bash
cargo fmt && cargo clippy -- -D warnings && cargo test
```

Expected: all tests PASS, clippy clean.

- [ ] **Step 5: Build release binary**

```bash
cargo build --release
```

Expected: builds successfully.

- [ ] **Step 6: Final commit**

```bash
git add src/prompts/onboarding_prompt.md
git commit -m "feat: onboarding Phase 0.5 — model selection menu + session restart suggestion"
```

---

## Summary of all changed files

| File | Nature of change |
|---|---|
| `src/embed/mod.rs` | Add `bge-m3` to 8192-token group in `tokens_for_bare`; add `chunk_size_bge_m3` test |
| `src/tools/workflow.rs` | Add `HardwareContext`, `GpuInfo`, `ModelOption` structs; add `model_options_for_hardware()`, `detect_hardware_context()`, probe helpers, `ollama_tcp_addr()`; update `Onboarding::call()` to detect hardware, inject recommended model, return `hardware`+`model_options` fields; add 4 new tests |
| `src/prompts/onboarding_prompt.md` | Insert Phase 0.5 (model selection menu); append restart suggestion to "After Everything Is Created" |

**Total new tests:** 7 — `chunk_size_bge_m3`, `model_options_ollama_available_recommends_nomic`, `model_options_cpu_only_recommends_jina`, `model_options_exactly_one_recommended`, `ollama_tcp_addr_strips_http_prefix`, `onboarding_includes_hardware_and_model_options`, `onboarding_writes_recommended_model_to_config`.

**No new dependencies. No new MCP tools. No schema changes.**
