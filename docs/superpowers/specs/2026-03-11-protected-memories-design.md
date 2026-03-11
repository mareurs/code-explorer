# Protected Memories — Design Spec

**Date:** 2026-03-11
**Status:** Draft
**Branch:** experiments

## Problem

When `onboarding(force=true)` runs, it overwrites all 6 onboarding memories
(`project-overview`, `architecture`, `conventions`, `development-commands`,
`domain-glossary`, `gotchas`). User-curated content — especially in `gotchas` —
is lost with no merge or confirmation.

## Goal

Certain memory topics survive force re-onboarding via a hybrid flow:
anchor-based staleness check (fast, deterministic) + LLM verification of stale
entries + user approval before writing.

## Design

### Configuration

New field in `project.toml` under `[memory]`:

```toml
[memory]
protected = ["gotchas"]
```

- `protected`: `Vec<String>` of topic names that onboarding must not blindly
  overwrite.
- **Default:** `["gotchas"]` — via a `#[serde(default = "default_protected_topics")]`
  helper function returning `vec!["gotchas".to_string()]`, matching the existing
  per-field default pattern in `MemorySection`.
- Users can add any topic name, including custom ones they created manually.

**Implementation:** Add `protected: Vec<String>` to the existing `MemorySection`
struct in `src/config/project.rs` with a per-field serde default function.

### Rust Changes (workflow.rs — onboarding `call()`)

After the language/file scan but **before** writing memories (~line 870), a new
step gathers protected-memory state:

1. Read `protected` list from `config.memory.protected`.
2. Filter out programmatic topics (`onboarding`, `language-patterns`) — these
   are always machine-generated and silently excluded from protection.
3. For each remaining protected topic that already has content in `MemoryStore`:
   - Read the existing memory content via `memory.read(topic)`.
   - Check whether the anchor sidecar file exists on disk via
     `anchor_path_for_topic()`. If it does not exist, mark as `untracked`.
   - If the anchor file exists, compute staleness via
     `check_path_staleness()`.
4. Bundle into the onboarding result JSON as a new top-level field:

```json
{
  "protected_memories": {
    "gotchas": {
      "exists": true,
      "content": "# Gotchas & Known Issues\n...",
      "staleness": {
        "stale_files": [
          { "path": "src/tools/output_buffer.rs", "status": "changed" },
          { "path": "src/old_module.rs", "status": "deleted" }
        ],
        "untracked": false
      }
    }
  }
}
```

The `staleness` shape maps directly to what `check_path_staleness()` already
returns:
- `stale_files`: `Vec<{path, status}>` from `StalenessReport.stale_files`
  where `status` is `AnchorStatus::Changed` or `AnchorStatus::Deleted`
  serialized as `"changed"` / `"deleted"`.
- `untracked`: `true` when the `.anchors.toml` sidecar does not exist on
  disk (checked via `anchor_path_for_topic().exists()`). When `untracked`
  is true, `stale_files` is empty (no anchors to check).
- Fresh files are **not** listed — the LLM can infer "everything not stale
  is fresh." This avoids noise.

5. For protected topics that don't exist yet: `"exists": false`.

**Key principle:** The Rust code **computes** staleness but does **not** write
protected memories. It hands structured data to the LLM via the JSON result
(not injected into the prompt template string). The onboarding prompt
instructions reference the `protected_memories` JSON field by name, and the
LLM applies the merge flow based on what it finds there.

**Non-force onboarding (`force=false`):** Returns early with the existing
status message. No protected-memory data is computed or returned, since
non-force onboarding never writes memories.

### Prompt Changes (onboarding_prompt.md — Phase 2)

Phase 2 ("Write the 6 Memories") gains a conditional flow. Before writing each
memory, check if it appears in `protected_memories` from the onboarding result
JSON:

#### Protected + no stale files and not untracked → skip

Keep as-is. Tell the user:
> "Kept `gotchas` unchanged (all references still valid)."

#### Protected + stale files or untracked → hybrid merge flow

1. Read the existing content from `protected_memories[topic].content`.
2. For entries referencing stale/deleted files: read the relevant source
   files and verify whether each entry is still accurate.
3. Identify new gotchas discovered during Phase 1 exploration.
4. Present a diff-style summary to the user:
   - **Stale (recommend removing):** [entries no longer accurate]
   - **Still valid (keeping):** [verified entries]
   - **New findings:** [discoveries from fresh exploration]
   - **Proposed merged version:** [full content]
5. **Wait for user approval** before calling `memory(action="write")`.

#### Protected + doesn't exist → create fresh

No existing content to protect. Write as today.

#### Unprotected → overwrite as today

No change in behavior.

### Edge Cases

| Scenario | Behavior |
|---|---|
| First onboarding (no memories) | All protected topics have `exists: false` — created fresh |
| Non-force onboarding (`force=false`) | Early return, no memory writes, no staleness computed |
| Custom topic in `protected` that onboarding doesn't write | Harmless — Rust reports staleness, prompt never writes it |
| User removes a topic from `protected` | Onboarding overwrites it freely |
| `onboarding` or `language-patterns` in `protected` | Silently excluded — always programmatic |
| No anchor sidecar for a protected memory | `untracked: true` — LLM verifies all entries |
| Memory written by another session between check and merge | Possible but unlikely; content field may be stale. Acceptable risk — the merge is user-approved. |

## Files to Change

| File | Change |
|---|---|
| `src/config/project.rs` | Add `protected: Vec<String>` to `MemorySection` with `#[serde(default = "default_protected_topics")]` |
| `src/memory/anchors.rs` | Serialize `AnchorStatus` as `"changed"` / `"deleted"` for JSON output (add `Serialize` derive or manual impl) |
| `src/tools/workflow.rs` | Gather protected-memory state, include `protected_memories` in onboarding result JSON |
| `src/prompts/onboarding_prompt.md` | Add conditional merge/approve flow in Phase 2 |

## Testing

| Test | Location | What it verifies |
|---|---|---|
| `onboarding_includes_protected_memories_for_existing_topic` | `workflow.rs` | Protected topic with content → `protected_memories` JSON has `exists: true`, content, staleness |
| `onboarding_protected_memory_missing_topic` | `workflow.rs` | Protected topic without content → `exists: false` |
| `onboarding_excludes_programmatic_from_protected` | `workflow.rs` | `onboarding` and `language-patterns` in config `protected` list → excluded from `protected_memories` |
| `onboarding_protected_memory_untracked_no_anchors` | `workflow.rs` | Protected topic with content but no `.anchors.toml` → `untracked: true` |
| `memory_section_serde_roundtrip_with_protected` | `config/project.rs` | TOML with `protected = ["gotchas", "conventions"]` round-trips correctly |
| `memory_section_default_includes_gotchas` | `config/project.rs` | Default `MemorySection` has `protected = ["gotchas"]` |

## Out of Scope

- **Hard write protection in `MemoryStore`:** Direct `memory(action="write")`
  calls outside onboarding are not blocked. Protection is at the decision
  layer, not the storage layer.
- **Semantic memory protection:** Only markdown topic memories are covered.
  `remember`/`recall`/`forget` are unaffected.
- **Non-onboarding memory writes:** If the user or LLM explicitly writes to a
  protected topic via the memory tool, that's intentional and allowed.
