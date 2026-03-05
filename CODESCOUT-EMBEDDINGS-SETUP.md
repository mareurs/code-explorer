# Code Explorer — Embedding Setup on WSL2 Behind Zscaler

This document captures the full journey of getting semantic search working in
`codescout` on a WSL2 machine behind a Zscaler corporate proxy. It is
written as a troubleshooting narrative so that future developers can skip the
trial-and-error and go straight to the working solution.

---

## Environment

| Layer | Detail |
|---|---|
| OS | Ubuntu on WSL2 (Windows host) |
| Network proxy | Zscaler (SSL-inspecting corporate proxy) |
| Tool | `codescout` MCP server (built from source) |
| Target | `local:AllMiniLML6V2Q` — CPU-only, INT8-quantized, ~22 MB |

---

## Problem 1 — Binary built without local-embed feature

### Symptom

```
The local embedding model isn't available in this environment. The
codescout server was built without the local-embed feature, so
semantic search isn't supported here.
```

### Cause

`codescout` has an optional Cargo feature `local-embed` that pulls in
`fastembed-rs` and the ONNX Runtime. The default build omits it.

### Fix

```bash
cd /home/vradu/work/tools/codescout
git pull
cargo install --path . --features local-embed
```

Restart Claude Code after rebuilding.

---

## Problem 2 — Model file blocked by Zscaler (XetHub CDN)

### Symptom

```
Failed to retrieve model_optimized.onnx
```

### Cause

HuggingFace stores large files (ONNX models, etc.) on its XetHub CDN
(`cas-bridge.xethub.hf.co`). Every download of a model file redirects there:

```
huggingface.co → cas-bridge.xethub.hf.co → gateway.zscalertwo.net (BLOCKED)
```

Zscaler returns an HTML block page instead of the binary. This affects all
HTTP clients inside WSL2: `curl`, `wget`, Rust `reqwest`, and the `hf_hub`
Rust crate.

Small files (`tokenizer.json`, `config.json`, etc.) are served directly from
`huggingface.co` without a redirect and download fine.

### What does NOT work

- `curl -L` / `wget` — same redirect chain, same block
- HuggingFace mirrors (e.g. `hf-mirror.com`) — same XetHub redirect
- LFS batch API — also returns an XetHub URL
- Setting `HF_ENDPOINT` — only changes the metadata host, not the LFS CDN

### Fix — manually build the hf_hub cache

`fastembed-rs` uses `hf_hub` to cache models with this directory layout:

```
.fastembed_cache/
  models--<Org>--<repo>/
    refs/
      main                          ← text file containing the commit hash
    blobs/
      <etag>                        ← actual file content (named by server etag)
    snapshots/
      <commit_hash>/
        tokenizer.json              ← symlink → ../../blobs/<etag>
        config.json                 ← symlink → ../../blobs/<etag>
        onnx/
          model_quantized.onnx     ← symlink → ../../../blobs/<etag>
```

The `etag` value comes from the HTTP `x-linked-etag` response header:

```bash
curl -sI "https://huggingface.co/<org>/<repo>/resolve/main/<file>" \
  | grep -i "^x-linked-etag:"
```

**Important:** Use `grep -i "^x-linked-etag:"` (anchored). The
`access-control-expose-headers` line also contains the string `x-linked-etag`
as a field name — using an unanchored grep silently captures the wrong line
and creates broken symlinks.

Small files (tokenizer, config, vocab) can be downloaded inside WSL2 with
`curl`. The large ONNX file must be downloaded from the **Windows browser**
(which is outside the Zscaler WSL2 policy) and then copied into WSL2.

---

## Problem 3 — BGESmallENV15Q is GPU-only (ORT incompatibility)

### Symptom

```
Non-zero status code returned while running SkipLayerNormalization node.
Name:'SkipLayerNorm_AddBias_0'
Missing Input: encoder.layer.0.attention.output.LayerNorm.weight
```

### Cause

`codescout` statically links ORT 1.20.0 via the `ort-download-binaries`
Cargo feature. The Qdrant model `Qdrant/bge-small-en-v1.5-onnx-Q` was
exported with:

```json
{ "optimize_for_gpu": true, "fp16": true }
```

(Readable from `ort_config.json` in the same HuggingFace repo.)

This means:

- Weights are stored in **float16** — not INT8
- The graph uses **GPU-specific fused operators** (`SkipLayerNormalization`
  with FP16 inputs) that have no CPU kernel in ORT 1.20
- The "Q" suffix in fastembed's docs is misleading — this model is
  **GPU-optimized FP16**, not a CPU-friendly INT8 quantized model

The model will fail on any CPU-only machine regardless of ORT version. The
only fix is to use a different model.

### How to check a model before downloading

```bash
curl -sL "https://huggingface.co/<org>/<repo>/resolve/main/ort_config.json" \
  | python3 -m json.tool | grep -E "optimize_for_gpu|fp16"
```

