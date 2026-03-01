# Your First Project

This page walks you through opening a project for the first time and making sure code-explorer
is working correctly before you start a real task.

## Start a Claude Code Session

Open Claude Code in your project directory:

```bash
cd /path/to/your/project
claude
```

When code-explorer is registered (either globally or via `.mcp.json`), it starts automatically
alongside Claude Code. You do not need to do anything to launch the MCP server.

## What Happens on First Open

The first time code-explorer activates in a project it:

1. Creates a configuration file at `.code-explorer/project.toml` with sensible defaults.
2. Detects the languages present in the repository (based on file extensions and tree-sitter
   grammar support).
3. Starts LSP servers for the detected languages, ready to answer symbol queries.

You can check the generated configuration at any time with the `get_config` tool:

```json
{ "name": "get_config", "arguments": {} }
```


## Running Onboarding

For a project you have not explored before, run `onboarding` first. It performs a structured
discovery pass: reads directory structure, detects languages and frameworks, and writes a set of
memory entries so future sessions start with context already in place.

```json
{ "name": "onboarding", "arguments": {} }
```

Onboarding takes 10–30 seconds depending on project size. It produces a summary of what it found
and tells you how many memory entries it wrote. Subsequent sessions skip the heavy discovery work
because the memories are already there — call `onboarding` again (with default arguments) to
retrieve existing memories without re-running discovery:

```json
{ "name": "onboarding", "arguments": {} }
```


## Building the Embedding Index

Semantic search requires an embedding index. Build it once, then keep it up to date as the
codebase changes:

```json
{ "name": "index_project", "arguments": {} }
```

Indexing chunks every source file, embeds each chunk, and stores the vectors in
`.code-explorer/embeddings.db`. For a project with ~100k lines of code this typically takes
1–3 minutes. The index is incremental — only changed files are re-embedded on subsequent runs.

Verify the index was built successfully:

```json
{ "name": "index_status", "arguments": {} }
```

Sample output:

```
Embedding index status
  Files indexed : 312
  Chunks        : 4 847
  Model         : mxbai-embed-large
  Last updated  : 2026-02-26 14:32 UTC
```

## Trying the Basic Tools

Once onboarding and indexing are done, try these tools to get a feel for what is available.

### List Directory Structure

See the top-level layout of the project:

```json
{ "name": "list_dir", "arguments": { "path": "." } }
```

Drill into a subdirectory:

```json
{ "name": "list_dir", "arguments": { "path": "src", "recursive": true } }
```

### List Symbols

See the classes, functions, and types defined in a directory — one compact line per symbol,
no bodies:

```json
{ "name": "list_symbols", "arguments": { "path": "src/" } }
```

Sample output (Rust project):

```
src/main.rs
  fn main                    src/main.rs:12
  fn parse_args              src/main.rs:28

src/server.rs
  struct Server              src/server.rs:14
  impl Server
    fn new                   src/server.rs:31
    fn run                   src/server.rs:58
    fn shutdown              src/server.rs:102
```

### Find a Symbol by Name

Locate every definition of a symbol across the entire project:

```json
{ "name": "find_symbol", "arguments": { "pattern": "main" } }
```

To see the full function body alongside the location, add `include_body`:

```json
{
  "name": "find_symbol",
  "arguments": { "pattern": "main", "include_body": true }
}
```

### Semantic Search

Find code by concept rather than by name — useful when you do not know what the relevant
symbol is called:

```json
{
  "name": "semantic_search",
  "arguments": { "query": "error handling" }
}
```

Sample output:

```
src/server.rs  lines 88-112  score 0.91
  fn handle_request(...) -> Result<Response, AppError> {
      ...

src/errors.rs  lines 1-45  score 0.87
  pub enum AppError { ... }
```

Each result includes the file path, line range, similarity score, and a preview of the matched
chunk. Use the score as a rough relevance signal — results above 0.8 are usually directly
relevant; results below 0.5 are often tangential.

## Typical First-Session Workflow

A practical sequence for exploring an unfamiliar codebase:

1. `onboarding` — discover and remember the project structure.
2. `index_project` — build the semantic search index.
3. `list_dir` on the root and key subdirectories — build a mental map.
4. `list_symbols("src/")` — see what is defined at the top level.
5. `semantic_search("entry point")` or `find_symbol("main")` — find where execution starts.
6. From there, use `find_references` to trace callers and `list_symbols` to
   navigate deeper into subsystems.

After the first session, onboarding memories persist in `.code-explorer/memories/` and the
embedding index stays in `.code-explorer/embeddings.db`. Both are checked into `.gitignore`
by default so team members build their own local copies.

## Next Steps

- [Routing Plugin](routing-plugin.md) — install the plugin that ensures subagents also use code-explorer
- [Tool Selection](../concepts/tool-selection.md) — when to use symbol tools vs semantic search vs text search
- [Progressive Disclosure](../concepts/progressive-disclosure.md) — how tools manage output size automatically
