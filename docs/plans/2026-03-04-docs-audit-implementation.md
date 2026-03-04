# Documentation Audit Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix all documentation gaps discovered in the audit — wrong tool counts, missing GitHub tools reference, undocumented `remove_symbol`, misplaced `create_file`/`edit_file`, missing FEATURES.md entries, and stale count references.

**Architecture:** Two passes. Pass 1 is targeted edits to existing files (fast, verifiable). Pass 2 creates the new GitHub tools reference page and wires it in. All changes are markdown only — no code.

**Tech Stack:** Markdown only.

**Important — what NOT to change:**
- `.code-explorer/` directory path references — that is the actual config directory name on disk, correct and intentional
- `code-explorer-routing` plugin references — that is the plugin's actual ID, not a stale reference
- `history.md` and `CHANGELOG.md` — use `code-explorer` intentionally as historical context

---

## PASS 1 — Factual Corrections

---

### Task 1: Fix tool count in README

**Files:**
- Modify: `README.md`

The README currently says "**23 tools total**" (line 34) and "## Tools (23)" (around line 213). Both need updating to 28.

**Step 1: Update the intro paragraph**

Find this line:
```
Plus file operations (6 tools), workflow (2 tools), and config & navigation (3 tools) — **23 tools total**.
```

Replace with:
```
Plus file operations (6 tools), workflow (2 tools), config & navigation (3 tools), and GitHub integration (5 tools) — **28 tools total**.
```

**Step 2: Update the section heading**

Find: `## Tools (23)`
Replace with: `## Tools (28)`

**Step 3: Verify**

Run: `grep -n "23 tools\|Tools (23)\|23 total" README.md`
Expected: no matches

**Step 4: Commit**

```bash
git add README.md
git commit -m "docs: update README tool count 23 → 28"
```

---

### Task 2: Add GitHub to README tools table and "What sets it apart"

**Files:**
- Modify: `README.md`

**Step 1: Add GitHub row to the tools table**

The current table ends with:
```
| Config & Navigation | 3 | `activate_project`, `project_status`, `list_libraries` |
```

Add a new row after it:
```
| GitHub | 5 | `github_identity`, `github_issue`, `github_pr`, `github_file`, `github_repo` |
```

**Step 2: Add a bullet to "What sets it apart"**

The current "What sets it apart" section ends with:
```
- **Progressive disclosure** — every tool defaults to compact output; `detail_level: "full"` + pagination unlocks everything. No accidental context floods.
```

Add after it:
```
- **GitHub integration** — `github_issue`, `github_pr`, `github_file`, and `github_repo` give the AI authenticated access to GitHub — read and write issues, review PRs, push files, search code — without leaving the coding session.
```

**Step 3: Update the intro line (line 3) to mention GitHub**

Find:
```
Rust MCP server giving LLMs IDE-grade code intelligence — symbol navigation, semantic search, git blame, shell integration, and persistent memory. Built for [Claude Code](https://code.claude.com/).
```

Replace with:
```
Rust MCP server giving LLMs IDE-grade code intelligence — symbol navigation, semantic search, shell integration, persistent memory, and GitHub integration. Built for [Claude Code](https://code.claude.com/).
```

**Step 4: Verify the table renders correctly**

Read the section back: `grep -A 10 "## Tools" README.md`
Expected: 7-row table (6 categories + GitHub), correct counts.

**Step 5: Commit**

```bash
git add README.md
git commit -m "docs: add GitHub tools to README — table, what sets it apart, intro"
```

---

### Task 3: Fix tool count and add GitHub category to overview.md

**Files:**
- Modify: `docs/manual/src/tools/overview.md`

**Step 1: Fix the intro count**

Find (line 3):
```
codescout exposes 23 tools organized into six categories.
```

Replace with:
```
codescout exposes 28 tools organized into seven categories.
```

**Step 2: Add GitHub section at the end of the tool tables (before "Which Tool Do I Use?")**

