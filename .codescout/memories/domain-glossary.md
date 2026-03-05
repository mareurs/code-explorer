# Domain Glossary

**OutputGuard** — The shared struct (`src/tools/output.rs`) that enforces progressive disclosure in every tool. Wraps result formatting with Exploring/Focused mode switching and overflow hint generation.

**OutputBuffer / @ref handles** — Large tool outputs stored server-side as `@cmd_*`, `@file_*`, or `@tool_*` handles. Claude queries them with shell commands (`grep FAILED @cmd_id`) rather than receiving the full text. `src/tools/output_buffer.rs`.

**RecoverableError** — An error type (`src/tools/mod.rs:67`) for expected, input-driven failures (bad path, unsupported file type). Routes to `isError: false` in MCP so sibling parallel calls aren't aborted. Contrasts with `anyhow::bail!` for true crashes.

**ActiveProject** — The currently active project root + config + memory store held inside `Agent` (`src/agent.rs:48`). Switchable at runtime via `activate_project` tool (needed after `EnterWorktree`).

**ToolContext** — The bag of shared services passed into every tool's `call()` method (`src/tools/mod.rs:47`). Contains Agent, LspManager, OutputBufferStore, ProgressReporter.

**Exploring / Focused mode** — Two output modes enforced by OutputGuard. Exploring = compact default (≤200 items). Focused = `detail_level: "full"` + offset/limit pagination. See `docs/PROGRESSIVE_DISCOVERABILITY.md`.

**Scope** — `enum Scope` in `src/library/` distinguishing `Project` (current root) vs `Lib(name)` (third-party library source indexed separately).

**Drift detection** — After re-indexing, measures how much code changed in *meaning* (embedding distance), not just bytes. Exposed via `project_status`. Configurable in `.code-explorer/project.toml`.

**Three-query sandwich** — Testing pattern for cache invalidation: (1) baseline query, (2) mutate without notification, (3) assert stale, (4) trigger invalidation, (5) assert fresh. Two-query is insufficient — see CLAUDE.md.

**Three prompt surfaces** — `server_instructions.md`, `onboarding_prompt.md`, `build_system_prompt_draft()` — all three reference tool names and must be kept in sync when tools are renamed.

**code-explorer-routing** — Companion Claude Code plugin (`../claude-plugins/code-explorer-routing/`) that hooks into every session/subagent to enforce codescout tool use and block native Read/Grep/Glob on source files.
