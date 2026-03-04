# Git Worktree Support

## The Core Problem

Claude Code's `EnterWorktree` creates an isolated git worktree for feature work,
and the shell's working directory moves into it. The MCP server does not follow.

codescout's project root is set when the server starts (or when
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

## Layer 2 — Navigation Exclusions

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

## Plan Execution Gotcha: Start a New Session in the Worktree

When using a workflow like [Superpowers writing-plans](superpowers.md) and
choosing the **Parallel Session** option, don't try to launch `executing-plans`
from the same session that created the worktree. The `EnterWorktree` +
`activate_project` dance is easy to miss, and subagents spawned from the current
session won't automatically inherit the right project root.

The cleanest approach — one that sidesteps all of this — is:

```bash
cd /path/to/.worktrees/<feature-branch>
claude
```

Open a new terminal, `cd` into the worktree, and start Claude there. The session
is rooted in the worktree from the first message. No `activate_project` call
needed, no stale context from the planning session, no risk of writes going to
the main repo.

Other approaches can work, but this one always does.

## Further Reading

- [Superpowers Workflow](superpowers.md) — how the Superpowers plugin integrates
  worktrees into a full TDD + parallel-agent development workflow
- [Workflow & Config Tools](../tools/workflow-and-config.md) — `activate_project`
  reference: the required call after entering a worktree
