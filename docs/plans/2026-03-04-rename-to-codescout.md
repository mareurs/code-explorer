# Rename code-explorer → codescout Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rename the crate, binary, and project identity from `code-explorer` to `codescout` across all source files, docs, and the companion plugin — while keeping `.code-explorer/` config directories unchanged.

**Architecture:** Pure rename/refactor — no behaviour changes. Three categories of change: (1) Cargo identity (`Cargo.toml`, `src/main.rs` lib path), (2) internal Rust identifiers (`CodeExplorerServer`), (3) string literals and prose in source, prompts, and docs. Config directories stay as `.code-explorer/` to avoid breaking existing user data.

**Tech Stack:** Rust (cargo), sed/grep for verification, companion plugin shell scripts.

---

### Task 1: Update Cargo.toml

**Files:**
- Modify: `Cargo.toml`

**Step 1: Update the three name fields**

In `Cargo.toml`, change:
```toml
[package]
name = "code-explorer"
# ...
repository = "https://github.com/mareurs/code-explorer"

[lib]
name = "code_explorer"
path = "src/lib.rs"

[[bin]]
name = "code-explorer"
path = "src/main.rs"
```
to:
```toml
[package]
name = "codescout"
# ...
repository = "https://github.com/mareurs/codescout"

[lib]
name = "codescout"
path = "src/lib.rs"

[[bin]]
name = "codescout"
path = "src/main.rs"
```

**Step 2: Verify it compiles**

Run: `cargo build 2>&1 | head -30`

Expected: compile errors about `code_explorer::` not found in `src/main.rs` — that's correct, we fix it next.

---

### Task 2: Update src/main.rs — lib path and string literals

**Files:**
- Modify: `src/main.rs`

**Step 1: Replace lib crate references**

In `src/main.rs`, replace all `code_explorer::` with `codescout::` (3 occurrences):
- `code_explorer::server::run(...)` → `codescout::server::run(...)`
- `code_explorer::embed::index::build_index(...)` → `codescout::embed::index::build_index(...)`
- `code_explorer::dashboard::serve(...)` → `codescout::dashboard::serve(...)`

**Step 2: Update the clap app name and log string**

- `name = "code-explorer",` (in clap derive) → `name = "codescout",`
- `"Starting code-explorer MCP server (transport={})"` → `"Starting codescout MCP server (transport={})"`

**Step 3: Verify it compiles**

Run: `cargo build 2>&1 | head -30`
Expected: clean build (no errors related to the rename).

**Step 4: Commit**

```bash
git add Cargo.toml src/main.rs
git commit -m "feat: rename crate and binary to codescout"
```

---

### Task 3: Rename CodeExplorerServer struct in src/server.rs

**Files:**
- Modify: `src/server.rs`

**Step 1: Rename the struct and its impls**

In `src/server.rs`, replace:
- `pub struct CodeExplorerServer {` → `pub struct CodeScoutServer {`
- `impl CodeExplorerServer {` → `impl CodeScoutServer {`
- `impl ServerHandler for CodeExplorerServer {` → `impl ServerHandler for CodeScoutServer {`
- All other `CodeExplorerServer` references (constructor calls, type annotations) → `CodeScoutServer`

Also update the log string:
- `"code-explorer MCP server ready (stdio)"` → `"codescout MCP server ready (stdio)"`

**Step 2: Find and update all references in other files**

Run: `grep -rn "CodeExplorerServer" src/ --include="*.rs"`

Update any remaining references (likely only `src/main.rs` if it creates the server directly — check).

**Step 3: Verify**

Run: `cargo build 2>&1 | head -20`
Expected: clean.

**Step 4: Update the lib.rs doc comment**

In `src/lib.rs` line 1, change:
- `//! code-explorer: high-performance coding agent MCP server.`
→ `//! codescout: high-performance coding agent MCP server.`

**Step 5: Commit**

```bash
git add src/server.rs src/lib.rs src/main.rs
git commit -m "refactor: rename CodeExplorerServer to CodeScoutServer"
```

---

### Task 4: Update string literals in source files

