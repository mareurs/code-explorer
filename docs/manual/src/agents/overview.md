# Agent Integrations

codescout works with any MCP-capable coding agent. Once registered as an MCP
server, codescout's system prompt injects automatically into every session,
giving the agent tool selection rules and iron laws for code navigation.

## Feature comparison

| Feature | Claude Code | GitHub Copilot | Cursor |
|---------|-------------|----------------|--------|
| MCP protocol | stdio | stdio | stdio |
| System prompt injection | Automatic | Automatic | Automatic |
| Tool enforcement (routing plugin) | Plugin with hooks | Copilot Skill guidance | Cursor Rules guidance |
| Workspace support | Full | Full | Full |
| Onboarding | Automatic | Automatic | Automatic |

## Guides

| Agent | Guide |
|---|---|
| Claude Code | [Claude Code](claude-code.md) — primary integration with routing plugin enforcement |
| GitHub Copilot | [GitHub Copilot](copilot.md) — VS Code extension with Skills-based guidance |
| Cursor | [Cursor](cursor.md) — Cursor Rules-based guidance |
