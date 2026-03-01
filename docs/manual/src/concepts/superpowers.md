# Superpowers Workflow

## What Is Superpowers

[Superpowers](https://github.com/obra/superpowers) is a Claude Code plugin that
wraps the full development lifecycle in composable skills that trigger
automatically. Rather than jumping straight into code, Claude steps back,
clarifies requirements, writes a spec, produces a detailed implementation plan,
then executes it task by task via subagents — each with two-stage review (spec
compliance, then code quality).

The core skill chain:

1. **brainstorming** — refines rough ideas, explores alternatives, validates design
2. **using-git-worktrees** — creates an isolated workspace on a new branch
3. **writing-plans** — breaks the design into 2–5 minute tasks with exact file paths and verification steps
4. **subagent-driven-development** / **executing-plans** — dispatches fresh subagents per task, or works in human-checkpoint batches
5. **finishing-a-development-branch** — merges, creates a PR, or discards; cleans up the worktree

## The Manual Worktree Workflow

For large implementation plans — or when working on two or three separate issues
on the same repo simultaneously — the safest approach is to skip `EnterWorktree`
entirely and do it manually:

```bash
# create the worktree on a new branch
git worktree add .claude/worktrees/my-feature my-feature

# cd into it and launch Claude from there
cd .claude/worktrees/my-feature
claude
```

Claude starts with its CWD already set to the worktree. The MCP server needs one
`activate_project` call to follow:

```
activate_project("/abs/path/to/.claude/worktrees/my-feature")
```

After that, every read, write, and shell command targets the worktree
automatically — no `EnterWorktree` inside the session, no risk of writing to the
main repo, no mid-session project-switch to forget.

**Why this beats `EnterWorktree` inside a running session:**

- The session CWD matches the project root from the start — there's no window
  where writes could land in the wrong tree
- You can run multiple independent Claude sessions in parallel, each in its own
  worktree, each working a different branch of the same repo
- The terminal tab itself is the isolation boundary — clear, auditable, easy to
  kill

## Running Parallel Sessions

When a plan is large and there are also 2–3 smaller issues to knock out at the
same time, parallel worktrees let you do all of it concurrently:

```bash
# terminal 1 — big feature
git worktree add .claude/worktrees/big-feature big-feature
cd .claude/worktrees/big-feature && claude

# terminal 2 — hotfix
git worktree add .claude/worktrees/hotfix-auth hotfix-auth
cd .claude/worktrees/hotfix-auth && claude

# terminal 3 — docs update
git worktree add .claude/worktrees/docs-update docs-update
cd .claude/worktrees/docs-update && claude
```

Each session is fully isolated. They share the object store (fast) and can see
each other's commits once pushed, but their working trees never interfere.

## The Prune Bug in `finishing-a-development-branch`

When the `finishing-a-development-branch` skill cleans up, it runs:

```bash
git worktree remove <worktree-path>
```

This works when Claude was launched from the main repo and entered the worktree
via `EnterWorktree`. It **fails** when you launched Claude from inside the
worktree — because `git worktree remove` refuses to remove a worktree whose path
is the current working directory of any running process.

The symptom: the skill completes the merge or PR, then hangs or errors on
cleanup, leaving a stale worktree entry in `.git/worktrees/`.

**The correct cleanup command when CWD is inside the worktree:**

```bash
# from the main repo — NOT from inside the worktree
git -C /abs/path/to/main-repo worktree prune
```

`git worktree prune` doesn't require the directory to exist or to be reachable
as CWD — it just reconciles `.git/worktrees/` against what's actually on disk.
If the worktree directory was already deleted (by the skill or manually), `prune`
clears the stale entry cleanly. `git worktree remove` cannot do this.

**Practical fix:** After a session that was launched from inside a worktree,
if cleanup fails, exit back to the main repo and run `git worktree prune`
manually. The branch itself may need a separate `git branch -d my-feature` if
you want it gone entirely.
