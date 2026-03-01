# Dashboard

The dashboard is a local web UI that gives you a live view of your project's
health, tool usage, and memories. It runs as a separate process — no MCP server,
no LSP, no tool machinery — just the data already on disk in `.code-explorer/`.

```bash
code-explorer dashboard --project .
# opens http://127.0.0.1:8099
```

## Pages

### Overview

Project health at a glance:

- **Project** — root path, detected languages, entry points
- **Configuration** — active settings from `.code-explorer/project.toml`
- **Semantic Index** — chunk count, last-indexed commit, staleness relative to HEAD
- **Drift** — files with high semantic drift since last index (files where meaning
  changed significantly, not just bytes)
- **Libraries** — registered third-party libraries and their index status

### Tool Stats

Usage telemetry for every tool call the MCP server has handled:

- **Summary** — total calls, error rate, overflow rate for the selected window
- **Calls by Tool** — bar chart ranked by call volume
- **Per-Tool Breakdown** — table with calls, errors, Err%, overflows, Ovf%, p50
  and p99 latency
- **Recent Errors** — last N errors with full input/output, searchable and
  collapsible by duplicate group

The time window selector covers 1h / 24h / 7d / 30d and updates all panels
simultaneously.

### Memories

Read and edit the project's persistent memory store directly in the browser:

- Browse topics in the sidebar
- View raw markdown content
- Create, update, or delete topics without touching the filesystem manually

## Options

| Flag | Default | Description |
|---|---|---|
| `--host` | `127.0.0.1` | Bind address |
| `--port` | `8099` | Port |
| `--no-open` | off | Disable auto-opening the browser |

```bash
code-explorer dashboard --project . --port 9000
```

## Notes

- The dashboard reads `.code-explorer/` directly; the MCP server does not need to
  be running
- Static assets (HTML, CSS, JS) are embedded in the binary — no separate serving
  step
- Theme toggle (light/dark) persists across page loads via `localStorage`
