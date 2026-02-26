# Troubleshooting

This page covers common problems and their fixes, organized by symptom. If
your issue is not listed here, see [Getting Help](#getting-help) at the bottom.

---

## MCP Server Issues

### "Tool not found" in Claude Code

The server is not registered in your MCP configuration.

**Fix:** Verify the server entry exists:

```bash
claude mcp list
```

You should see `code-explorer` listed with 31 tools. If it is missing,
register it:

```bash
claude mcp add --global code-explorer -- code-explorer start --project .
```

See [Installation](getting-started/installation.md) for the full setup.

### Server starts but no tools appear

The project path is not set or points to a directory that does not exist.

**Fix:** Check the `--project` argument in your MCP configuration. The path
must be an absolute path to an existing directory, or `.` to use the current
working directory. Verify it resolves correctly:

```bash
code-explorer start --project /path/to/your/project
```

If you omit `--project`, code-explorer tries to auto-detect from the current
working directory. This works when Claude Code launches the server from within
a project, but can fail if the working directory is unexpected.

### "Connection refused" or server won't start

The binary is not installed, not on PATH, or (in HTTP mode) the port is
already in use.

**Fix:**

```bash
# Check the binary is installed
which code-explorer
code-explorer --version

# If not found, install it
cargo install code-explorer

# For HTTP mode, check port conflicts
lsof -i :8080
```

### Server crashes on startup

This usually means a required shared library is missing (common with the
`local-embed` feature, which needs ONNX Runtime).

**Fix:** Check the error output on stderr. If it mentions `libonnxruntime`,
install the ONNX Runtime or switch to the `remote-embed` feature:

```bash
# Reinstall with only remote embedding support
cargo install code-explorer --no-default-features --features remote-embed
```

---

## LSP and Symbol Tools

### Symbol tools return empty results

The language server for that file's language is not installed or not on PATH.

**Fix:** Install the required language server and verify it is accessible:

```bash
# Rust
which rust-analyzer

# Python
which pyright-langserver

# TypeScript/JavaScript
which typescript-language-server

# Go
which gopls

# Java
which jdtls

# Kotlin
which kotlin-language-server

# C/C++
which clangd

# C#
which OmniSharp

# Ruby
which solargraph
```

See the [Language Support](language-support.md) page for the full list.

**Also:** The language server may still be initializing. This is especially
common with `jdtls` (Java) and `kotlin-language-server`, which can take 10-30
seconds on first startup while they index the project. Wait a few seconds and
retry the tool call.

### "No tree-sitter grammar for 'X'"

The `list_functions` or `extract_docstrings` tool was called on a file whose
language does not have a bundled tree-sitter grammar.

**Fix:** Use LSP-based tools instead. `get_symbols_overview` provides similar
information (file structure, symbol names and kinds) and works for all 9 LSP
languages, not just the 6 with tree-sitter grammars.

If the language is not supported at all, only file operations and semantic
search (after indexing) are available.

### `find_referencing_symbols` returns nothing

Two common causes:

1. **Language server not fully indexed.** Some LSP servers need to scan the
   entire project before they can answer reference queries. This is especially
   true for Java (`jdtls`), which builds a workspace model at startup. Wait
   for initialization to complete and retry.

2. **Symbol has no references.** The symbol genuinely is not referenced
   anywhere in the project. This is correct behavior for unused code.

### Symbol tools work for some files but not others

The file's language may not be recognized, or the language server may have
crashed on that specific file.

**Fix:** Check the server logs (stderr) for error messages from the language
server. Restart the MCP server to reset all language server processes. If the
problem persists with a specific file, the file may contain syntax errors that
the language server cannot parse.

---

## Semantic Search

### "No results" from `semantic_search`

The embedding index has not been built for this project.

**Fix:** Build the index, then verify:

```json
{ "tool": "index_project", "arguments": {} }
{ "tool": "index_status", "arguments": {} }
```

`index_status` shows the number of indexed files and chunks. If both are zero,
the index build failed -- check server logs for errors.

### "Connection refused" when indexing

The default embedding backend is Ollama, which must be running locally.

**Fix:**

```bash
# Start Ollama
ollama serve

# Verify it's running
curl http://localhost:11434/v1/embeddings \
  -d '{"model": "mxbai-embed-large", "input": "test"}'
```

If you do not want to run Ollama, switch to a different backend. See
[Embedding Backends](configuration/embedding-backends.md).

### "Model not found" when indexing

The configured embedding model has not been pulled into Ollama.

**Fix:**

```bash
ollama pull mxbai-embed-large
```

Or if you configured a different model in `project.toml`:

```bash
ollama pull <your-model-name>
```

### Results seem wrong or irrelevant after changing the model

The index was built with a different embedding model. Vectors from different
models are incompatible -- mixing them produces meaningless similarity scores.

**Fix:** Rebuild the index from scratch:

```json
{ "tool": "index_project", "arguments": { "force": true } }
```

Then verify the models match:

```json
{ "tool": "index_status", "arguments": {} }
```

The response includes `configured_model` and `indexed_with_model`. They must
be the same.

### Indexing is very slow

Embedding large codebases with Ollama on CPU can take minutes or longer.

**Fix (pick one):**

- **Use a faster backend.** `openai:text-embedding-3-small` is significantly
  faster than local Ollama for large projects.
- **Reduce scope.** Add build artifacts, vendored code, and generated files to
  `ignored_paths` in `project.toml` so they are skipped during indexing.
- **Use GPU.** If Ollama has GPU access, embedding is much faster. Check
  `ollama ps` to verify the model is loaded on GPU.

### "No embedding backend compiled in"

The binary was built with `--no-default-features` and no embedding feature
was enabled.

**Fix:** Reinstall with an embedding backend:

```bash
# Remote (Ollama, OpenAI)
cargo install code-explorer --features remote-embed

# Local (CPU, no external service)
cargo install code-explorer --features local-embed
```

---

## Configuration

### Changes to `project.toml` not taking effect

Configuration is loaded when a project is activated. Editing the file after
activation does not automatically reload it.

**Fix:** Call `activate_project` again to reload the configuration:

```json
{ "tool": "activate_project", "arguments": { "path": "/path/to/project" } }
```

Or restart the MCP server.

### "No active project" errors

The server was started without a `--project` flag and could not auto-detect
a project from the working directory.

**Fix:** Set the project explicitly:

```json
{ "tool": "activate_project", "arguments": { "path": "/path/to/project" } }
```

Or restart the server with `--project`:

```bash
code-explorer start --project /path/to/project
```

---

## File Operations

### "Permission denied" or "Access denied" reading a file

The file is in the built-in deny list or matches a pattern in
`denied_read_patterns`.

The built-in deny list blocks access to sensitive locations regardless of
configuration:

```
~/.ssh
~/.aws
~/.gnupg
~/.config/gcloud
~/.config/gh
~/.docker/config.json
~/.netrc
~/.npmrc
~/.kube/config
```

On Linux, `/etc/shadow` and `/etc/gshadow` are also blocked. On macOS,
`/etc/master.passwd` is blocked.

**Fix:** This is intentional security behavior. If you genuinely need access
to a blocked path, check whether it is in the built-in list (cannot be
overridden) or in `denied_read_patterns` in `project.toml` (can be removed).
See [Project Configuration](configuration/project-toml.md) for details.

### "Access denied" writing a file outside the project

File write tools are restricted to the project root by default.

**Fix:** Add the target directory to `extra_write_roots` in `project.toml`:

```toml
[security]
extra_write_roots = ["/path/to/other/directory"]
```

### Shell commands return "shell execution is disabled"

Shell execution requires two settings to both be enabled.

**Fix:** Set both fields in `project.toml`:

```toml
[security]
shell_enabled = true
shell_command_mode = "warn"   # or "unrestricted"
```

`shell_enabled` is the master switch (default: `false`). `shell_command_mode`
controls whether a warning is appended to shell output (default: `"warn"`).
Even with `shell_enabled = true`, setting `shell_command_mode = "disabled"`
blocks all shell calls.

---

## Git Tools

### Git tools return errors

Two common causes:

1. **Not a git repository.** The project root does not contain a `.git`
   directory.

   **Fix:** Verify with `ls -la /path/to/project/.git`. If the project is not
   a git repo, git tools will not work.

2. **Git is disabled.** The `git_enabled` setting is `false` in the security
   configuration.

   **Fix:** Check `project.toml`:

   ```toml
   [security]
   git_enabled = true   # default is true
   ```

### `git_blame` returns errors on new files

Files that have never been committed cannot be blamed -- there is no git
history for them.

**Fix:** This is expected. Commit the file first, or use `git_diff` to see
its contents as uncommitted changes.

---

## Performance

### Slow responses on first tool call for a language

The first symbol tool call for a given language starts the language server
process. Startup time varies:

| Server | Typical startup |
|--------|-----------------|
| `rust-analyzer` | 2-5 seconds |
| `pyright-langserver` | 1-3 seconds |
| `typescript-language-server` | 1-2 seconds |
| `gopls` | 1-3 seconds |
| `clangd` | 1-2 seconds |
| `jdtls` | 10-30 seconds |
| `kotlin-language-server` | 5-15 seconds |

**Fix:** This is expected. Subsequent calls are fast because the server stays
running. If startup time is a problem for Java or Kotlin, consider using the
AST tools (`list_functions`, `extract_docstrings`) for initial exploration --
they use tree-sitter and have no startup delay.

### Large project causes tool timeouts

The default tool timeout is 60 seconds. Operations on very large projects
(indexing, initial LSP workspace scan) can exceed this.

**Fix:** Increase the timeout in `project.toml`:

```toml
[project]
tool_timeout_secs = 120
```

### High memory usage

Language servers can use significant memory, especially `jdtls` and
`rust-analyzer` on large projects. Running multiple language servers
simultaneously compounds this.

**Fix:** code-explorer starts language servers on demand, so only languages
you actively use consume memory. When the MCP server exits (or receives
SIGINT/SIGTERM), it gracefully shuts down all language servers via the LSP
shutdown protocol. As a safety net, the `LspClient` Drop implementation also
sends SIGTERM to child processes, ensuring cleanup even on abrupt exits.

If you have multiple Claude Code sessions, each spawns its own code-explorer
process with its own language servers. Close unused sessions to reclaim
memory.

---

## Getting Help

If none of the above resolves your issue:

1. **Check server logs.** code-explorer logs to stderr. In stdio mode, Claude
   Code captures this; look in Claude Code's MCP server logs. In HTTP mode,
   stderr goes to the terminal where you started the server.

2. **Enable debug logging.** Set `RUST_LOG=debug` for verbose output:

   ```bash
   RUST_LOG=debug code-explorer start --project /path/to/project
   ```

   This shows every tool call, LSP message, and embedding operation.

3. **Check the configuration.** Use the `get_current_config` tool to see the
   active configuration as the server sees it:

   ```json
   { "tool": "get_current_config", "arguments": {} }
   ```

4. **File an issue.** Open a GitHub issue with:
   - The error message (exact text)
   - The tool call that triggered it
   - Your `project.toml` (redact any secrets)
   - Output from `RUST_LOG=debug` if available
