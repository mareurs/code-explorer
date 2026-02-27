code-explorer MCP server: high-performance semantic code intelligence.
Provides file operations, symbol navigation (LSP), AST analysis (tree-sitter),
git blame, semantic search (embeddings), and project memory.

## How to Choose the Right Tool

### You know the name → use structure-aware tools
- `find_symbol(pattern)` — locate by name substring. Also accepts `name_path` (e.g. 'MyStruct/my_method').
- `list_symbols(path)` — symbol tree for file/dir/glob
- `find_references(name_path, path)` — find all usages
- `list_functions(path)` — quick signatures (tree-sitter, no LSP)

### You know the concept → semantic search first
- `semantic_search(query)` → then drill down with `list_symbols` / `find_symbol(include_body=true)`

### You know nothing → start with the map
1. `list_dir(path)` → 2. `list_symbols(file)` → 3. `semantic_search("what does this do")`

### Library code
`find_symbol` auto-discovers libraries. Use `scope: "lib:<name>"` on symbol/search tools.

### Other local repositories
- **Quick peek** (few files): use absolute paths — `list_dir`, `read_file`, `list_functions`, `search_pattern` all work without switching projects
- **Deep dive** (symbols, references, semantic search): `activate_project("/absolute/path")` first, explore, then switch back

## Output Modes

Default: **exploring** — compact, capped at 200 items.
Pass `detail_level: "full"` for focused mode with `offset`/`limit` pagination.
Only switch to focused AFTER identifying targets.

Overflow produces: `{ "overflow": { "shown": N, "total": M, "hint": "..." } }` — follow the hint.

## Rules

1. **PREFER symbol tools over read_file.** `list_symbols` + `find_symbol(include_body=true)` beats reading entire files.
2. **`read_file` rejects source code.** Use symbol tools for `.rs`, `.py`, `.ts`, etc. `read_file` is for README, configs, TOML, JSON, YAML.
3. **Semantic search for "how does X work?"** Then drill into results with symbol tools.
4. **Exploring mode first.** Only `detail_level: "full"` after you know what you need.
5. **Respect overflow hints.** Narrow your query, don't repeat it.
6. **Prefer symbol edits** (`replace_symbol`, `insert_before_symbol`) over `edit_lines` for code files.
