# Cursor

codescout works with Cursor Agent chat via the MCP protocol. Once registered,
Cursor uses codescout's symbol and semantic tools for all code navigation instead of reading
source files directly.

---

## One-Time Setup

**Prerequisites:** Cursor (latest), Rust toolchain.

**Install codescout:**

```bash
cargo install codescout
```

The binary lands at `~/.cargo/bin/codescout`. Make sure `~/.cargo/bin` is in your `PATH`.

Next, clone the [codescout-companion](https://github.com/mareurs/codescout-companion) repository — this provides the workflow skills, enforcement hook, and code-reviewer agent used in the steps below. The commands use `path/to/copilot-codescout` as a placeholder for wherever you cloned it.

### Register codescout as an MCP server

**Recommended: project-level registration** — checked into git, shared with your team. Create
`.cursor/mcp.json` in your project root:

```json
{
  "mcpServers": {
    "codescout": {
      "command": "codescout",
      "args": ["start"]
    }
  }
}
```

> **Note:** Cursor uses `"mcpServers"` (with `s`), unlike VS Code's `"servers"`. If `cargo`
> is not in PATH, use the full path as the `command` value (e.g. `~/.cargo/bin/codescout`).

**Alternative: global registration** — available in all projects. Open Cursor → Settings →
Cursor Settings → MCP → Add new server → paste the same config block.

Once registered, codescout's system prompt injects automatically into every Agent session.

### Add workflow skills

Cursor discovers rules in `.cursor/rules/`. Two options:

**Option A — Convert each skill to `.mdc` format (recommended):**

For each skill in `path/to/copilot-codescout/Skills/`, create a corresponding
`.cursor/rules/<name>.mdc` file:

```bash
mkdir -p .cursor/rules
```

Each `.mdc` file follows this format:

```markdown
---
description: [paste the description from skill frontmatter]
globs:
alwaysApply: false
---

[paste the full skill body here]
```

Set `alwaysApply: false` — Cursor auto-applies rules based on description matching, the same
model-invoked behavior as Copilot Skills.

**Option B — Entry-point rule with `alwaysApply: true`:**

Add the `using-superpowers` skill content to `.cursor/rules/using-superpowers.mdc` with
`alwaysApply: true`. This tells the agent to check for relevant rules before any task,
mirroring the Claude Code hook behavior.

### Add the code-reviewer agent

```bash
mkdir -p .cursor/agents
cp path/to/copilot-codescout/Agents/code-reviewer.agent.md .cursor/agents/
```

---

## Enforcement Hook

The enforcement hook blocks Cursor from reading source files directly and redirects it to
codescout tools. Requires Python 3.

```bash
mkdir -p .cursor/hooks
cp path/to/copilot-codescout/Hooks/enforce-codescout.py .cursor/hooks/
cp path/to/copilot-codescout/Hooks/enforce-codescout.json .cursor/hooks/
```

This installs a `PreToolUse` hook that:

- **Blocks** `read/readFile` on source files (`.ts`, `.js`, `.py`, `.rs`, etc.) and redirects
  to the appropriate codescout tool
- **Blocks** `search/codebase` and redirects to `mcp__codescout__semantic_search`
- **Allows** reading config files, markdown, JSON, YAML, and lock files

---

## Verify

Start a new Agent chat and ask:

> "What symbols are in src/main.ts?"

The agent should call `mcp__codescout__list_symbols` rather than reading the file directly.

---

## Day-to-Day Workflow

Skills activate automatically when their description matches the request — you never type a
slash command. The standard flow is:

```
brainstorming → using-git-worktrees → writing-plans → subagent-driven-development → finishing-a-development-branch
```

### Rule trigger table

| What you say | Rule that activates |
|---|---|
| "Let's build X" / "I want to add X" | `brainstorming` |
| "Create a worktree" / "New branch" | `using-git-worktrees` |
| "Write the plan" / "Break into tasks" | `writing-plans` |
| "Execute" / "Implement" | `subagent-driven-development` |
| "Review this" / "Before I merge" | `requesting-code-review` |
| "I'm done" / "All tasks complete" | `finishing-a-development-branch` |

### Step-by-step

**1. Brainstorming** — Describe what you want to build. The agent activates the `brainstorming`
rule, explores the codebase via codescout (semantic search, symbol lookup), asks clarifying
questions one at a time, and proposes 2–3 approaches with trade-offs. No code is written until
you approve the design.

**2. Set up an isolated workspace** — Say "Create a worktree" or "Let's work on a branch."
The `using-git-worktrees` rule creates an isolated git worktree on a new branch, runs project
setup, and verifies the test baseline is clean before any code is written.

**3. Write the implementation plan** — Say "Write the plan" or "Break this into tasks." The
`writing-plans` rule produces a detailed `docs/plans/YYYY-MM-DD-feature.md` with every task
broken into 2–5 minute TDD steps: write the failing test, watch it fail, write minimal code,
watch it pass, commit.

**4. Execute the plan** — Say "Execute the plan" or "Implement it." The
`subagent-driven-development` rule dispatches a fresh sub-agent per task (clean context, no
drift). A spec compliance reviewer and a code quality reviewer both check each implementation
before the next task begins.

**5. Code review** — Happens automatically after each task in `subagent-driven-development`.
For ad-hoc review, say "Review this implementation." The `requesting-code-review` rule
dispatches the `code-reviewer` agent, which uses `find_references` and `semantic_search` to
check impact beyond the changed files.

**6. Finish the branch** — Say "I'm done" or "All tasks complete." The
`finishing-a-development-branch` rule verifies all tests pass, presents merge/PR/discard
options, and cleans up the worktree.

### Tips

- **`.cursor/rules/` is your friend.** If a rule isn't triggering, check that the `.mdc` file
  exists and the `description` field is specific enough.
- **Context window.** Cursor Agent can lose context on very long sessions. The
  subagent-driven approach (fresh agent per task) is specifically designed to prevent this.
- **Let codescout navigate.** If the agent starts reading whole files instead of using symbol
  tools, say: "use codescout to explore this".
- **Compose with existing Cursor Rules.** Your team's `.cursor/rules/` files for style,
  architecture, or tooling sit alongside the codescout skill rules — they don't conflict.
- **Check MCP is active.** Open Cursor Settings → MCP and verify the server shows a green
  status indicator.

---

## Multi-Project Workspaces

codescout supports multi-project workspaces via `.codescout/workspace.toml`.
After onboarding, pass `project` to scope tool calls to a specific project:

```json
{ "tool": "find_symbol", "arguments": { "pattern": "UserService", "project": "backend" } }
```

See [Multi-Project Workspaces](../manual/src/concepts/multi-project-workspace.md).

---

## Cursor-Specific Notes

### Rules vs Skills

Cursor calls them "Rules" (`.cursor/rules/*.mdc`). They are functionally identical to Copilot
Skills — model-invoked based on description matching. Same content, different filename
extension and location.

### Agent Chat vs Background Agent

- **Agent Chat** (Cmd+L → Agent): Interactive, sees your conversation. Use this for the full
  workflow.
- **Background Agent**: Headless task runner. Good for executing a specific isolated task from
  the plan, but lacks the interactive brainstorming/review loop.

Use **Agent Chat** for the full codescout workflow.

### Plan Mode

Cursor has a built-in "Plan Mode" (the thinking icon in Agent chat). The `brainstorming` rule
replaces this for structured design work — it's more thorough and saves a design doc. For
quick one-off tasks, Plan Mode is fine.

### Feature comparison

| Feature | GitHub Copilot | Cursor |
|---|---|---|
| Skills location | `.github/skills/<name>/SKILL.md` | `.cursor/rules/<name>.mdc` |
| Skills activation | `chat.useAgentSkills: true` | `alwaysApply: false` per rule |
| MCP config key | `"servers"` | `"mcpServers"` |
| Per-project config | `.vscode/mcp.json` | `.cursor/mcp.json` |
| Agent config location | `.github/agents/` | `.cursor/agents/` |
| Always-on instructions | `.github/copilot-instructions.md` | `.cursor/rules/*.mdc` with `alwaysApply: true` |
