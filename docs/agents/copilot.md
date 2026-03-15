# GitHub Copilot

codescout works with GitHub Copilot Chat in VS Code via the MCP protocol. Once registered,
Copilot uses codescout's symbol and semantic tools for all code navigation instead of reading
source files directly.

---

## One-Time Setup

**Prerequisites:** VS Code (latest), GitHub Copilot subscription (Individual, Business, or
Enterprise), Rust toolchain.

**Install codescout:**

```bash
cargo install codescout
```

The binary lands at `~/.cargo/bin/codescout`. Make sure `~/.cargo/bin` is in your `PATH`.

Next, clone the [codescout-companion](https://github.com/mareurs/codescout-companion) repository — this provides the workflow skills, enforcement hook, and code-reviewer agent used in the steps below. The commands use `path/to/copilot-codescout` as a placeholder for wherever you cloned it.

### Register codescout as an MCP server

**Recommended: user-level registration** — codescout becomes available in all your projects
without per-project config. Edit `~/.config/Code/User/mcp.json` (create if it doesn't exist):

```json
{
  "servers": {
    "codescout": {
      "type": "stdio",
      "command": "codescout",
      "args": ["start"]
    }
  },
  "inputs": []
}
```

> **Note:** VS Code uses `"servers"` not `"mcpServers"`. If `cargo` is not in PATH, use the
> full path as the `command` value (e.g. `~/.cargo/bin/codescout`).

**Alternative: per-project registration** — create `.vscode/mcp.json` in your project root
with the same `servers` block. Use this if you want codescout only for specific projects.

> **VS Code schema validation note:** VS Code validates MCP tool schemas more strictly than
> Claude Code. If you see a warning like *"Failed to validate tool … array type must have
> items"*, it is a schema bug in the MCP server — other tools still load and work normally.
> Report it to the codescout maintainer.

Once registered, codescout's system prompt injects automatically into every Copilot Chat
session, giving Copilot iron laws for code navigation, a tool selection table, anti-patterns
to avoid, and the output buffer system for large results.

### Enable Agent Skills

1. Open **Settings** (`Ctrl+,` / `Cmd+,`)
2. Search for `chat.useAgentSkills`
3. Enable it

Copilot will now auto-detect and load skill files when relevant to your request.

### Add workflow skills

VS Code Copilot discovers skills in `.github/skills/`.

```bash
# Option A: Symlink (recommended — stays in sync with updates)
# NOTE: do NOT mkdir .github/skills first — ln creates the symlink as the target name
mkdir -p .github
ln -s path/to/copilot-codescout/Skills .github/skills

# Option B: Copy (standalone, no sync)
mkdir -p .github/skills
cp -r path/to/copilot-codescout/Skills/* .github/skills/
```

Skills are discovered automatically — no further configuration needed.

### Add the code-reviewer agent

VS Code Copilot discovers agents in `.github/agents/`.

```bash
mkdir -p .github/agents
cp path/to/copilot-codescout/Agents/code-reviewer.agent.md .github/agents/
```

---

## Enforcement Hook

The enforcement hook blocks Copilot from reading source files directly and redirects it to
codescout tools. Requires Python 3.

```bash
mkdir -p .github/hooks
cp path/to/copilot-codescout/Hooks/enforce-codescout.py .github/hooks/
cp path/to/copilot-codescout/Hooks/enforce-codescout.json .github/hooks/
```

This installs a `PreToolUse` hook that:

- **Blocks** `read/readFile` on source files (`.ts`, `.js`, `.py`, `.rs`, etc.) and redirects
  to the appropriate codescout tool
- **Blocks** `search/codebase` and redirects to `mcp__codescout__semantic_search`
- **Allows** reading config files, markdown, JSON, YAML, and lock files

---

## Multi-Project Workspaces

codescout supports multi-project workspaces via `.codescout/workspace.toml`.
After onboarding, pass `project` to scope tool calls to a specific project:

```json
{ "tool": "find_symbol", "arguments": { "pattern": "UserService", "project": "backend" } }
```

See [Multi-Project Workspaces](../manual/src/concepts/multi-project-workspace.md).

---

## Always-On Instructions

```bash
cp path/to/copilot-codescout/copilot-instructions.md .github/copilot-instructions.md
```

VS Code Copilot injects `.github/copilot-instructions.md` into every chat session
automatically, giving Copilot the codescout iron laws and tool selection table before any
request.

> If `.github/copilot-instructions.md` already exists, append the codescout section rather
> than overwriting.

---

## Verify

Start a new Copilot Chat session and ask:

> "What symbols are in src/main.ts?"

Copilot should call `mcp__codescout__list_symbols` rather than reading the file directly.

---

## Day-to-Day Workflow

Skills activate automatically when their description matches the request — you never type a
slash command. The standard flow is:

```
brainstorming → using-git-worktrees → writing-plans → subagent-driven-development → finishing-a-development-branch
```

### Skill trigger table

| What you say | Skill that activates |
|---|---|
| "Let's build X" / "I want to add X" | `brainstorming` |
| "Create a worktree" / "New branch" | `using-git-worktrees` |
| "Write the plan" / "Break into tasks" | `writing-plans` |
| "Execute" / "Implement" | `subagent-driven-development` |
| "Review this" / "Before I merge" | `requesting-code-review` |
| "I'm done" / "All tasks complete" | `finishing-a-development-branch` |

### Step-by-step

**1. Brainstorming** — Describe what you want to build. Copilot activates the `brainstorming`
skill, explores the codebase via codescout (semantic search, symbol lookup), asks clarifying
questions one at a time, and proposes 2–3 approaches with trade-offs. No code is written until
you approve the design.

**2. Set up an isolated workspace** — Say "Create a worktree" or "Let's work on a branch."
The `using-git-worktrees` skill creates an isolated git worktree on a new branch, runs project
setup, and verifies the test baseline is clean before any code is written.

**3. Write the implementation plan** — Say "Write the plan" or "Break this into tasks." The
`writing-plans` skill produces a detailed `docs/plans/YYYY-MM-DD-feature.md` with every task
broken into 2–5 minute TDD steps: write the failing test, watch it fail, write minimal code,
watch it pass, commit.

**4. Execute the plan** — Say "Execute the plan" or "Implement it." The
`subagent-driven-development` skill dispatches a fresh sub-agent per task (clean context, no
drift). A spec compliance reviewer and a code quality reviewer both check each implementation
before the next task begins.

**5. Code review** — Happens automatically after each task in `subagent-driven-development`.
For ad-hoc review, say "Review this implementation." The `requesting-code-review` skill
dispatches the `code-reviewer` agent, which uses `find_references` and `semantic_search` to
check impact beyond the changed files.

**6. Finish the branch** — Say "I'm done" or "All tasks complete." The
`finishing-a-development-branch` skill verifies all tests pass, presents merge/PR/discard
options, and cleans up the worktree.

### Tips

- **Don't rush brainstorming.** The questions feel slow but they prevent far more work later.
- **Trust the spec reviewer.** If it says something is missing, it read the actual code — not
  the implementer's report.
- **Let codescout navigate.** If Copilot starts reading whole files, remind it: "use codescout
  to explore".
- **One task at a time.** The subagent-driven workflow is sequential by design — parallel tasks
  introduce conflicts.

---

## Updating Skills

```bash
cd path/to/copilot-codescout
git pull
# Symlink: already up to date.
# Copy: re-run the cp command.
```