Find the text:
```
---

## Which Tool Do I Use?
```

Insert before it:
```
---

## [GitHub](github.md)

Authenticated access to GitHub repositories, issues, pull requests, and files.
Requires a GitHub token configured in your environment.

| Tool | Description |
|------|-------------|
| `github_identity` | Get authenticated user profile, search users, list teams and members |
| `github_issue` | List, search, get, create, and update issues and comments |
| `github_pr` | List, search, get diffs, review, and merge pull requests |
| `github_file` | Get, create, update, delete files and push multi-file commits |
| `github_repo` | Search repos, manage branches, commits, releases, tags, and code search |

---

```

**Step 3: Add GitHub rows to the "Which Tool Do I Use?" decision table**

At the end of the decision table, before the closing line, add:
```
| View a GitHub issue | `github_issue` with `method: "get"` |
| Create a GitHub issue | `github_issue` with `method: "create"` |
| Review a pull request | `github_pr` with `method: "get_diff"` or `"get_files"` |
| Push a file to GitHub | `github_file` with `method: "create_or_update"` |
| Search GitHub code | `github_repo` with `method: "search_code"` |
```

**Step 4: Commit**

```bash
git add docs/manual/src/tools/overview.md
git commit -m "docs: add GitHub category to tools overview, fix count to 28"
```

---

### Task 4: Add `remove_symbol` to symbol-navigation.md

**Files:**
- Modify: `docs/manual/src/tools/symbol-navigation.md`

`remove_symbol` is registered and listed in the overview table but has no entry in `symbol-navigation.md`. The file currently ends with `## \`hover\``. Add `remove_symbol` after `rename_symbol` and before `goto_definition` (logical grouping: the write tools together).

**Step 1: Locate the insertion point**

The entry for `rename_symbol` ends around line 495. `goto_definition` starts at line 496. Insert the new `remove_symbol` section between them.

Find this exact text (end of rename_symbol section):
```
- After renaming, always verify the build compiles — `run_command("cargo build")` or equivalent.
- The text sweep covers comments and strings, but cannot reason about dynamic dispatch or reflection-based usage. Rename those manually if present.

---

## `goto_definition`
```

**Step 2: Insert `remove_symbol` section**

Replace the text above with:

```markdown
- After renaming, always verify the build compiles — `run_command("cargo build")` or equivalent.
- The text sweep covers comments and strings, but cannot reason about dynamic dispatch or reflection-based usage. Rename those manually if present.

---

## `remove_symbol`

**Purpose:** Delete a named symbol (function, struct, method, test, etc.) entirely from a file.
Uses LSP to identify the exact line range covered by the symbol — no manual line counting required.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `name_path` | string | yes | — | Symbol identifier, e.g. `"MyStruct/my_method"` or `"old_helper"` |
| `path` | string | yes | — | File containing the symbol |

**Example — delete a deprecated function:**

```json
{
  "tool": "remove_symbol",
  "arguments": {
    "name_path": "legacy_auth_check",
    "path": "src/auth/middleware.rs"
  }
}
```

**Output:**

```json
"ok"
```

**Tips:**

- Use `find_references` first to confirm nothing still calls the symbol before removing it.
- For methods on a struct or class, use the full path: `"MyStruct/my_method"`.
- The tool removes the exact LSP range — it will not leave behind stray blank lines from adjacent doc comments if they fall outside the symbol's range. Review the diff after removal.
- If you want to replace rather than delete, use `replace_symbol` instead.

---

## `goto_definition`
```

**Step 3: Verify**

Run: `grep -n "remove_symbol" docs/manual/src/tools/symbol-navigation.md`
Expected: at least 3 matches (heading, name_path example, tip).

**Step 4: Commit**

```bash
git add docs/manual/src/tools/symbol-navigation.md
git commit -m "docs: add remove_symbol reference entry to symbol-navigation.md"
```

---