These are error messages and comments that say "code-explorer" but don't refer to the config directory. The `.code-explorer/` directory references are intentionally left alone.

**Files:**
- Modify: `src/util/path_security.rs`
- Modify: `src/library/registry.rs`

**Step 1: Update path_security.rs error messages**

Find lines with "code-explorer tools" (3 occurrences):
- `"Shell commands are disabled. Set security.shell_enabled = true in .code-explorer/project.toml to enable."` — keep `.code-explorer/` as-is ✓
- `"File write tools are disabled. Set security.file_write_enabled = true in .code-explorer/project.toml to enable."` — keep ✓
- `"Indexing tools are disabled. Set security.indexing_enabled = true in .code-explorer/project.toml to enable."` — keep ✓
- Comment on line ~403: `/// Source file extensions that should be accessed via code-explorer tools,` → `/// Source file extensions that should be accessed via codescout tools,`
- Comment on line ~414: `/// present in the command string. Use code-explorer tools instead:` → `/// Use codescout tools instead:`

**Step 2: Update library/registry.rs doc comment**

- `/// A registered external library that code-explorer can search into.` → `/// A registered external library that codescout can search into.`

**Step 3: Verify**

Run: `cargo build 2>&1 | head -10`
Expected: clean.

**Step 4: Commit**

```bash
git add src/util/path_security.rs src/library/registry.rs
git commit -m "chore: update code-explorer → codescout in source comments and strings"
```

---

### Task 5: Update prompt files

These are injected into every MCP session — they are user-visible and need the new name.

**Files:**
- Modify: `src/prompts/server_instructions.md`
- Modify: `src/prompts/onboarding_prompt.md`

**Step 1: server_instructions.md**

Line 1: `code-explorer MCP server:` → `codescout MCP server:`
Line 5-6: `code-explorer too` / `code-explorer tools` → `codescout too` / `codescout tools`

All `.code-explorer/system-prompt.md` references — leave as-is (config directory).

**Step 2: onboarding_prompt.md**

Replace all occurrences of `code-explorer` that refer to the tool name (not the `.code-explorer/` directory):
- `"use code-explorer tools"` → `"use codescout tools"`
- `"code-explorer session"` → `"codescout session"`
- `"Your code-explorer setup is complete."` → `"Your codescout setup is complete."`
- `"code-explorer tools to fill gaps"` → `"codescout tools to fill gaps"`

Keep all `.code-explorer/` directory references exactly as-is.

**Step 3: Verify no `.code-explorer` directory references were accidentally changed**

Run: `grep -n "\.code-explorer" src/prompts/*.md`
Expected: all original `.code-explorer/` paths still present.

**Step 4: Verify tests pass**

Run: `cargo test 2>&1 | tail -5`
Expected: all tests pass (no test references these strings directly).

**Step 5: Commit**

```bash
git add src/prompts/
git commit -m "chore: update prompt surfaces to codescout name"
```

---

### Task 6: Update README.md

This is the public face of the project. Full rename.

**Files:**
- Modify: `README.md`

**Step 1: Replace all tool-name occurrences**

Replace every `code-explorer` that refers to the tool/binary/project name with `codescout`:
- Heading: `# code-explorer` → `# codescout`
- All `code-explorer start`, `code-explorer dashboard`, `code-explorer index` → `codescout start`, etc.
- `cargo install code-explorer` → `cargo install codescout`
- `claude mcp add --global code-explorer -- code-explorer start` → `claude mcp add --global codescout -- codescout start`
- `git clone https://github.com/mareurs/code-explorer.git` → `https://github.com/mareurs/codescout.git`
- `cd code-explorer` → `cd codescout`
- All prose references: `code-explorer is an MCP server` → `codescout is an MCP server`

**Step 2: Keep companion plugin name unchanged**

