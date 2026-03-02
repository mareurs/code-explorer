# Code-Explorer Bug Log

This is a living document. **Update it whenever you discover a bug in code-explorer's own
logic — regardless of whether you're fixing it immediately.** Each entry should capture:
what the wrong behavior is, where it lives, how to reproduce it, and a root cause hypothesis.

See `docs/TODO-tool-misbehaviors.md` for MCP tool UX bugs (wrong output, misleading errors,
etc). This file is for **correctness bugs** in the implementation itself.

---

## Prompt for future sessions

> Before starting any task, scan this file for known bugs in the area you're working on.
> When you discover a bug — even if you're not fixing it — **add an entry immediately**
> before continuing. When you fix one, mark it resolved with the commit reference.
> The goal is a reliable audit trail that makes regressions obvious and fast to diagnose.

---

## Open Bugs

*(none yet — add entries as discovered)*

---

## Resolved Bugs

*(move entries here when fixed, with commit/PR reference)*

---

## Template

```
### BUG-NNN — <short title>

**Date:** YYYY-MM-DD
**Severity:** Low | Medium | High | Critical
**Status:** 🐛 OPEN | 🔧 IN PROGRESS | ✅ FIXED (commit abc1234)

**What:** <one-sentence description of the wrong behavior>

**Where:** `src/path/to/file.rs` — `function_name()` (approx. line N)

**Repro:**
<minimal steps or failing test case>

**Root cause:**
<hypothesis or confirmed cause>

**Fix:**
<what was changed, or "TBD">
```
