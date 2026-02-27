# Design: User-Configurable `system_prompt` in project.toml

**Date:** 2026-02-27
**Status:** Approved

## Problem

Users cannot customize the MCP server instructions that guide the LLM's use of
code-explorer tools. The server instructions are fully static (baked-in
`server_instructions.md` + auto-detected project status). Users who want to add
project-specific guidance (e.g. "this is a Django project, prefer class-based
views") have no mechanism to do so without forking the server.

## Solution

Add an optional `system_prompt` field to the `[project]` section of
`.code-explorer/project.toml`. When present, its content is appended to the
server instructions after the project status block, under a `## Custom
Instructions` header.

## Configuration

```toml
[project]
name = "my-app"
system_prompt = "This is a Django project. Prefer class-based views."
```

Multi-line is also supported via TOML's triple-quoted strings:

```toml
[project]
name = "my-app"
system_prompt = '''
This is a Django project.
Always prefer class-based views.
Use pytest for testing.
'''
```

When omitted (default), no custom section is appended — backward compatible.

## Data Flow

```
project.toml        →  ProjectConfig.project.system_prompt
                              ↓
Agent::project_status()  →  ProjectStatus.system_prompt
                              ↓
build_server_instructions()  →  appended after project status
                              ↓
ServerHandler::get_info()  →  returned to MCP client
```

## Output Format

The assembled instructions look like:

```markdown
<server_instructions.md content>

## Project Status

- **Project:** my-app at `/path/to/project`
- **Languages:** python
- **Available memories:** ...
- **Semantic index:** Built

## Custom Instructions

This is a Django project. Prefer class-based views.
```

## Changes

| File | Change |
|------|--------|
| `src/config/project.rs` | Add `system_prompt: Option<String>` to `ProjectSection` |
| `src/prompts/mod.rs` | Add `system_prompt: Option<String>` to `ProjectStatus`, append in `build_server_instructions()` |
| `src/agent.rs` | Thread `system_prompt` from config into `ProjectStatus` |

## Tests

1. **Config deserializes correctly** — `system_prompt` parses from TOML, defaults to `None` when absent
2. **Prompt assembly with system_prompt** — `build_server_instructions()` appends `## Custom Instructions` section
3. **Prompt assembly without system_prompt** — existing behavior unchanged (no extra section)
4. **Existing tests pass** — no regressions

## Non-Goals

- CLI `--system-prompt` argument (may add later if needed)
- Separate `.code-explorer/system_prompt.md` file
- Template/variable substitution in the prompt text
- Re-building instructions on `activate_project` (instructions are built once at server startup per connection)