### Task 5: Add `create_file` and `edit_file` to file-operations.md

**Files:**
- Modify: `docs/manual/src/tools/file-operations.md`

These tools are fully documented in `editing.md`. Add concise entries to `file-operations.md` (their home as file tools) with a cross-reference to `editing.md` for full details.

**Step 1: Add entries at the end of file-operations.md**

The file currently ends with the `find_file` section (line 229). Append after it:

```markdown

---

## `create_file`

**Purpose:** Create a new file or overwrite an existing file with given content.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | File path relative to project root |
| `content` | string | yes | — | Full file content to write |

**Example:**

```json
{
  "tool": "create_file",
  "arguments": {
    "path": "src/utils/helpers.rs",
    "content": "pub fn clamp(v: f64, min: f64, max: f64) -> f64 {\n    v.max(min).min(max)\n}\n"
  }
}
```

**Output:** `"ok"`

**Tips:**
- Creates parent directories if they don't exist.
- Overwrites without warning — check that the path is correct before writing.
- For editing existing files, use `edit_file` instead.

See [Editing](editing.md#create_file) for more usage guidance.

---

## `edit_file`

**Purpose:** Find-and-replace editing within an existing file. Matches an exact string and replaces it — whitespace-sensitive.

**Parameters:**

| Name | Type | Required | Default | Description |
|------|------|----------|---------|-------------|
| `path` | string | yes | — | File path relative to project root |
| `old_string` | string | yes | — | Exact text to find (must match including whitespace) |
| `new_string` | string | yes | — | Replacement text |
| `replace_all` | boolean | no | `false` | Replace every occurrence instead of just the first |
| `insert` | string | no | — | `"prepend"` or `"append"` — add text at the start/end of the file |

**Example — change an import:**

```json
{
  "tool": "edit_file",
  "arguments": {
    "path": "src/main.rs",
    "old_string": "use crate::utils::old_helper;",
    "new_string": "use crate::utils::new_helper;"
  }
}
```

**Output:** `"ok"`

**Tips:**
- `old_string` must match exactly — including indentation and line endings.
- Use for imports, constants, config values, and small literal changes.
- For changes to a function or struct body, prefer `replace_symbol` — it's robust to line number shifts.

See [Editing](editing.md#edit_file) for full parameter details and more examples.
```

**Step 2: Verify**

Run: `grep -n "create_file\|edit_file" docs/manual/src/tools/file-operations.md`
Expected: multiple matches including the new headings.

**Step 3: Commit**

```bash
git add docs/manual/src/tools/file-operations.md
git commit -m "docs: add create_file and edit_file entries to file-operations.md"
```

---

### Task 6: Add `goto_definition`, `hover`, and GitHub to FEATURES.md

**Files:**
- Modify: `docs/FEATURES.md`

**Step 1: Add goto_definition + hover section**

After the `## Kotlin LSP (JetBrains Official)` section (which currently ends around line 216), insert:

```markdown
---

## `goto_definition` and `hover`

Two LSP-backed point-in-file tools that mirror what an IDE shows in the gutter.

**`goto_definition`** — takes a file path and 1-indexed line number, returns the definition location for the symbol at that position. When the definition lives outside the project root (e.g. in a Rust dependency), it auto-discovers and registers the library source so subsequent symbol navigation works on it too.

**`hover`** — takes a file path and line number, returns the type signature and documentation for the symbol at that position. Surfaces the same information as hovering in VS Code: inferred types, generic bounds, doc comments.

Both tools require a running LSP server for the target language.

---
```

**Step 2: Add GitHub tools section**

At the end of `FEATURES.md`, append:

