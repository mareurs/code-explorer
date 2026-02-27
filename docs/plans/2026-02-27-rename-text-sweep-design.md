# Design: Post-Rename Text Sweep

**Date:** 2026-02-27
**Status:** Proposed
**Scope:** `rename_symbol` tool enhancement

## Problem

`rename_symbol` delegates to LSP `textDocument/rename`, which only renames
semantic symbol references the language server can resolve. Three categories
of occurrences are missed:

1. **Comments** mentioning the old name (doc comments, inline comments)
2. **Files outside LSP scope** (e.g., markdown docs, READMEs, changelogs)
3. **Partial string matches** within compound identifiers that the LSP
   correctly ignores but humans would want to review (e.g., `LibrarGetConfig`
   artifact after a typo-fix rename)

Users must currently do a manual grep sweep after every rename to catch these.

## Decision Summary

| Question | Decision |
|----------|----------|
| Where does the fix live? | Enhanced `rename_symbol` (single tool call) |
| Auto-replace textual matches? | No — report only, LLM decides |
| Sweep scope | Project root only (respecting .gitignore) |
| Match style | Exact old name, word-boundary, case-sensitive |
| Result cap | 20 matches max |
| Min name length for sweep | 4 characters (skip shorter names) |

## High-Level Flow

```
rename_symbol(name_path, path, new_name)
  │
  ├─ Phase 1: LSP rename (existing, unchanged)
  │   → textDocument/rename → apply WorkspaceEdit
  │   → collect HashSet<PathBuf> of files modified by LSP
  │
  └─ Phase 2: Text sweep (NEW)
      ├─ Guard: old_name.len() < 4 → skip, return sweep_skipped: true
      ├─ Regex: \b{regex::escape(old_name)}\b (case-sensitive)
      ├─ Walk: ignore::WalkBuilder (respects .gitignore, skips hidden)
      ├─ Exclude: files already modified by LSP, binary files
      ├─ Classify: documentation > config > source
      ├─ Dedup: max 2 preview lines per file, with total count
      └─ Cap: 20 matches total
```

## Response Schema

### Normal rename with textual matches found

```json
{
  "status": "ok",
  "old_name": "FooHandler",
  "new_name": "BarHandler",
  "files_changed": 5,
  "total_edits": 12,
  "textual_matches": [
    {
      "file": "README.md",
      "lines": [15, 42],
      "previews": [
        "The `FooHandler` struct manages...",
        "See `FooHandler::new()` for initialization"
      ],
      "occurrence_count": 3,
      "kind": "documentation"
    },
    {
      "file": "src/lib.rs",
      "lines": [87, 103],
      "previews": [
        "// FooHandler handles the connection pooling",
        "// TODO: refactor FooHandler to use async"
      ],
      "occurrence_count": 5,
      "kind": "source"
    }
  ],
  "textual_match_count": 8,
  "textual_matches_shown": 2,
  "sweep_skipped": false
}
```

### Sweep skipped (short name)

```json
{
  "status": "ok",
  "old_name": "Ok",
  "new_name": "Success",
  "files_changed": 3,
  "total_edits": 8,
  "textual_matches": [],
  "textual_match_count": 0,
  "sweep_skipped": true,
  "sweep_skip_reason": "name too short (2 chars, minimum 4)"
}
```

## Implementation

### New function: `text_sweep`

Location: `src/tools/symbol.rs`

```rust
struct TextualMatch {
    file: String,           // relative path from project root
    lines: Vec<u32>,        // all matching line numbers
    previews: Vec<String>,  // first max_lines_per_file matching lines
    occurrence_count: usize,
    kind: &'static str,     // "documentation" | "config" | "source"
}

fn text_sweep(
    project_root: &Path,
    old_name: &str,
    lsp_modified_files: &HashSet<PathBuf>,
    max_matches: usize,        // 20
    max_lines_per_file: usize, // 2
) -> anyhow::Result<Vec<TextualMatch>>
```

### Logic

1. Build regex `\b{regex::escape(old_name)}\b` via `RegexBuilder`
2. Walk project root with `ignore::WalkBuilder` (same as `SearchPattern`)
3. For each file:
   - Skip if path is in `lsp_modified_files`
   - Attempt `read_to_string` — skip on failure (binary/non-UTF8)
   - Classify by extension:
     - `.md`, `.txt`, `.rst`, `.adoc` → `"documentation"`
     - `.toml`, `.yaml`, `.yml`, `.json` → `"config"`
     - Everything else → `"source"`
   - Scan lines, collect all matching line numbers
   - Keep first `max_lines_per_file` lines as previews
   - Build `TextualMatch` with full count
4. Sort results: documentation first, config second, source third
5. Truncate to `max_matches`

### Integration into `RenameSymbol::call`

After the existing WorkspaceEdit application (~line 1165):

```rust
// Collect files modified by LSP
let mut lsp_files: HashSet<PathBuf> = HashSet::new();
// ... populate from edit.changes and edit.document_changes URIs

// Phase 2: text sweep
let old_name_str = name_path.rsplit('/').next().unwrap_or(name_path);
let (textual, sweep_skipped, skip_reason) = if old_name_str.len() < 4 {
    (vec![], true, Some(format!(
        "name too short ({} chars, minimum 4)", old_name_str.len()
    )))
} else {
    match text_sweep(&rename_root, old_name_str, &lsp_files, 20, 2) {
        Ok(matches) => (matches, false, None),
        Err(e) => {
            tracing::warn!("text sweep failed: {e}");
            (vec![], false, Some(format!("sweep error: {e}")))
        }
    }
};

let textual_count: usize = textual.iter().map(|m| m.occurrence_count).sum();
let textual_shown = textual.len();
```

### Collecting `lsp_files`

The LSP modified files set is built from the same `edit.changes` and
`edit.document_changes` iteration that already exists — just insert each
`path` into the `HashSet` before writing:

```rust
lsp_files.insert(path.clone());
```

No new file I/O or LSP calls required.

### Error handling

The sweep must **never fail the rename**. The LSP rename is the primary value;
the sweep is best-effort supplementary information. Any sweep error is logged
via `tracing::warn!` and results in an empty `textual_matches` array.

### Tool description update

From:
> "Rename a symbol across the entire codebase using LSP."

To:
> "Rename a symbol across the entire codebase using LSP. After renaming,
> sweeps for remaining textual occurrences (comments, docs, strings) that
> LSP missed and reports them."

## Files Changed

| File | Change |
|------|--------|
| `src/tools/symbol.rs` | Add `TextualMatch` struct, `text_sweep()` fn, extend `RenameSymbol::call` |

## Testing

- **Unit test for `text_sweep`**: create a temp directory with source files
  containing comments, a README, and a config file. Run sweep, verify
  correct matches are returned with proper classification and capping.
- **Unit test for short name guard**: verify sweep is skipped for names < 4 chars.
- **Unit test for dedup**: file with 10 occurrences should produce 1 `TextualMatch`
  with `occurrence_count: 10` and 2 previews.
- **Integration**: rename a symbol in a fixture project, verify response includes
  textual matches from comments and docs.

## Non-Goals

- Auto-replacing textual matches (report only)
- Case-variant matching (e.g., snake_case ↔ CamelCase)
- Searching outside project root (sibling directories, monorepo peers)
- AST-aware comment extraction (plain regex is sufficient)
- Semantic ranking of matches
