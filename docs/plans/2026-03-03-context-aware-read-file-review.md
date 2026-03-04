# Context-Aware `read_file` Implementation Plan — Review Notes

**Plan reviewed:** `docs/plans/2026-03-03-context-aware-read-file-impl.md`
**Date:** 2026-03-03

## Issues to Fix Before Execution

### 1. Remove `serde_yml` dependency (YAGNI)

Task 5 adds `serde_yml` but neither `summarize_yaml()` nor `extract_yaml_key()` use it — both use line-scanning. Remove entirely.

### 2. `find_json_key_line()` is fragile

Searches for `"key"` as literal string — matches first occurrence including inside nested objects or string values. For summary, either drop `line` from schema output (json_path makes it unnecessary) or use a depth-0-only scanner.

### 3. JSON `line_range` in `extract_json_path()` is meaningless

`start_line + pretty.lines().count()` adds pretty-printed size to a rough original-file offset. Return extracted value only — no fake line range.

### 4. `SectionResult` over-generalized for JSON

`breadcrumb`/`siblings` are meaningful for Markdown and TOML but empty for JSON. Keep the struct but have `extract_json_path` return its own response shape directly in `call()` rather than going through `SectionResult`.

### 5. TOML table header detection: bogus `[#` guard

`!trimmed.starts_with("[#")` — `[#` isn't a TOML construct. Remove it. The rest of the detection (starts with `[`, ends with `]`) is fine and handles `[[array-of-tables]]` correctly.

### 6. `format_read_file_summary` dispatch coordination

Adding `"json"`, `"yaml"`, `"toml"` branches works, but add a comment on the residual `"config"` branch: `// .xml, .ini, .env, .lock, .cfg`.

## Minor Notes (not blocking)

- **Heading fuzzy match collisions**: Substring match on short queries (e.g. `"## A"`) could collide. Document that exact match is preferred; LLM should use full heading text from summary.
- **H1 end_line covers everything**: Correct behavior (parent heading spans children) but document this explicitly.
- **30-heading cap**: Reasonable for summaries.

## Verdict

Plan structure is solid (TDD, phased, incremental). Fix the 6 issues above — most are small. Issues #1 (unused dep), #2-3 (fake JSON line numbers), #5 (bogus TOML guard) are the most impactful.
