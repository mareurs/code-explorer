# Contributor Skills — Design

**Date:** 2026-02-26
**Status:** Planned
**Location:** `.claude/skills/` within the code-explorer repo

## Overview

Three Claude Code skills for contributors working on the code-explorer Rust codebase. Skills live alongside the codebase so anyone who clones the repo and uses Claude Code gets them automatically. They are prompting/workflow assets — not MCP tools — and ship independently of the server binary.

## Skills

### 1. `project-management`

**Purpose:** Help contributors understand current sprint status, navigate the roadmap, and work with GitHub issues and PRs without manually cross-referencing docs.

**Workflow it enables:**
- Surface the current phase and sprint from `docs/ROADMAP.md`
- Map recent commits (`git_log`) to sprint items
- List open PRs and issues via GitHub MCP
- Guide contributors on how to pick up the next task and open a correctly-structured PR

**Key tools used:**
- `git_log`, `git_diff` — recent commit context
- GitHub MCP (`list_pull_requests`, `list_issues`, `search_issues`)
- `read_file` on `docs/ROADMAP.md` and `docs/plans/`

**Dependencies:** None. Ready to implement.

---

### 2. `debugging`

**Purpose:** Systematic debugging workflow for the code-explorer Rust codebase — from symptom to fix to verification.

**Workflow it enables:**
- Symptom classification: build failure / test failure / LSP timeout / tree-sitter parse error / embedding pipeline issue
- Hypothesis formation using `semantic_search` and `find_symbol`
- Targeted investigation (`search_for_pattern`, `get_symbols_overview`, `git_blame`)
- Fix → `cargo test` → verification loop
- Guided escalation path (add tracing, isolate with a unit test, bisect with `git_log`)

**Key tools used:**
- All code-explorer symbol/semantic tools for navigation
- `execute_shell_command` for `cargo build`, `cargo test`, `cargo clippy`
- `git_blame`, `git_log` for regression hunting

**Dependencies:** None. Ready to implement.

---

### 3. `log-stat-analyzer`

**Purpose:** Structured workflow for interpreting `usage.db` Tool Usage Monitor data — spotting call pattern drift, error rate spikes, and latency regressions.

**Workflow it enables:**
- Query `get_usage_stats` for per-tool call counts, error rates, p50/p99 latency
- Compare time buckets (last hour / day / week) to detect drift
- Flag high overflow rates (agent calling too broadly)
- Produce a structured summary with actionable findings (e.g. "semantic_search error rate up 3× in last 24h — investigate embed backend")

**Key tools used:**
- `get_usage_stats` (planned MCP tool from Tool Usage Monitor)
- `read_file` on `.code-explorer/usage.db` as fallback if SQL access is needed

**Dependencies:** Blocked on Tool Usage Monitor implementation (see ROADMAP — Future Improvements).

---

## Location & Delivery

Skills are `.md` files placed in `.claude/skills/` at the repo root. Contributors who open the repo in Claude Code automatically have them available. No build step required.

```
.claude/
└── skills/
    ├── project-management.md
    ├── debugging.md
    └── log-stat-analyzer.md
```

## Status

| Skill | Status |
|---|---|
| `project-management` | Planned — no blockers |
| `debugging` | Planned — no blockers |
| `log-stat-analyzer` | Blocked — requires Tool Usage Monitor |
