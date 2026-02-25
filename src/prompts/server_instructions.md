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
- `git_blame(path, [start_line], [end_line], [detail_level])` — line-by-line blame
- `git_log([path], [limit])` — commit history (default: 20)
- `git_diff([commit], [path], [detail_level])` — uncommitted changes or diff against commit

### Project Memory
- `write_memory(topic, content)` / `read_memory(topic)` / `list_memories()` / `delete_memory(topic)`

### Project Management
- `onboarding` — first-time project discovery and memory creation
- `check_onboarding_performed` — check if onboarding is done
- `execute_shell_command(command)` — run shell commands in project root
- `activate_project(path)` — switch active project
- `get_current_config` — show project config

## Rules

1. **PREFER symbol tools over reading entire files.** `get_symbols_overview` + `find_symbol(include_body=true)` is almost always more efficient than `read_file`.
2. **Use `read_file` for non-code files** (README, configs, TOML, JSON, YAML) or when you need a specific line range.
3. **Start with semantic search for "how does X work?" questions.** Then drill into results with symbol tools.
4. **Use exploring mode first.** Only switch to `detail_level: "full"` after you've identified what you need.
5. **Respect overflow hints.** When a tool says "narrow with a file path or glob", do it — don't re-run the same broad query.
6. **Use `list_functions` for quick overviews** when you just need signatures, not full symbol trees.
