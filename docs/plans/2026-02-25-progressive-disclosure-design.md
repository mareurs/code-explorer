# Progressive Disclosure & Token Efficiency Design

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add guardrails so tools never produce unbounded output, enforce progressive disclosure as a project-wide pattern, and teach the LLM optimal tool selection via improved server instructions.

**Architecture:** Shared `OutputGuard` helper in `src/tools/output.rs` encodes two modes (exploring/focused). All unbounded tools use it. Server instructions teach the LLM when to use which mode and which tool category.

---

## 1. The Problem

Several tools can produce unbounded output:

| Tool | Worst case |
|---|---|
| `get_symbols_overview(dir/glob)` | Walks entire project, every symbol in every file |
| `find_symbol(pattern)` | Project-wide search, no result cap |
| `find_referencing_symbols` | Popular symbols → thousands of references |
| `list_dir(recursive=true)` | Entire directory tree |
| `git_diff` | Full diff of all uncommitted changes |
| `git_blame` (no line range) | Full file blame |
| `read_file` (no line range) | Full file content |

Already safe: `search_for_pattern` (50), `find_file` (100), `git_log` (20), `semantic_search` (10).

## 2. Two Modes: Exploring & Focused

| | Exploring (default) | Focused (`detail_level: "full"`) |
|---|---|---|
| Purpose | Get a map of what's there | Drill into specific content |
| Symbol output | name, kind, file, line | + body, children, full detail |
| File cap | 200 files max | No file cap (paginated) |
| Result cap | 200 results max | Paginated via `offset`/`limit` |
| Overflow | "Showing 47 of 312. Narrow with..." | Next page hint: `offset=200` |

## 3. OutputGuard Helper (`src/tools/output.rs`)

A lightweight struct, not a trait or middleware. Tools opt in by constructing it from their input JSON.

```rust
pub struct OutputGuard {
    pub mode: OutputMode,
    pub max_files: usize,     // default 200
    pub max_results: usize,   // default 200
    pub offset: usize,        // pagination start (focused mode)
    pub limit: usize,         // page size (focused mode), default 50
}

pub enum OutputMode {
    Exploring,
    Focused,
}

impl OutputGuard {
    /// Parse from tool input JSON. Reads `detail_level`, `offset`, `limit`.
    pub fn from_input(input: &Value) -> Self { ... }

    /// Whether to include bodies/full detail.
    pub fn should_include_body(&self) -> bool { matches!(self.mode, Focused) }

    /// Cap a vec of items. Returns (kept_items, overflow_info).
    pub fn cap_items<T>(&self, items: Vec<T>, total: usize) -> (Vec<T>, Option<OverflowInfo>) { ... }

    /// Build standardized overflow JSON.
    pub fn overflow_json(&self, shown: usize, total: usize, hint: &str) -> Value { ... }
}

pub struct OverflowInfo {
    pub shown: usize,
    pub total: usize,
    pub hint: String,
}
```

### Tool Integration

Each affected tool:
1. Constructs `OutputGuard::from_input(&input)`
2. Uses `guard.should_include_body()` to decide output detail
3. Uses `guard.cap_items()` to enforce limits
4. Appends `guard.overflow_json()` if overflow occurred

### Per-Tool Changes

**`get_symbols_overview`:**
- Exploring: return name/kind/file/line only, cap at 200 files
- Focused: return full symbol trees with bodies, paginate files via offset/limit
- For directory mode: apply same caps (currently walks with max_depth=1, but still unbounded)

**`find_symbol`:**
- Exploring: 200 result cap, no bodies
- Focused: paginated results, bodies included when `include_body=true`

**`find_referencing_symbols`:**
- Exploring: 200 reference cap
- Focused: paginated references

**`list_dir(recursive=true)`:**
- Exploring: 200 entry cap
- Focused: paginated entries

**`git_diff`:**
- Exploring: truncate diff at ~50KB, show overflow message with file count
- Focused: full diff (paginated by file if needed)

**`git_blame` (no line range given):**
- Exploring: first 200 lines + overflow message
- Focused: paginated via offset/limit

**`read_file` (no line range given):**
- Exploring: first 200 lines + overflow message
- Focused: full file (already has start_line/end_line)

## 4. CLAUDE.md Update

Add a "Design Principles" section before "Key Patterns":

```markdown
## Design Principles

**Progressive Disclosure** — Every tool defaults to the most compact useful
representation. Details are available on demand via `detail_level: "full"` +
pagination. Tools never dump unbounded output.

**Token Efficiency** — The LLM's context window is a scarce resource. Tools
minimize output by default: names + locations in exploring mode, full bodies
only in focused mode. Overflow produces actionable guidance ("showing N of M,
narrow with..."), not truncated garbage.

**Two Modes** — `Exploring` (default): compact, capped at 200 items. `Focused`:
full detail, paginated via offset/limit. This is enforced via `OutputGuard`
(`src/tools/output.rs`), a project-wide pattern, not per-tool logic.
```

## 5. Server Instructions Rewrite

The current `server_instructions.md` lists tools but doesn't teach **decision-making**. The rewrite adds:

### Tool Selection Decision Tree

Teach the LLM to pick the right tool based on what it knows:

- **Know the name** (file, function, class) → LSP/AST tools: `find_symbol`, `get_symbols_overview`, `list_functions`
- **Know the concept** (domain knowledge, "how does auth work") → Embedding search: `semantic_search` first, then drill down with symbol tools
- **Know nothing** → Start with `list_dir` + `get_symbols_overview` at the top level, then semantic search

### Output Modes Section

Teach the LLM the two-mode system and when to switch from exploring to focused.

### Progressive Disclosure Pattern

Formalize the workflow: explore broadly → identify target → focus narrowly.

### Full Server Instructions Content

```markdown
code-explorer MCP server: high-performance semantic code intelligence.
Provides file operations, symbol navigation (LSP), AST analysis (tree-sitter),
git history/blame, semantic search (embeddings), and project memory.

## How to Choose the Right Tool

### You know the name → use structure-aware tools
When you know the file path, function name, class name, or method name:
- `find_symbol(pattern)` — locate by name substring
- `get_symbols_overview(path)` — see all symbols in a file/directory/glob
- `list_functions(path)` — quick signatures via tree-sitter (no LSP needed)
- `find_referencing_symbols(name_path, file)` — find all usages

### You know the concept → use semantic search first
When you're exploring by domain ("how are errors handled", "authentication flow"):
- `semantic_search(query)` — find relevant code by natural language
- Then drill down: `get_symbols_overview(found_file)` → `find_symbol(name, include_body=true)`

### You know nothing → start with the map
When exploring an unfamiliar area:
1. `list_dir(path)` — see directory structure (shallow by default)
2. `get_symbols_overview(interesting_file)` — see what's in each file
3. `semantic_search("what does this module do")` — get the high-level picture
4. Then drill into specifics with `find_symbol` once you know what to look for

## Output Modes

Tools default to **exploring** mode — compact output (names, locations, counts)
capped at 200 items.

When you need full detail (function bodies, all children, complete diffs):
- Pass `detail_level: "full"` to get focused mode
- Use `offset` and `limit` to paginate through large results
- Only switch to focused mode AFTER you've identified specific targets

### Progressive disclosure pattern
1. **Explore broadly:** `get_symbols_overview("src/services/")` → compact map of all files
2. **Identify target:** spot the file/symbol you need from the overview
3. **Focus narrowly:** `find_symbol("handleAuth", path="src/services/auth.rs", include_body=true, detail_level="full")`

### Overflow messages
When results exceed the cap, you'll see:
```json
{ "overflow": { "shown": 47, "total": 312, "hint": "Narrow with a file path or glob pattern" } }
```
Follow the hint to refine your query.

## Tool Reference

### Symbol Navigation (LSP-backed)
- `find_symbol(pattern, [path], [include_body], [depth], [detail_level])` — find symbols by name
- `get_symbols_overview([path], [depth], [detail_level])` — symbol tree for file/dir/glob
- `find_referencing_symbols(name_path, file, [detail_level])` — find all references
- `list_functions(path)` — quick function signatures via tree-sitter

### Reading & Searching
- `read_file(path, [start_line], [end_line])` — read file content (use line ranges for large files)
- `semantic_search(query, [limit])` — find code by natural language description
- `search_for_pattern(pattern, [max_results])` — regex search across the project
- `find_file(pattern, [max_results])` — find files by glob pattern

### Editing
- `replace_symbol_body(name_path, file, new_body)` — replace a function/method body
- `insert_before_symbol(name_path, file, code)` / `insert_after_symbol(...)` — insert code
- `rename_symbol(name_path, file, new_name)` — rename across codebase (LSP)
- `replace_content(path, old, new)` — find-and-replace text
- `create_text_file(path, content)` — create or overwrite a file

### Git
- `git_blame(path, [start_line], [end_line])` — line-by-line blame
- `git_log([path], [limit])` — commit history (default: 20)
- `git_diff([commit], [path])` — uncommitted changes or diff against commit

### Project Memory
- `write_memory(topic, content)` / `read_memory(topic)` / `list_memories()` / `delete_memory(topic)`

### Project Management
- `onboarding` — first-time project discovery
- `check_onboarding_performed` — check if onboarding is done
- `execute_shell_command(command)` — run shell commands in project root
- `activate_project(path)` — switch active project
- `get_current_config` — show project config

## Rules

1. **PREFER symbol tools over reading entire files.** `get_symbols_overview` + `find_symbol(include_body=true)` is almost always more efficient than `read_file`.
2. **Use `read_file` for non-code files** (README, configs, TOML, JSON, YAML) or when you need a specific line range.
3. **Start with semantic search for "how does X work?" questions.** Then use symbol tools to drill into the results.
4. **Use exploring mode first.** Only switch to `detail_level: "full"` after you've identified what you need.
5. **Respect overflow hints.** When a tool says "narrow with a file path or glob", do it — don't just re-run the same broad query.
6. **Use `list_functions` for quick overviews** when you just need signatures, not full symbol trees.
```

## 6. Testing Strategy

- Unit tests for `OutputGuard`: cap behavior, overflow messages, mode parsing
- Update existing tool tests to verify capped output in exploring mode
- Add tests that would have previously produced unbounded output (e.g., `get_symbols_overview` on a directory with 300+ files) and verify they're capped