```markdown
## GitHub Integration (5 tools)

Five consolidated tools giving the AI authenticated read/write access to GitHub — replacing the need to shell out to `gh` for common operations.

| Tool | Operations |
|------|------------|
| `github_identity` | `get_me`, `search_users`, `get_teams`, `get_team_members` |
| `github_issue` | `list`, `search`, `get`, `get_comments`, `create`, `update`, `add_comment` |
| `github_pr` | `list`, `get`, `get_diff`, `get_files`, `create_review`, `submit_review`, `merge` |
| `github_file` | `get`, `create_or_update`, `delete`, `push_files` |
| `github_repo` | `list_branches`, `create_branch`, `list_commits`, `list_releases`, `search_code` |

**Key design:** `get_diff` and `get_commit` return `@tool_*` buffer handles rather than raw text — diffs are large and would flood context. Query them with `run_command("grep pattern @tool_id")` as with any other buffer.

**Authentication:** Requires a `GITHUB_TOKEN` environment variable (or equivalent configured in the MCP host).

See [GitHub tools reference](manual/src/tools/github.md) for full parameter docs.
```

**Step 3: Verify**

Run: `grep -n "goto_definition\|hover\|GitHub" docs/FEATURES.md`
Expected: multiple matches across both new sections.

**Step 4: Commit**

```bash
git add docs/FEATURES.md
git commit -m "docs: add goto_definition, hover, and GitHub tools sections to FEATURES.md"
```

---

### Task 7: Fix count in manual introduction.md

**Files:**
- Modify: `docs/manual/src/introduction.md`

**Step 1: Search for stale "23" references**

Run: `grep -n "23" docs/manual/src/introduction.md`

If any mention "23 tools" or "23 tool", update to 28.

**Step 2: Commit if changes made**

```bash
git add docs/manual/src/introduction.md
git commit -m "docs: fix tool count in introduction.md"
```

If no count references found, skip the commit and note it.

---

## PASS 2 — GitHub Tools Reference Page

---

### Task 8: Create docs/manual/src/tools/github.md

**Files:**
- Create: `docs/manual/src/tools/github.md`

Write the full reference page for all 5 GitHub tools. Use `src/prompts/server_instructions.md` as the source of truth for parameters (the GitHub section starting at the `### GitHub` heading).

**Step 1: Create the file**

