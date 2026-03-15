# Experimental Features

> ⚠ **These features are on the `experiments` branch and may change or be removed without
> notice. They may not be present in your installed release.**

Features listed here are working but not yet merged to `master`. To try them:

```bash
git clone https://github.com/mareurs/codescout
cd codescout
git checkout experiments
cargo build --release
```

Then register the locally built binary in your MCP config instead of the installed `codescout`.

## Features in development

- [LSP Idle TTL Eviction](lsp-idle-ttl.md) — automatically shut down idle LSP servers to
  reclaim memory, with per-language configurable timeouts
- [Multi-Project Workspace Support](multi-project-workspace.md) — register and navigate
  multiple related projects from a single codescout instance
