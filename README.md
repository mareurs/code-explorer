# codescout

Rust MCP server giving LLMs IDE-grade code intelligence — symbol navigation, semantic search, shell integration, persistent memory, and GitHub integration. Built for [Claude Code](https://code.claude.com/).

![Dashboard — Tool Stats page](docs/images/dashboard.png)

*[Dashboard docs](docs/manual/src/concepts/dashboard.md) — tool usage charts, error log, memory browser, drift view. Run with `codescout dashboard --project .`*

**What sets it apart:**

- **Output Buffers** — large command output is stored in a buffer, not dumped into context. The AI queries it with Unix tools (`grep @cmd_id`, `sed @cmd_id`), keeping the context window lean while making exploration visible and reviewable in Claude Code's UI.
- **Shell integration** — `run_command` executes any shell command from the project root with safety guardrails, stderr capture, and full Output Buffer support for large results.
- **Symbol-level editing** — `replace_symbol`, `insert_code`, `rename_symbol` operate on named code symbols via LSP, not fragile line ranges.
- **Progressive disclosure** — every tool defaults to compact output; `detail_level: "full"` + pagination unlocks everything. No accidental context floods.
- **GitHub integration** — `github_issue`, `github_pr`, `github_file`, and `github_repo` give the AI authenticated access to GitHub — read and write issues, review PRs, push files, search code — without leaving the coding session.

## The Problem

LLMs waste most of their context window on code navigation. `grep` returns walls of text. `cat` dumps entire files when you need one function. There's no way to ask "who calls this?" or "what changed here last?" — the tools are blind to code structure.

The result: shallow understanding, hallucinated edits, constant human course-correction.

## The Solution

codescout is an MCP server that gives your AI coding agent the same navigation tools a human developer uses in an IDE — but optimized for token efficiency.

**Four pillars:**

| Pillar | What it does | Tools |
|---|---|---|
| LSP Navigation | Go-to-definition, hover, find references, rename — via real language servers | 9 tools, 9 languages |
| Semantic Search | Find code by concept, not just text match — via embeddings | 2 tools |
| Persistent Memory | Remember project knowledge across sessions | 1 tool |

Plus file operations (6 tools), workflow (2 tools), config & navigation (3 tools), and GitHub integration (5 tools) — **28 tools total**.

**Recent additions:**
- **`goto_definition` + `hover`** — LSP-backed jump-to-definition and type/doc inspection. `goto_definition` auto-discovers and registers library source when the definition lives outside the project root.
- **`project_status`** — unified project health snapshot: config, embedding index stats, usage telemetry (call counts, error rates, p50/p99 latency), and registered libraries — all in one call.
- **Dashboard** — `codescout dashboard --project .` launches a local web UI (default port 8099) with tool usage charts and project health views.
- **Library Search** — navigate third-party dependency source code via LSP-inferred discovery, symbol navigation, and semantic search. Libraries auto-register when `goto_definition` returns paths outside the project root. Index a library with `index_project(scope: "lib:name")`.
- **Incremental Index Rebuilding** — smart change detection for the embedding index. Uses git diff → mtime → SHA-256 fallback chain to skip unchanged files, with staleness warnings when the index falls behind HEAD.
- **Semantic Drift Detection** — detects *how much* code changed in meaning after re-indexing, not just that bytes changed. Query via `project_status`. Opt out with `drift_detection_enabled = false` in `[embeddings]`.

## Superpowers

The structured workflows we use in this project — TDD, systematic debugging, brainstorming before building, parallel agent dispatch, code review — come from **[Superpowers](https://github.com/obra/superpowers)**, a Claude Code plugin by [Jesse Luoto (obra)](https://github.com/obra). His [blog post introducing Superpowers](https://blog.fsck.com/2025/10/09/superpowers/) is what got us thinking seriously about agent-first development workflows. The discipline baked into our day-to-day work here — the TDD gates, the brainstorming-before-building habit, the worktree isolation patterns — all flow from that foundation. If it has helped you, consider [sponsoring Jesse's open-source work](https://github.com/sponsors/obra).

We also want to acknowledge **[Serena](https://github.com/oraios/serena)**, an MCP code intelligence server we ran for a good while before deciding to build our own. Serena proved the concept: an LLM really can navigate a real codebase through LSP and tree-sitter. That proof of concept directly shaped what codescout became — we kept the good ideas, threw out what didn't fit, and wrote the rest in Rust.

For the worktree workflow specifically: the recommended pattern for large plans and parallel work is to manually `git worktree add` + `cd` + launch Claude from inside the worktree — one terminal per branch, fully isolated, no mid-session project-switching. Note: `finishing-a-development-branch` cleanup can fail when Claude's CWD is inside the worktree; use `git worktree prune` from the main repo instead.

→ [Full details: Superpowers Workflow](docs/manual/src/concepts/superpowers.md)

## Routing Plugin

The MCP server gives Claude the tools, but subagents start with a blank slate and fall back to `Read`/`Grep`/`Bash` by default. The companion `code-explorer-routing` plugin closes this: it injects guidance into every agent and subagent via Claude Code hooks, hard-blocks native file-read patterns before they execute, auto-reindexes in the background, and enforces worktree safety.

→ [Full details: Routing Plugin](docs/manual/src/concepts/routing-plugin.md)

## Output Buffers

Large command output and file reads are stored in a buffer and returned as `@id` handles rather than dumped into context. The AI queries them with Unix tools: `run_command("grep FAILED @cmd_a1b2c3")`, `run_command("sed -n '42,80p' @file_abc456")`. Each query appears as a distinct tool call in Claude Code's UI, keeping exploration visible and the context window lean.

→ [Full details: Output Buffers](docs/manual/src/concepts/output-buffers.md)

## Shell Integration

`run_command` executes any shell command from the project root with stderr capture, Output Buffer support, and safety guardrails: dangerous commands require explicit acknowledgement, direct source file access via `cat`/`grep` is blocked and redirected to symbol tools, and `cwd` paths are validated against the project root.

→ [Full details: Shell Integration](docs/manual/src/concepts/shell-integration.md)

## Security & Permissions

Reads are permissive (agents can navigate any codebase, library source, or system headers they need). Writes are hard-sandboxed to the project root — an agent running autonomously cannot touch unrelated projects or system files without explicit opt-in. Shell execution is off by default and requires two-field opt-in. Credential paths (`~/.ssh`, `~/.aws`, etc.) are blocked unconditionally for both reads and writes.

→ [Full details: Security & Permissions](docs/manual/src/concepts/security.md)

## Git Worktree Support

`EnterWorktree` moves the shell into a worktree, but the MCP server's project root doesn't follow — write tools silently target the main repo unless you call `activate_project` first. Three layers handle this: a write guard (hard block), a `worktree_hint` advisory on every write response, and `.worktrees/` exclusions from file navigation.

→ [Full details: Git Worktrees](docs/manual/src/concepts/worktrees.md)

## Platform Support

Tested on **Linux**. macOS and Windows may work but have not been verified. Contributions welcome.

## Tested Setup

The behavior described in this README — Claude consistently reaching for the right tool, structured workflows, cross-session memory — was produced with a specific combination of Claude Code plugins running together. Without the full stack, results will vary.

> **The short version:** codescout gives Claude the *capability*. The surrounding plugins ensure that capability is actually *used*, consistently, across every session and every subagent.

**Our full stack:**

| Component | Plugin ID | Role |
|---|---|---|
| **codescout** | *(this server)* | 28 tools: symbol navigation, semantic search, memory |
| **code-explorer-routing** | `code-explorer-routing@sdd-misc-plugins` | Hooks that enforce tool selection — intercepts `grep`/`cat`/`read` in all sessions and subagents and redirects to codescout equivalents |
| **superpowers** | `superpowers@superpowers-marketplace` | Skills framework: TDD, systematic debugging, brainstorming, parallel agents, code review |
| **episodic-memory** | `episodic-memory@superpowers-marketplace` | Cross-session memory: Claude searches past conversations semantically before starting any task |
| **hookify** | `hookify@claude-plugins-official` | Hook-based behavior enforcement: codify recurring antipatterns into rules that block them automatically |
| **rust-analyzer-lsp** | `rust-analyzer-lsp@claude-plugins-official` | Rust language server: powers `goto_definition`, `hover`, `find_references` for Rust code |

**Gotchas:**

- **Without `code-explorer-routing`**: Claude has the tools but won't always reach for them. Old habits resurface — especially `grep`, `cat`, and `read` in subagents that start with a blank slate. The routing plugin is what makes the behavior consistent.
- **Without `superpowers`**: Claude works but lacks structured workflows. TDD, systematic debugging, and parallel agent dispatch require the skills framework.
- **Without `episodic-memory`**: Knowledge resets each session. The built-in `memory` tool persists notes within a project, but only episodic-memory lets Claude search past *conversations*.
- **Without `rust-analyzer-lsp`** (or a running `rust-analyzer` daemon): LSP tools (`goto_definition`, `hover`, `find_references`, `find_symbol`) degrade to tree-sitter-only mode for Rust — still useful, but no type resolution or cross-file navigation.

## Installation

> **This is a Claude Code tool.** codescout is built for [Claude Code](https://code.claude.com/) and currently requires it as the host agent. Other MCP-capable agents may work but are not tested.

**The easiest way to get started:** clone the repo and let Claude do the installation for you. It has access to the full documentation, your system, and the install scripts — it will handle everything from building the binary to registering the MCP server and installing LSP servers for your languages.

```bash
git clone https://github.com/mareurs/codescout.git
cd codescout
claude
# Then ask: "Help me install and set up codescout"
```

If you prefer to install manually, follow the steps below.

---

codescout has two components that work together:

1. **MCP Server** — provides the 28 tools (symbol navigation, semantic search, memory, etc.)
2. **Routing Plugin** — ensures Claude always uses the right tool, across all sessions and subagents

**Both are recommended.** The MCP server gives Claude the capability; the plugin ensures
that capability is always used correctly. Without the plugin, Claude will occasionally
fall back to `grep`/`cat`/`read` out of habit — especially in subagents that start with
a blank slate.

### Step 1: Install the MCP server

```bash
cargo install codescout
```

Register it globally so it's available in every Claude Code session:

```bash
claude mcp add --global codescout -- codescout start --project .
```

Or per-project (add to your project's `.mcp.json`):

```bash
claude mcp add codescout -- codescout start --project /path/to/your/project
```

### Step 2: Install the routing plugin

```bash
claude /plugin install code-explorer-routing@sdd-misc-plugins
```

Or add to your user settings (`~/.claude/settings.json`) for all sessions:

```json
{
  "enabledPlugins": {
    "code-explorer-routing@sdd-misc-plugins": true
  }
}
```

The plugin is available from the [claude-plugins marketplace](https://github.com/mareurs/claude-plugins).

### Step 3: Verify

```bash
claude mcp list
# Should show: codescout with 28 tools
```

### How They Interact

```
┌─────────────────────────────────────────────────────┐
│                   Claude Code                        │
│                                                      │
│  ┌─────────────────────────────────────────────┐    │
│  │  code-explorer-routing plugin (hooks)        │    │
│  │                                              │    │
│  │  SessionStart  → inject tool selection guide │    │
│  │  SubagentStart → propagate to all subagents  │    │
│  │  PreToolUse    → redirect grep/cat/read to   │    │
│  │                  codescout equivalents        │    │
│  └──────────────────────┬──────────────────────┘    │
│                         │ routes to                   │
│  ┌──────────────────────▼──────────────────────┐    │
│  │  codescout MCP server (28 tools)             │    │
│  │                                              │    │
│  │  LSP · Semantic · Memory · ...               │    │
│  └──────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────┘
```

**Without the plugin:** Claude has access to the tools but may not use them
optimally — it might read whole files instead of using `find_symbol`, or grep
instead of `semantic_search`.

**With the plugin:** Every session and subagent gets automatic guidance on which
tool to use for each situation. The `PreToolUse` hook actively intercepts
suboptimal tool calls and redirects them before they execute.

## Tools (28)

| Category | Count | Highlights |
|---|---|---|
| Symbol Navigation | 9 | `find_symbol`, `list_symbols`, `goto_definition`, `hover`, `find_references`, `replace_symbol`, `remove_symbol`, `insert_code`, `rename_symbol` |
| File Operations | 6 | `read_file`, `list_dir`, `search_pattern`, `create_file`, `find_file`, `edit_file` |
| Semantic Search | 2 | `semantic_search`, `index_project` |
| Memory | 1 | `memory` (read / write / list / delete) |
| Workflow | 2 | `onboarding`, `run_command` |
| Config & Navigation | 3 | `activate_project`, `project_status`, `list_libraries` |
| GitHub | 5 | `github_identity`, `github_issue`, `github_pr`, `github_file`, `github_repo` |

Every tool defaults to compact output (exploring mode) and supports `detail_level: "full"` with pagination for when you need the complete picture.

See the [full tool reference](docs/manual/src/tools/overview.md) for parameters, examples, and usage guidance.

## Supported Languages

| | Languages |
|---|---|
| **Full** (LSP + tree-sitter) | Rust, Python, TypeScript, TSX, Go, Java, Kotlin |
| **LSP only** | JavaScript, JSX, C, C++, C#, Ruby |
| **Detection only** | PHP, Swift, Scala, Elixir, Haskell, Lua, Bash, Markdown |

Install LSP servers with the bundled script:

```bash
./scripts/install-lsp.sh --check          # see what's installed / missing
./scripts/install-lsp.sh --all            # install everything
./scripts/install-lsp.sh rust python go   # install specific languages
```

See [Language Support](docs/manual/src/language-support.md) for manual install commands and known quirks.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for how to get started. PRs from Claude Code are welcome!

## License

[MIT](LICENSE)
