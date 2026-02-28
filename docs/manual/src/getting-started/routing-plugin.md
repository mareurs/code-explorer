# Routing Plugin

## Why the Plugin Exists

Claude Code has access to code-explorer's 31 tools, but it also has built-in tools like `grep`,
`cat`, and `Read`. Without guidance, Claude tends to reach for the familiar built-ins — especially
in subagents, which start each task with a blank slate and have no memory of earlier instructions.

The `code-explorer-routing` plugin solves this with three hooks that run automatically:

- **SessionStart** — injects a tool selection guide into every new session, explaining when to
  prefer code-explorer tools over built-ins.
- **SubagentStart** — propagates the same guide to every subagent that Claude Code spawns, so
  subagents also know to use code-explorer from the start.
- **PreToolUse** — actively intercepts calls to `grep`, `cat`, `Read`, `find`, and `ls` and
  redirects them to the appropriate code-explorer equivalents before they execute.

The difference in practice:

- Without the plugin: Claude has access to code-explorer but may use `grep` for pattern search and
  `cat` for reading files out of habit, missing LSP-backed navigation, token-efficient output, and
  progressive disclosure.
- With the plugin: every session and subagent starts with a clear preference order, and old habits
  are caught and redirected automatically.

## Architecture

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
│  │                  code-explorer equivalents    │    │
│  └──────────────────────┬──────────────────────┘    │
│                         │ routes to                   │
│  ┌──────────────────────▼──────────────────────┐    │
│  │  code-explorer MCP server (31 tools)         │    │
│  │                                              │    │
│  │  LSP · Semantic · Git · AST · Memory · ...   │    │
│  └──────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────┘
```

The plugin is a lightweight shim — it holds no state and adds no latency to tool calls. Its only
job is to steer Claude toward the right tools at the right moment.

## Installation

### Option 1: Claude Code Plugin Command

```bash
claude /plugin install code-explorer-routing@sdd-misc-plugins
```

This downloads and enables the plugin immediately. It persists across sessions.

### Option 2: User Settings

Add the plugin to your Claude Code user settings at `~/.claude/settings.json`:

```json
{
  "enabledPlugins": {
    "code-explorer-routing@sdd-misc-plugins": true
  }
}
```

If the file does not exist yet, create it with that content. Settings take effect the next time
you start a Claude Code session.

### Verification

After installation, start a new Claude Code session and ask Claude which tools it will use for
code search. You should see it cite `search_pattern`, `find_symbol`, and `semantic_search`
rather than `grep`. You can also check installed plugins:

```bash
claude /plugin list
```

`code-explorer-routing@sdd-misc-plugins` should appear in the output.

## What Each Hook Does

### SessionStart

Injects the following guidance at the start of every session:

- Prefer `search_pattern` over `grep` for regex search across files.
- Prefer `list_symbols` and `find_symbol` over `cat`/`Read` when exploring code structure.
- Prefer `list_dir` over `ls` and `find_file` over `find`.
- Use `semantic_search` when looking for code by concept rather than by name.
- Reserve built-in file tools for writing new content and reading files that code-explorer does
  not index (binary files, generated artifacts, etc.).

This guidance is injected as a system-level note, not as a user message, so it does not clutter
the conversation.

### SubagentStart

Claude Code spawns subagents for parallel or delegated tasks. Each subagent is a fresh context
with no knowledge of earlier instructions. The SubagentStart hook fires when each subagent
initialises and injects the same tool selection guide, ensuring consistent behaviour across the
full agent tree.

Without this hook, subagents reliably fall back to `grep` and `cat` because they have no other
frame of reference.

### PreToolUse

The interception hook fires before any of these built-in tools execute:

| Built-in called | Redirected to |
|---|---|
| `grep` | `search_pattern` |
| `Read` | `list_symbols` or `find_symbol` (for source files) |
| `cat` | `list_symbols` or `find_symbol` (for source files) |
| `find` | `find_file` |
| `ls` | `list_dir` |

The hook does not blindly block all uses of these tools. It applies heuristics to distinguish
between reading source code (redirect) and reading configuration, logs, or other non-code files
(allow through). You can inspect the redirection logic in the plugin source if you need to adjust
the heuristics for your workflow.

## Disabling the Plugin

To turn off the plugin without uninstalling it, set its value to `false` in settings:

```json
{
  "enabledPlugins": {
    "code-explorer-routing@sdd-misc-plugins": false
  }
}
```

Or uninstall it entirely:

```bash
claude /plugin uninstall code-explorer-routing@sdd-misc-plugins
```
