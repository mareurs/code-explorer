# Research Validation: The Science Behind codescout

This document maps current academic research to codescout's design decisions,
showing that each major architectural choice is independently validated by papers
from the LLM agents and software engineering community.

---

## 1. Token Efficiency & Compact-by-Default Output

**Design**: Tools default to compact `Exploring` mode (capped at 200 items), with
`Focused` mode behind `detail_level: "full"`. `OutputGuard` enforces this
project-wide.

### "Lost in the Middle: How Language Models Use Long Contexts"
*Liu et al. — TACL 2024 (oral at ICLR 2024)*
[[arXiv:2307.03172]](https://arxiv.org/abs/2307.03172) · [[ACL Anthology]](https://aclanthology.org/2024.tacl-1.9/)

The landmark finding: LLM performance peaks when relevant information is at the
**beginning or end** of the context, and **degrades significantly** when it falls
in the middle — even in "long-context" models. Directly validates why codescout
tools keep output compact and lead with the most relevant items.

### "Context Length Alone Hurts LLM Performance Despite Perfect Retrieval"
*arXiv:2510.05381 — 2025*
[[arXiv]](https://arxiv.org/abs/2510.05381)

Even when a model can perfectly recite every token with 100% exact match,
**performance still degrades substantially as input length increases**. Demonstrated
across 5 models on math, QA, and code generation. Sheer context volume hurts
performance independent of retrieval quality — the strongest possible justification
for output caps.

### "Improving the Efficiency of LLM Agent Systems through Trajectory Reduction" (AgentDiet)
*arXiv:2509.23586 — 2025*
[[arXiv]](https://arxiv.org/abs/2509.23586)

Analyzed trajectories of top SWE-bench agents (Claude Sonnet-based): **39–60% of
input tokens can be removed** while maintaining performance. Typical waste:
irrelevant cache/resource files in repo enumeration. Directly validates the
`find_symbol` / `list_symbols` compact-by-default strategy over raw `cat`.

### "SWE-Pruner: Self-Adaptive Context Pruning for Coding Agents"
*arXiv:2601.16746 — 2025*
[[arXiv]](https://arxiv.org/html/2601.16746v1)

**Read operations dominate token consumption at 76.1%** of total tokens in coding
agents. SWE-Pruner achieves 23–38% token reduction with <1% performance
degradation. The empirical backbone for `read_file` refusing source code files and
redirecting to symbol tools.

---

## 2. LSP-Backed Symbol Navigation

**Design**: 9 tools backed by Language Server Protocol — the same infrastructure
IDEs use. `find_symbol`, `goto_definition`, `hover`, `find_references`,
`list_symbols`, `replace_symbol`, `remove_symbol`, `insert_code`, `rename_symbol`.

### "Language Server CLI Empowers Language Agents with Process Rewards" (Lanser-CLI)
*arXiv:2510.22907 — 2025*
[[arXiv]](https://arxiv.org/abs/2510.22907)

The core thesis: *"Large language models routinely hallucinate APIs and mislocalize
edits, while language servers compute verified, IDE-grade facts about real code."*
LSP servers provide **verifiable facts: definitions, references, types, diagnostics,
and safe edits** — exactly what codescout exposes. The closest academic sibling
to codescout's architecture.

### "LSPRAG: LSP-Guided RAG for Language-Agnostic Real-Time Unit Test Generation"
*arXiv:2510.22210 — 2025*
[[arXiv]](https://arxiv.org/html/2510.22210v1)

Uses LSP to generate language-agnostic unit tests, demonstrating LSP provides
structural context that improves both line coverage and valid test rates. Validates
the language-agnostic approach (20+ extensions supported in codescout).

### "An Exploratory Study of Code Retrieval Techniques in Coding Agents"
*Preprints.org, Oct 2025*
[[preprints.org]](https://www.preprints.org/manuscript/202510.0924)

Direct comparison of retrieval strategies — lexical search (grep), semantic search
(RAG), LSP symbol navigation, and multi-agent architectures. Key finding: *"LSP
integration provides structured symbol navigation (go-to-definition, find-references,
workspace symbol search) mirroring the tools used by human developers in IDEs."*
Validates exposing the full LSP surface rather than only grep.

---

## 3. Agent-Computer Interface (ACI) — Tool Design Philosophy

**Design**: Tools are designed around agent ergonomics: consistent `list_*` /
`find_*` / `search_*` naming, overflow hints with `by_file` distribution maps,
actionable error messages via `RecoverableError`.

### "SWE-agent: Agent-Computer Interfaces Enable Automated Software Engineering"
*Yang et al. — NeurIPS 2024*
[[arXiv:2405.15793]](https://arxiv.org/abs/2405.15793) · [[NeurIPS proceedings]](https://proceedings.neurips.cc/paper_files/paper/2024/file/5a7c947568c1b1328ccc5230172e1e7c-Paper-Conference.pdf)

Introduced the **ACI (Agent-Computer Interface)** concept: a fixed LM with a
carefully designed interface outperforms a smarter LM with a poor interface. Key
mechanism: *"observations preceding the last 5 are each collapsed into a single
line"* — the same progressive disclosure / output collapsing that OutputGuard
implements. Achieved SOTA on SWE-bench by careful interface design alone.

---

## 4. Semantic Search via Embeddings

**Design**: Vector embedding index + cosine similarity for concept-level retrieval
(`semantic_search`, `index_project`), with incremental updates and drift detection
(`project_status`).

### "LLM Agents Improve Semantic Code Search"
*arXiv:2408.11058 — 2024*
[[arXiv]](https://arxiv.org/abs/2408.11058)

RAG-powered agents that inject semantic context into queries significantly outperform
direct embedding lookup. Validates the combination of semantic search + symbol tools
that codescout provides (search to find the right area, then navigate precisely
with LSP).

### "Retrieval-Augmented Code Generation: A Survey with Focus on Repository-Level Approaches"
*arXiv:2510.04905 — 2025*
[[arXiv]](https://arxiv.org/html/2510.04905v1)

Comprehensive survey establishing RAG as the baseline architecture for
repository-level coding agents. Validates the embedding pipeline in `src/embed/`.

---

## 5. Memory & Project Context

**Design**: Per-project persistent memory (`src/memory/`), configuration
(`.codescout/project.toml`), and project instructions (`CLAUDE.md`) that
survive across sessions.

### "Codified Context: Infrastructure for AI Agents in a Complex Codebase"
*arXiv:2602.20478 — 2026*
[[arXiv]](https://arxiv.org/abs/2602.20478)

Documents a production system (108,000-line C# codebase, 283 development sessions)
using a three-layer approach: *"(1) hot-memory constitution encoding conventions,
retrieval hooks, and orchestration protocols; (2) 19 specialized domain-expert
agents; (3) cold-memory knowledge base of 34 on-demand specification documents."*
This is the real-world pattern that codescout's `memory/` store + `project.toml`
+ on-demand tool documentation implements.

### "Agent READMEs: An Empirical Study of Context Files for Agentic Coding"
*arXiv:2511.12884 — 2025*
[[arXiv]](https://arxiv.org/html/2511.12884v1)

Empirical analysis of CLAUDE.md / AGENTS.md files across GitHub. Finds they are
*actively maintained, structurally consistent, heavily focused on functional
instructions.* 72.6% of CLAUDE.md files define architecture. Validates the
`onboarding` tool and project-context design.

### "On the Impact of AGENTS.md Files on the Efficiency of AI Coding Agents"
*arXiv:2601.20404 — 2026*
[[arXiv]](https://arxiv.org/html/2601.20404v1)

**Controlled experiment**: agents with AGENTS.md files show *lower runtime and token
consumption* while maintaining comparable task completion. Directly validates why
codescout surfaces `CLAUDE.md` and project memory as first-class context through
the `onboarding` tool.

---

## 6. MCP as the Delivery Protocol

**Design**: Code-explorer delivers all tools via Model Context Protocol (MCP),
exposing them through `rmcp ServerHandler`.

### "MCP-Bench: Benchmarking Tool-Using LLM Agents with Complex Real-World Tasks via MCP Servers"
*arXiv:2508.20453 — 2025*
[[arXiv]](https://arxiv.org/abs/2508.20453)

The first rigorous benchmark for MCP-based tool use, covering 28 MCP servers and
250 tools. Tests tool selection from fuzzy instructions, multi-hop planning, and
cross-tool orchestration — all scenarios codescout tools must handle. Validates
MCP as the right protocol layer.

### "Configuring Agentic AI Coding Tools: An Exploratory Study"
*arXiv:2602.14690 — 2026*
[[arXiv]](https://arxiv.org/html/2602.14690v1)

Systematic analysis of configuration mechanisms across Claude Code, GitHub Copilot,
Cursor, Gemini, and Codex across 2,926 repositories. Finds distinct configuration
cultures forming around different tools, with Claude Code users employing the
broadest range of mechanisms (context files, skills, subagents). Validates the
MCP + configuration-file ecosystem codescout operates in.

---

## Summary

| codescout Feature | Validating Paper | Key Finding |
|---|---|---|
| Compact default output (OutputGuard) | Liu et al. TACL'24 | Info in middle of context is lost |
| Output caps, overflow hints | arXiv:2510.05381 | Context length alone degrades performance |
| `read_file` blocks source code | SWE-Pruner 2025 | Read ops = 76% of all tokens in agents |
| Trajectory compression via compact tools | AgentDiet 2025 | 40–60% tokens removable without loss |
| LSP navigation tools | Lanser-CLI 2025 | LSP computes verifiable facts, not hallucinations |
| Symbol-level navigation (goto_definition, etc.) | Code Retrieval Study 2025 | LSP mirrors how human developers use IDEs |
| ACI/tool design ergonomics | SWE-agent NeurIPS'24 | Interface design > model capability |
| Semantic search + embeddings | LLM Agents + Semantic Code 2024 | RAG + agents > retrieval alone |
| Per-project memory store | Codified Context 2026 | Hot/cold memory enables cross-session coherence |
| CLAUDE.md / project.toml as context | AGENTS.md Impact 2026 | Context files reduce runtime and tokens |
| MCP protocol | MCP-Bench 2025 | MCP is the emerging standard for tool-using agents |

The most striking pattern: papers like SWE-agent and AgentDiet are *optimizing
away* the bloat that codescout *never creates*. The "context rot" problem — raw
file reads, large grep dumps, unfiltered directory listings — is exactly what the
OutputGuard + symbol-tool architecture prevents by design.