`code-explorer-routing` stays as `code-explorer-routing` throughout (it's a separate project with its own identity).

**Step 3: Verify**

Run: `grep -n "code-explorer" README.md | grep -v "code-explorer-routing" | grep -v "\.code-explorer"`
Expected: zero results (all tool-name references updated; companion plugin and config dir refs untouched).

**Step 4: Commit**

```bash
git add README.md
git commit -m "docs: rename code-explorer → codescout in README"
```

---

### Task 7: Update CLAUDE.md and docs

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/ROADMAP.md`
- Modify: `docs/ARCHITECTURE.md` (if it exists and contains references)

**Step 1: CLAUDE.md**

- Heading: `# code-explorer` → `# codescout`
- All `code-explorer MCP tools` / `code-explorer's own MCP tools` → `codescout MCP tools`
- Binary example: `code-explorer start` → `codescout start`
- Keep `code-explorer-routing` plugin name unchanged
- Keep `mcp__code-explorer__*` tool prefix — this is the MCP server identifier set by the user in their config, not the binary name. Annotate with a comment: users configure this as the server name in their MCP config.

**Step 2: docs/ROADMAP.md**

- `code-explorer dashboard` → `codescout dashboard`
- `code-explorer-routing` stays unchanged
- Prose: `Make code-explorer usable by` → `Make codescout usable by`

**Step 3: Verify**

Run: `grep -rn "code-explorer" CLAUDE.md docs/ROADMAP.md | grep -v "code-explorer-routing" | grep -v "mcp__code-explorer" | grep -v "\.code-explorer"`
Expected: zero results.

**Step 4: Commit**

```bash
git add CLAUDE.md docs/ROADMAP.md
git commit -m "docs: rename code-explorer → codescout in CLAUDE.md and ROADMAP"
```

---

### Task 8: Update companion plugin detect-tools.sh

The plugin detects the MCP server by scanning for `code-explorer` in command strings. After the rename, users will have `codescout` in their MCP config. Update to match either name.

**Files:**
- Modify: `/home/marius/work/claude/claude-plugins/code-explorer-routing/hooks/detect-tools.sh`

**Step 1: Find the detection patterns**

The file has two detection blocks (around lines 35-36 and 51-52):
```sh
(.value.command // "" | test("code-explorer")) or
((.value.args // []) | map(test("code-explorer")) | any)
```

**Step 2: Update to match both names**

Change each pattern to:
```sh
(.value.command // "" | test("code-explorer|codescout")) or
((.value.args // []) | map(strings | test("code-explorer|codescout")) | any)
```

This makes the plugin work for users on the old name AND users who've migrated to `codescout`.

**Step 3: Verify manually**

Run a quick grep to confirm the patterns updated:
```bash
grep -n "code-explorer\|codescout" /home/marius/work/claude/claude-plugins/code-explorer-routing/hooks/detect-tools.sh
```

**Step 4: Commit**

```bash
cd /home/marius/work/claude/claude-plugins/code-explorer-routing
git add hooks/detect-tools.sh
git commit -m "feat: detect codescout binary alongside code-explorer in MCP config"
cd -
```

---

### Task 9: Full verification

**Step 1: Run full test suite**

Run: `cargo test`
Expected: all 992 tests pass.

**Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: clean.

**Step 3: Run fmt**

Run: `cargo fmt`
Expected: no changes (or apply any minor formatting).

**Step 4: Check for stray references**

Run: `grep -rn "code.explorer\|CodeExplorer\|code_explorer" src/ --include="*.rs" | grep -v "\.code-explorer" | grep -v "code-explorer-routing" | grep -v "code_explorer_secret_test"`
Expected: zero results (the test temp dir name `code_explorer_secret_test` can stay — it's an internal test artifact, not a user-facing name).

**Step 5: Rename GitHub repo**

On GitHub: Settings → Repository name → `codescout` → Rename.
GitHub auto-redirects all old clone URLs.

**Step 6: Final commit if any fmt changes**

```bash
git add -A
git commit -m "chore: cargo fmt after rename"
```

---

### Task 10: Notify existing users

Draft the one-liner to send:

> **codescout rename:** Rebuilt and update your MCP config:
> ```bash
> cargo install --path /path/to/codescout
> # In your Claude Code MCP config, change "command": "code-explorer" → "command": "codescout"
> # Your .code-explorer/ directories and memories are untouched.
> ```
