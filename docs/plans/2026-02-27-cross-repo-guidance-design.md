# Design: Cross-repository exploration guidance

**Date:** 2026-02-27
**Status:** Approved
**Project:** code-explorer

## Problem

When a user is working in project A but needs to explore code in project B (another
local checkout), there is no guidance in `server_instructions.md` about how to do this.
The model either fails with relative-path errors or doesn't know it can use
`activate_project` to get full LSP/semantic support.

## Context

Path resolution already supports both strategies:

- **Absolute paths work without switching projects.** `validate_read_path` in
  `path_security.rs` accepts absolute paths as-is, regardless of active project.
  Tools that use this: `list_dir`, `read_file`, `list_functions`, `search_pattern`,
  `edit_lines`, `create_file`, `git_blame`.

- **`activate_project` switches the full context.** LSP, semantic search, memories,
  and relative path resolution all operate on the active project. Switching gives
  full power but loses the previous project's LSP/index state.

## Design

Add a new subsection to the "How to Choose the Right Tool" decision tree in
`src/prompts/server_instructions.md`, after "### Library code" and before
"## Output Modes":

```markdown
### Other local repositories
- **Quick peek** (few files): use absolute paths — `list_dir`, `read_file`, `list_functions`, `search_pattern` all work without switching projects
- **Deep dive** (symbols, references, semantic search): `activate_project("/absolute/path")` first, explore, then switch back
```

## What this teaches the model

1. Absolute paths work for basic file-level tools without disrupting the current project
2. LSP-dependent tools (`find_symbol`, `find_references`) and `semantic_search` need
   `activate_project`
3. The model should switch back when done

## What we're NOT doing

- No plugin changes (routing plugin guidance.txt stays as-is)
- No code changes in path resolution (absolute paths already work)
- No auto-detection of "should I switch?" — the model decides based on task complexity

## Files changed

| File | Change |
|------|--------|
| `src/prompts/server_instructions.md` | Add "Other local repositories" subsection |