```markdown
# GitHub Tools

codescout includes five tools for authenticated GitHub access — reading and writing issues,
pull requests, files, and repository metadata. These tools require a GitHub token configured
in your MCP host environment (`GITHUB_TOKEN` or equivalent).

---

## `github_identity`

**Purpose:** Identity and team operations — get the authenticated user, search GitHub users,
or inspect team membership.

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `method` | string | yes | `get_me` \| `search_users` \| `get_teams` \| `get_team_members` |
| `query` | string | for `search_users` | Search query |
| `org` | string | for `get_team_members` | Organization login |
| `team_slug` | string | for `get_team_members` | Team slug |

**Methods:**
- `get_me` — returns the authenticated user's profile (login, name, email, bio)
- `search_users` — search GitHub users by query string
- `get_teams` — list all teams the authenticated user belongs to
- `get_team_members` — list members of a specific team (requires `org` + `team_slug`)

**Example:**

```json
{
  "tool": "github_identity",
  "arguments": { "method": "get_me" }
}
```

---

## `github_issue`

**Purpose:** Read and write GitHub issues and comments.

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `method` | string | yes | See methods below |
| `owner` | string | most methods | Repository owner (user or org) |
| `repo` | string | most methods | Repository name |
| `number` | integer | single-issue methods | Issue number |
| `title` | string | `create` | Issue title |
| `body` | string | `create`/`update`/`add_comment` | Issue or comment body |
| `state` | string | `list`/`update` | `"open"` or `"closed"` |
| `labels` | string | `list`/`create`/`update` | Comma-separated label names |
| `assignees` | string | `create`/`update` | Comma-separated login names |
| `query` | string | `search` | Search query |
| `limit` | integer | `list`/`search` | Max results (default 30) |

**Read methods:** `list` \| `search` \| `get` \| `get_comments` \| `get_labels` \| `get_sub_issues`

**Write methods:** `create` \| `update` \| `add_comment` \| `add_sub_issue` \| `remove_sub_issue`

**Example — create an issue:**

```json
{
  "tool": "github_issue",
  "arguments": {
    "method": "create",
    "owner": "acme",
    "repo": "myapp",
    "title": "Fix null pointer in auth middleware",
    "body": "Reproduces when the token is missing the `sub` claim."
  }
}
```

**Example — list open issues:**

```json
{
  "tool": "github_issue",
  "arguments": {
    "method": "list",
    "owner": "acme",
    "repo": "myapp",
    "state": "open",
    "limit": 10
  }
}
```

---

## `github_pr`

**Purpose:** Read and write pull requests — including diffs, reviews, and merges.

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `method` | string | yes | See methods below |
| `owner` | string | most methods | Repository owner |
| `repo` | string | most methods | Repository name |
| `number` | integer | single-PR methods | PR number |
| `title` | string | `create`/`update` | PR title |
| `body` | string | `create`/`update`/review | PR or review body |
| `base` | string | `create`/`update` | Base branch |
| `head` | string | `create` | Head branch (`user:branch`) |
| `state` | string | `list`/`update` | `"open"` or `"closed"` |
| `draft` | boolean | `create`/`update` | Draft status |
| `merge_method` | string | `merge` | `"merge"` \| `"squash"` \| `"rebase"` |
| `event` | string | `create_review` | `"APPROVE"` \| `"REQUEST_CHANGES"` \| `"COMMENT"` |
| `query` | string | `search` | Search query |
| `limit` | integer | `list`/`search` | Max results (default 30) |

**Read methods:** `list` \| `search` \| `get` \| `get_diff` \| `get_files` \| `get_comments` \| `get_reviews` \| `get_review_comments` \| `get_status`

**Write methods:** `create` \| `update` \| `merge` \| `update_branch` \| `create_review` \| `submit_review` \| `delete_review` \| `add_review_comment` \| `add_reply_to_comment`

**Important:** `get_diff` always returns a `@tool_*` buffer handle — diffs can be very large.
Query the buffer: `run_command("grep '+' @tool_abc123")`.

**Example — get a PR diff:**

```json
{
  "tool": "github_pr",
  "arguments": {
    "method": "get_diff",
    "owner": "acme",
    "repo": "myapp",
    "number": 42
  }
}
```

Then query: `run_command("grep '^+' @tool_xxxx")`

**Example — approve a PR:**

```json
{
  "tool": "github_pr",
  "arguments": {
    "method": "create_review",
    "owner": "acme",
    "repo": "myapp",
    "number": 42,
    "event": "APPROVE",
    "body": "Looks good — tests pass and the logic matches the spec."
  }
}
```

---

## `github_file`

**Purpose:** Read and write files in a GitHub repository via the GitHub API. Use for
pushing changes without a local clone, or reading files at a specific ref.

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `method` | string | yes | `get` \| `create_or_update` \| `delete` \| `push_files` |
| `owner` | string | yes | Repository owner |
| `repo` | string | yes | Repository name |
| `path` | string | most methods | File path within the repository |
| `ref` | string | `get` | Branch, tag, or commit SHA |
| `content` | string | `create_or_update` | Base64-encoded file content |
| `message` | string | write methods | Commit message |
| `branch` | string | write methods | Target branch |
| `sha` | string | `create_or_update`/`delete` | Blob SHA of existing file (required when updating) |
| `files` | array | `push_files` | `[{path, content}]` array for multi-file commits |

**Methods:**
- `get` — fetch file contents at an optional ref (returns `@buffer` handle for large files)
- `create_or_update` — create or update a single file; `sha` required when updating
- `delete` — delete a file; `sha` required
- `push_files` — push multiple files in a single commit

**Example — push multiple files:**

```json
{
  "tool": "github_file",
  "arguments": {
    "method": "push_files",
    "owner": "acme",
    "repo": "myapp",
    "branch": "main",
    "message": "Add config and readme",
    "files": [
      { "path": "config/default.toml", "content": "[server]\nport = 8080\n" },
      { "path": "docs/setup.md", "content": "# Setup\n\nRun `cargo run`.\n" }
    ]
  }
}
```

**Tips:**
- `sha` is returned by `get` — always fetch it before updating or deleting a file.
- `push_files` is the most efficient way to push multiple file changes in one commit.

---

## `github_repo`

**Purpose:** Repository, branch, commit, release, tag, and code search operations.

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `method` | string | yes | See methods below |
| `owner` | string | most methods | Repository owner |
| `repo` | string | most methods | Repository name |
| `query` | string | `search`/`search_code` | Search query |
| `name` | string | `create` | New repository name |
| `private` | boolean | `create` | Private repository flag |
| `branch` | string | `create_branch` | New branch name |
| `from_branch` | string | `create_branch` | Source branch (default: HEAD) |
| `sha` | string | `get_commit` | Commit SHA |
| `tag` | string | release/tag methods | Tag name |
| `limit` | integer | list methods | Max results (default 30) |

**Repo methods:** `search` \| `create` \| `fork`

**Branch methods:** `list_branches` \| `create_branch`

**Commit methods:** `list_commits` \| `get_commit` (returns `@buffer` handle)

**Release methods:** `list_releases` \| `get_latest_release` \| `get_release_by_tag`

**Tag methods:** `list_tags` \| `get_tag`

**Code:** `search_code` (returns `@buffer` handle)

**Example — search code:**

```json
{
  "tool": "github_repo",
  "arguments": {
    "method": "search_code",
    "query": "authenticate_user repo:acme/myapp language:rust"
  }
}
```

Then query: `run_command("grep 'path' @tool_xxxx")`

**Example — create a branch:**

```json
{
  "tool": "github_repo",
  "arguments": {
    "method": "create_branch",
    "owner": "acme",
    "repo": "myapp",
    "branch": "feat/new-auth",
    "from_branch": "main"
  }
}
```

**Tips:**
- `get_commit` and `search_code` return buffer handles — query them with `run_command`.
- For code search, GitHub's query syntax supports `repo:`, `language:`, `path:`, `extension:` filters.
```

