# Git Worktree Support

## The Core Problem

Claude Code's `EnterWorktree` creates an isolated git worktree for feature work,
and the shell's working directory moves into it. The MCP server does not follow.

code-explorer's project root is set when the server starts (or when
`activate_project` is called). It has no visibility into where the shell is
currently pointed. So after `EnterWorktree`, write tools — `edit_file`,
`create_file`, `replace_symbol`, `insert_code`, `remove_symbol` — are still
targeting the main repo. The AI writes to the wrong tree, silently, with no
error, because the path is valid in both contexts.

## The Fix: `activate_project`

After `EnterWorktree`, always call `activate_project` with the absolute worktree
path before doing any writes:

```
activate_project("/abs/path/to/.claude/worktrees/my-feature")
```

All subsequent reads, writes, symbol navigation, and shell commands then target
that tree. Switch back to the main repo when done:

```
activate_project("/abs/path/to/main-repo")
```

## Layer 1 — Write Guard (Hard Block)

If the AI enters a worktree but hasn't called `activate_project`, write tools
detect the mismatch and raise a hard error rather than silently writing to the
wrong place. The error message lists the detected worktree paths and the exact
`activate_project` call needed to unblock.

This is the last line of defence — `worktree_hint` (below) should catch
the situation earlier.

## Layer 2 — Worktree Hint on Write Responses

After every successful write, if linked worktrees exist under the active project
root, the response JSON includes an advisory `"worktree_hint"` field:

```json
{
  "worktree_hint": "This repo has linked worktrees: [/repo/.claude/worktrees/my-feature].
                    If you meant to edit a worktree, call activate_project first."
}
```

This surfaces immediately after the first write in a worktree session. The AI
can self-correct before the mistake compounds. Zero overhead when no worktrees
exist — it's a single `.git/worktrees/` directory check that returns `None` on
the fast path.

## Layer 3 — Navigation Exclusions

Worktree directories (`.claude/worktrees/`, `.worktrees/`) are excluded from
`find_file` and `list_dir` results. Without this, file searches and directory
listings in the main project would surface duplicate copies of every source
file from every active worktree — polluting navigation and confusing symbol
lookups.

## Cleanup Gotcha

`git worktree remove <path>` requires the directory to still exist. If the
worktree directory was already deleted (e.g. by the agent or a cleanup script),
`worktree remove` will fail. The correct command for an already-gone directory
is:

```bash
git worktree prune
```

Run this from the main repo root, not from inside the (now-deleted) worktree.
