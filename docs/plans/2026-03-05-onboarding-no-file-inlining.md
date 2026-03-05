# Onboarding: Remove File Content Inlining

**Date:** 2026-03-05  
**Status:** Approved

## Problem

`onboarding` inlines README, CLAUDE.md, and build file verbatim into the response
(up to 32KB each). This causes "⚠ Large MCP response (~10.2k tokens)" warnings in
Claude Code and wastes context window on every first-run onboarding.

Two additional issues:
- CLAUDE.md is already in the agent's context in Claude environments (loaded by
  companion plugin) — inlining it is pure duplication
- Non-Claude environments may not auto-load CLAUDE.md, but the agent can still
  read it via `read_file` — inlining doesn't help there either

## Design: Approach A — Path Manifest Only

`gather_project_context` checks for file existence but does **not** read content.
`GatheredContext` drops the `readme`, `build_file_content`, and `claude_md` content
fields; keeps `build_file_name` (needed for the manifest listing).

`build_onboarding_prompt` replaces the inline content blocks with a path manifest:

```
**Key files found (read during Phase 1):**
- `README.md`
- `CLAUDE.md`
- `Cargo.toml`
```

The onboarding template already tells the agent to read these files in Phase 1 step 5.

## Changes

- `src/tools/workflow.rs` — `GatheredContext`, `gather_project_context`, `read_capped`, `MAX_GATHERED_FILE_BYTES` removed/simplified
- `src/prompts/mod.rs` — `build_onboarding_prompt` signature drops content params, outputs path manifest

## Expected Outcome

Onboarding response drops from potentially 15k+ tokens to ~5k (template + metadata only).
No "⚠ Large MCP response" warning. Agent reads files via `read_file` during Phase 1 as
it already should.
