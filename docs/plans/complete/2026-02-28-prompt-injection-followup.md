# Follow-up: guidance.txt deleted + WORKTREES gap

**Re:** `2026-02-28-prompt-injection-design.md`
**Date:** 2026-02-28

---

## Q1 is answered

We tested empirically: spawned a subagent with the `SubagentStart` hook fully silenced
(zero injection), gave it a Rust codebase task with no code-explorer instructions in the
prompt. It navigated using `list_symbols`, `find_symbol`, and `semantic_search` — not
`grep` or `Read`. **Server instructions reach every subagent via their own fresh MCP session.**

As a result: we deleted `guidance.txt` from the plugin. It was a redundant manual copy of
`server_instructions.md`. The plugin now only injects dynamic, project-specific content
(`system-prompt.md`, memory hints, drift warnings) — the things `server_instructions` can't carry.

---

## One gap this exposed

`guidance.txt` contained a WORKTREES section that `server_instructions.md` does not:

```
WORKTREES:
  After EnterWorktree, ALWAYS call activate_project("/abs/worktree/path") before
  using any code-explorer tools. code-explorer tracks its own active project
  independently of Bash CWD — they are NOT automatically coupled.
  MCP write tools are HARD-BLOCKED until activate_project is called.
```

Without this, agents enter worktrees via `EnterWorktree` and hit the write guard's hard
block without understanding why. The plugin's reactive reminder (fires when a session
*starts already inside* a worktree) doesn't cover the proactive case.

**Ask:** add a `## Worktrees` section to `src/prompts/server_instructions.md` with the
content above (or your preferred wording). Since subagents now get server_instructions
confirmed, this reaches the full agent tree automatically.

---

## Worktree cleanup — agent gets stuck (new issue)

Observed in the wild: agent tries to clean up a finished worktree, the directory was
already deleted (Claude Code cleaned it up automatically), and the session locks up.

**What happens:**
1. Agent calls `git worktree remove <path>` — fails, directory already gone
2. Agent retries variations of the same command, all fail with the same error
3. Claude Code's Bash tool validates CWD before executing *any* command — once
   the worktree directory is deleted, the entire Bash tool is dead for that session
4. Agent loops until interrupted

**Root cause:** `git worktree remove` requires the directory to exist.
The correct command for an already-deleted directory is `git worktree prune`.

**Fix for the stuck session:** there is none — the session is unrecoverable once the
CWD is invalid. The user must open a plain terminal and run:
```bash
git -C /path/to/main/repo worktree prune
```
Then start a fresh Claude Code session from the main repo directory.

**Prevention — two asks:**

1. **In the routing plugin's write guard message** (we'll handle this on our side):
   add cleanup instructions so agents know `prune` vs `remove` before they get stuck.

2. **In `server_instructions.md` Worktrees section** (your side, same ask as above):
   add a cleanup note, e.g.:
   ```
   To clean up a finished worktree: run `git worktree prune` from the main repo
   (not `git worktree remove` — that requires the directory to still exist).
   Then start a new session from the main repo directory.
   ```

The second point prevents the loop from ever starting — if the agent knows `prune`
upfront, it won't spiral on `remove`.
