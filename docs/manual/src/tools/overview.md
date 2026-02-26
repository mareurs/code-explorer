# Tools Overview

code-explorer exposes 33 tools organized into nine categories. This page is a
quick map. Each category has a dedicated reference page linked from the headings
below.

---

## [Symbol Navigation](symbol-navigation.md)

LSP-backed tools for locating and editing code by name rather than by line
number. These tools require an LSP server to be running for the target language.

The navigation tools (`find_symbol`, `get_symbols_overview`, `find_referencing_symbols`, `list_functions`) accept an optional **`scope`** parameter to search library code as well as project code — see [Library Navigation](#library-navigation) below.

| Tool | Description |
|------|-------------|
| `find_symbol` | Find symbols by name pattern across the project or within a file |
| `get_symbols_overview` | Outline of a file, directory, or glob — classes, functions, structs in tree form |
| `find_referencing_symbols` | All callers and usages of a given symbol |
| `replace_symbol_body` | Replace the entire body of a named symbol with new source |
| `insert_before_symbol` | Insert code immediately before a named symbol |
| `insert_after_symbol` | Insert code immediately after a named symbol |
| `rename_symbol` | Rename a symbol across the entire codebase using LSP |

---

## [File Operations](file-operations.md)

Read, list, and search files. These tools work on any file regardless of
language support.

| Tool | Description |
|------|-------------|
| `read_file` | Read lines from a file, with optional range and pagination |
| `list_dir` | List files and directories, optionally recursive |
| `search_for_pattern` | Search file contents with a regex pattern |
| `find_file` | Find files by glob pattern, respecting `.gitignore` |

---

## [Editing](editing.md)

Create and modify files directly, without going through LSP.

| Tool | Description |
|------|-------------|
| `create_text_file` | Create or overwrite a file with given content |
| `replace_content` | Find-and-replace text in a file (literal or regex) |
| `edit_lines` | Replace a specific line range in a file |

---

## [Semantic Search](semantic-search.md)

Find code by meaning rather than by name or pattern. Requires an embedding
index built with `index_project`. Use the optional `scope` parameter to search
within a specific library (see [Library Navigation](#library-navigation)).

| Tool | Description |
|------|-------------|
| `semantic_search` | Search code by natural language description or code snippet |
| `index_project` | Build or incrementally update the embedding index |
| `index_status` | Show index stats: file count, chunk count, model, last update |
| `check_drift` | Query semantic drift scores from the last index build — which files changed *meaningfully* (opt out with `drift_detection_enabled = false`) |

---

## [Git](git.md)

Inspect version history and uncommitted changes.

| Tool | Description |
|------|-------------|
| `git_blame` | Who last changed each line and in which commit |
| `git_log` | Commit history for a file or the whole project |
| `git_diff` | Uncommitted changes, or diff against a specific commit |

---

## Library Navigation {#library-navigation}

Navigate third-party dependency source code (read-only). Libraries are
auto-registered when LSP `goto_definition` returns a path outside the project
root; you can also register them manually.

| Tool | Description |
|------|-------------|
| `list_libraries` | Show all registered libraries, their root paths, and index status |
| `index_library` | Build an embedding index for a registered library |

**Scope parameter** — once a library is registered, pass `scope` to any
navigation or search tool to target it:

| Value | What it searches |
|---|---|
| `"project"` (default) | Only project source code |
| `"lib:<name>"` | A specific registered library |
| `"libraries"` | All registered libraries |
| `"all"` | Project + all libraries |

All results include a `"source"` field (`"project"` or `"lib:<name>"`) so you
can tell where each result came from.

---

## [AST Analysis](ast.md)

Tree-sitter based analysis that works offline without a language server.
Supports Rust, Python, TypeScript, and Go.

| Tool | Description |
|------|-------------|
| `list_functions` | All function and method signatures in a file |
| `extract_docstrings` | All docstrings and top-level comments with their associated symbol names |

---

## [Memory](memory.md)

Persistent key-value store backed by markdown files in
`.code-explorer/memories/`. Survives across sessions.

| Tool | Description |
|------|-------------|
| `write_memory` | Write a memory entry under a topic path |
| `read_memory` | Read a stored memory entry by topic |
| `list_memories` | List all stored memory topics |
| `delete_memory` | Delete a memory entry by topic |

---

## [Workflow & Config](workflow-and-config.md)

Project setup, shell execution, and server configuration.

| Tool | Description |
|------|-------------|
| `onboarding` | Perform initial project discovery: detect languages, list top-level structure |
| `check_onboarding_performed` | Check whether project onboarding has been done |
| `execute_shell_command` | Run a shell command in the project root and return stdout/stderr |
| `activate_project` | Switch the active project to a given path |
| `get_current_config` | Display the active project config and server settings |

---

## Which Tool Do I Use?

Use this table when you know what you want but are not sure which tool to reach
for.

| You want to... | Use this |
|----------------|----------|
| See what functions/classes a file contains | `get_symbols_overview` |
| Find where a function is defined | `find_symbol` |
| Find all callers of a function | `find_referencing_symbols` |
| Rewrite a function body | `replace_symbol_body` |
| Add a new function next to an existing one | `insert_after_symbol` |
| Rename a function everywhere | `rename_symbol` |
| Find code that does something (concept, not name) | `semantic_search` |
| Find code by concept inside a library | `semantic_search` with `scope: "lib:<name>"` (after `index_library`) |
| See what third-party libraries are registered | `list_libraries` |
| Check which files changed meaningfully after re-indexing | `check_drift` |
| Search for a string or regex across files | `search_for_pattern` |
| Find files matching a name pattern | `find_file` |
| Read a specific part of a file | `read_file` (with `start_line`/`end_line`) |
| See who changed a line and why | `git_blame` |
| See what changed recently | `git_diff` |
| Check commit history for a file | `git_log` |
| Get all function signatures quickly (no LSP) | `list_functions` |
| Extract all doc comments from a file | `extract_docstrings` |
| Remember a decision for the next session | `write_memory` |
| Run a build or test command | `execute_shell_command` |
| Orient yourself in a new project | `onboarding` |

### Choosing Between Symbol Navigation and Semantic Search

Use **symbol navigation** (`find_symbol`, `get_symbols_overview`) when you know
the name of what you are looking for. LSP tools are precise and fast.

Use **semantic search** when you know the concept but not the name: "retry
logic", "token validation", "connection pool initialization". Semantic search
finds code that _means_ what you describe, regardless of what it is called.

### Choosing Between `find_symbol` and `get_symbols_overview`

`get_symbols_overview` answers "what is in this file or directory?" — it gives
you the map. `find_symbol` answers "where is this specific thing?" — it finds
a target by name, optionally across the whole project. Start with
`get_symbols_overview` to orient, then use `find_symbol` to drill in.

### Choosing Between LSP Editing and Direct Editing

`replace_symbol_body`, `insert_before_symbol`, `insert_after_symbol`, and
`rename_symbol` operate on named symbols. They do not care about line numbers
and are robust to changes above the target. Use them when you know the symbol
name.

`replace_content` and `edit_lines` operate on text. Use them for changes that
are not naturally symbol-scoped: adding an import, changing a constant value,
patching a configuration block.