A CPU-safe model should show `false` for both. A model safe for CPU also uses
standard ONNX INT8 quantization ops (`QuantizeLinear` / `DequantizeLinear`),
not graph-fused operators.

---

## Working Solution — AllMiniLML6V2Q (Xenova format)

| Property | Value |
|---|---|
| fastembed model name | `local:AllMiniLML6V2Q` |
| HuggingFace repo | `Xenova/all-MiniLM-L6-v2` |
| Model file | `onnx/model_quantized.onnx` |
| Download size | ~22 MB |
| Quantization | Standard INT8 (QuantizeLinear/DequantizeLinear) |
| ORT compatibility | Any ORT version |
| Dimensions | 384 |

### Step-by-step setup

**1. Rebuild codescout with local-embed** (if not already done)

```bash
cd /home/vradu/work/tools/codescout
cargo install --path . --features local-embed
```

**2. Update `.codescout/project.toml`**

```toml
[embeddings]
model = "local:AllMiniLML6V2Q"
```

**3. Get commit hash and etags from HuggingFace**

```bash
BASE="https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main"

# Commit hash
COMMIT=$(curl -sI "$BASE/tokenizer.json" | grep -i "^x-repo-commit:" | awk '{print $2}' | tr -d '\r')
echo "Commit: $COMMIT"

# Etag for the ONNX file
ONNX_ETAG=$(curl -sI "$BASE/onnx/model_quantized.onnx" | grep -i "^x-linked-etag:" | awk '{print $2}' | tr -d '"\r')
echo "ONNX etag: $ONNX_ETAG"
```

**4. Download the ONNX file from Windows browser**

Open a browser **on the Windows host** (not inside WSL2) and download:

```
https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/onnx/model_quantized.onnx
```

Verify the SHA256 matches the etag:

```bash
sha256sum /mnt/c/Users/<username>/Downloads/model_quantized.onnx
# Should match: afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1
```

**5. Build the cache structure**

```bash
PROJECT="/home/vradu/work/WorkspaceExtenda/hiiretail-payment-client-adyen"
COMMIT="751bff37182d3f1213fa05d7196b954e230abad9"
ONNX_ETAG="afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1"
CACHE="$PROJECT/.fastembed_cache/models--Xenova--all-MiniLM-L6-v2"

mkdir -p "$CACHE/blobs" "$CACHE/refs" \
         "$CACHE/snapshots/$COMMIT/onnx"

# Write ref
echo -n "$COMMIT" > "$CACHE/refs/main"

# Copy ONNX blob and create symlink
cp /mnt/c/Users/vradu/Downloads/model_quantized.onnx "$CACHE/blobs/$ONNX_ETAG"
ln -sf "../../../blobs/$ONNX_ETAG" "$CACHE/snapshots/$COMMIT/onnx/model_quantized.onnx"

# Download small files (these are not behind XetHub)
BASE="https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main"
for fname in tokenizer.json config.json special_tokens_map.json tokenizer_config.json vocab.txt; do
  ETAG=$(curl -sI "$BASE/$fname" | grep -i "^x-linked-etag:" | awk '{print $2}' | tr -d '"\r')
  curl -sL -o "$CACHE/blobs/$ETAG" "$BASE/$fname"
  ln -sf "../../blobs/$ETAG" "$CACHE/snapshots/$COMMIT/$fname"
  echo "$fname ✓"
done
```

**6. Trigger reindex in Claude Code**

```
reindex
```

Or use the MCP tool `index_project(force=true)`.

Expected result: ~5,600 chunks indexed across ~187 files.

---

## Key Lessons

### 1 — hf_hub etag extraction is tricky

The `access-control-expose-headers` response header lists `X-Linked-ETag` as
one of many field names. Grepping for `x-linked-etag` without anchoring the
pattern returns this line instead of the actual etag value, producing broken
symlinks that look correct but resolve to nonexistent paths.

Always use:
```bash
grep -i "^x-linked-etag:"
```

### 2 — Model "quantized" ≠ CPU-safe

The `-Q` suffix in fastembed model names and the word "quantized" in docs do
not guarantee CPU compatibility. Always inspect `ort_config.json` for
`optimize_for_gpu` and `fp16` flags before attempting CPU inference.

### 3 — Zscaler blocks XetHub, not HuggingFace

Small files on HuggingFace (< a few MB, not in Git LFS) serve directly from
`huggingface.co` and download fine through Zscaler. Large binary files (ONNX
models, weights) are stored in XetHub LFS and always redirect to
`cas-bridge.xethub.hf.co`, which Zscaler blocks. The workaround is to
download from the Windows host browser and copy into WSL2.

### 4 — ORT is statically compiled into codescout

The `ort-download-binaries` Cargo feature downloads the ORT binary at build
time and links it statically. You cannot swap the ORT runtime at runtime via
`LD_PRELOAD` or environment variables. If a model requires a different ORT
version, you must rebuild codescout targeting that version.