**Step 2: Verify the file looks right**

Read it back: `grep -n "^## " docs/manual/src/tools/github.md`
Expected: 5 headings — `github_identity`, `github_issue`, `github_pr`, `github_file`, `github_repo`

**Step 3: Commit**

```bash
git add docs/manual/src/tools/github.md
git commit -m "docs: add github.md — full reference for 5 GitHub tools"
```

---

### Task 9: Wire github.md into SUMMARY.md and verify all links

**Files:**
- Modify: `docs/manual/src/SUMMARY.md`

**Step 1: Add GitHub to Tool Reference section**

The Tool Reference section currently ends with:
```
  - [Workflow & Config](tools/workflow-and-config.md)
```

Add after it:
```
  - [GitHub](tools/github.md)
```

**Step 2: Commit**

```bash
git add docs/manual/src/SUMMARY.md
git commit -m "docs: add GitHub tools to manual table of contents"
```

**Step 3: Final link sanity check**

Run: `grep -rn "github.md" docs/manual/src/`
Expected: at least 3 matches (SUMMARY.md, overview.md, FEATURES.md).

Run: `grep -n "28 tools\|28)" docs/manual/src/tools/overview.md README.md`
Expected: both files mention 28.

Run: `grep -n "remove_symbol" docs/manual/src/tools/symbol-navigation.md`
Expected: at least 3 matches.

Run: `grep -n "create_file\|edit_file" docs/manual/src/tools/file-operations.md`
Expected: both appear with headings.

If all checks pass, done. Commit any minor fixes:
```bash
git add -p
git commit -m "docs: final link and count verification fixes"
```
