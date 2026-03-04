# Research Validation: Progressive Disclosure & Tool Discovery for LLM Agents

This document maps current academic research to codescout's progressive
disclosure design — the `OutputGuard` two-mode system, overflow hints, tool
selection guidance, and `RecoverableError` with actionable hints.

The core argument: exposing everything upfront is actively harmful to LLM agents.
Progressive disclosure is not a UX nicety — it is a prerequisite for reliable
agent behavior.

---

## The Problem: Tool Overload & Prompt Bloat

When an agent receives too many tools, too many results, or too much detail, its
performance collapses. Three independent lines of evidence converge on this.

### "RAG-MCP: Mitigating Prompt Bloat in LLM Tool Selection via Retrieval-Augmented Generation"
*arXiv:2505.03275 — May 2025*
[[arXiv]](https://arxiv.org/abs/2505.03275)

Defines the problem directly: LLMs struggle to utilize a growing number of MCP
tools due to **prompt bloat and selection complexity**. Key result: exposing all
tool schemas to the model drops selection accuracy to **13.6%** on stress tests.
RAG-MCP fixes this by pre-selecting via semantic retrieval, cutting prompt tokens
by >50% and tripling accuracy to **43.1%**. The winning strategy is withholding
information — showing the agent only what it needs for the current query.

### "MCP-Zero: Active Tool Discovery for Autonomous LLM Agents"
*arXiv:2506.01056 — June 2025*
[[arXiv]](https://arxiv.org/abs/2506.01056)

Frames the problem architecturally: *"Current LLM agents inject pre-defined tool
schemas into prompts, reducing models to passive selectors."* MCP-Zero restores
**tool discovery autonomy** — agents actively request specific tools on-demand
rather than receiving all 2,797 tools from 308 MCP servers upfront. Result:
**98% reduction in token consumption** on APIBank while maintaining accuracy.

The core insight maps exactly to codescout's two-mode system: expose the map
first (exploring mode), let the agent navigate to what it needs (focused mode).

### "ToolLLM: Facilitating Large Language Models to Master 16,000+ Real-world APIs"
*arXiv:2307.16789 — ICLR 2024 Spotlight*
[[arXiv]](https://arxiv.org/abs/2307.16789)

The foundational paper establishing that naive exposure to large tool catalogs
fails. Solution: a neural API retriever that recommends appropriate APIs per
instruction rather than feeding all 16,464 APIs into context. The retrieval
architecture is what makes the scale tractable — not bigger context windows.

---

## Tool Description Quality Determines Selection Behavior

How tools are described is as important as what they do. Three papers show that
vague, ambiguous, or poorly structured descriptions cause agents to select the
wrong tool — consistently and at scale.

### "Tool Preferences in Agentic LLMs are Unreliable"
*arXiv:2505.18135 — 2025*
[[arXiv]](https://arxiv.org/abs/2505.18135)

Exposes a structural fragility: LLMs rely entirely on text descriptions to choose
tools, and this is *surprisingly fragile*. Minor edits to tool descriptions cause
**>10x change in tool usage** from GPT-4.1 and Qwen2.5-7B. The implication:
tool naming and description are not cosmetic — they are the selection mechanism.

This validates codescout's strict naming convention: `list_*` (enumerate),
`find_*` (search by criteria), `search_*` (text/semantic). Consistent, predictable
naming reduces the space of selection errors.

### "Learning to Rewrite Tool Descriptions for Reliable LLM-Agent Tool Use"
*arXiv:2602.20426 — 2026*
[[arXiv]](https://arxiv.org/abs/2602.20426)

Demonstrates that tool interfaces are *"largely human-oriented and often become
a bottleneck, especially when agents must select from large candidate tool sets."*
Performance depends on two factors equally: agent reasoning capability **and**
tool description quality. Tool descriptions need to be written for machine
consumers, not humans.

This is the rationale behind codescout's `server_instructions.md` — the
navigation guide that tells the agent *which tool to use given what it knows*:
"Know the name → LSP tools. Know the concept → semantic search. Know nothing →
list_dir + list_symbols."

### "How Good Are LLMs at Processing Tool Outputs?"
*arXiv:2510.15955 — 2025*
[[arXiv]](https://arxiv.org/abs/2510.15955)

Studies LLM performance on structured (JSON) tool responses. Core finding:
**semantic ambiguity in tool outputs causes systematic errors** — e.g., when
multiple JSON keys have similar meaning (a hotel with `name`, `room_name`, and
`name_without_policy`), models select the wrong value. Verbosity compounds this:
models *"regurgitate the input JSON response"* when outputs are too long, breaking
structured pipelines where one tool's output becomes another's input.

This validates codescout's output design: structured outputs with unambiguous
field names, compact representations that avoid key-name collision, and the
decision to return focused bodies only on explicit request.

---

## Tool Output Size Directly Controls Agent Performance

Beyond tool selection, the *volume of individual tool responses* matters.

### "Context Length Alone Hurts LLM Performance Despite Perfect Retrieval"
*arXiv:2510.05381 — 2025*
[[arXiv]](https://arxiv.org/abs/2510.05381)

Even when a model can recite all relevant tokens with 100% exact match, performance
degrades as input grows. Tested across 5 models (including Claude and GPT-4.1)
on code generation: **success rates drop from 40–50% baseline to under 10%** in
long-context conditions. Context length alone — not noise, not retrieval failure —
is the cause.

The implication for tool design: every token in a tool's response is borrowed from
the agent's reasoning budget. Compact tool responses are not just efficient, they
are a prerequisite for reliable downstream reasoning.

### "Improving the Efficiency of LLM Agent Systems through Trajectory Reduction" (AgentDiet)
*arXiv:2509.23586 — 2025*
[[arXiv]](https://arxiv.org/abs/2509.23586)

Post-hoc analysis of real SWE-bench agent trajectories identifies three classes
of token waste, the largest being **irrelevant information in read operations**
(cache files, resource files, long directory listings). Removing this waste
achieves **39–60% token reduction** with no performance loss.

Code-explorer's exploring mode was designed to never produce this waste in the
first place: directory listings cap at 200, symbol lists truncate with overflow
hints, and source files are never returned as raw text.

### "SWE-Pruner: Self-Adaptive Context Pruning for Coding Agents"
*arXiv:2601.16746 — 2025*
[[arXiv]](https://arxiv.org/html/2601.16746v1)

Finds that **read operations constitute 76.1% of all tokens** in coding agent
trajectories. SWE-Pruner's key contribution is pruning these aggressively (23–38%
reduction) without performance loss. The "read-heavy" cost profile is exactly what
codescout's architecture avoids by routing through symbol tools instead of
raw file access.

---

## The Overflow Hint Pattern: Actionable Guidance on Truncation

Codescout's overflow responses include a `hint` field and `by_file` distribution
map — telling the agent *how to narrow the search* rather than just truncating.
Research validates why this matters.

### "SWE-agent: Agent-Computer Interfaces Enable Automated Software Engineering"
*Yang et al. — NeurIPS 2024*
[[arXiv:2405.15793]](https://arxiv.org/abs/2405.15793)

Identifies that good ACI design includes **informative error messages and history
processors**. The key design principle: when the agent can't proceed, the interface
should tell it why and how to recover. A truncated response with no guidance forces
the agent to guess; a truncated response with a narrowing hint keeps the agent on
track.

SWE-agent's own history collapsing — where older observations are reduced to a
single line — is the macroscopic version of what `OutputGuard` does at the
tool-response level.

### "Tool-to-Agent Retrieval: Bridging Tools and Agents for Scalable LLM Multi-Agent Systems"
*arXiv:2511.01854 — 2025*
[[arXiv]](https://arxiv.org/abs/2511.01854)

Finds that coarse agent-level descriptions used for routing *"obscures fine-grained
tool functionality and often results in suboptimal agent selection."* The fix:
embed both tools **and** their parent agents in a shared vector space with metadata
relationships, enabling the agent to navigate from coarse to fine. This is the
multi-agent equivalent of codescout's explore → focus workflow.

---

## RecoverableError vs Fatal Error: A Two-Tier Signal

Codescout routes tool errors through two paths: `RecoverableError` (the agent
made a correctable mistake, here's the hint) vs. `anyhow::bail!` (something truly
broke). Research validates why conflating these harms agents.

### "BiasBusters: Uncovering and Mitigating Tool Selection Bias in Large Language Models"
*arXiv:2510.00307 — 2025*
[[arXiv]](https://arxiv.org/html/2510.00307)

Demonstrates that agents form persistent selection biases based on superficial
signals — tool names, description ordering, prior error signals. When a tool
returns an opaque failure (isError: true), agents learn to avoid it even when the
error was their own fault (bad parameters). Surfacing correctable errors as
structured guidance (isError: false with explanation) keeps the agent's belief
about tool reliability accurate.

### "Evaluation and Benchmarking of LLM Agents: A Survey"
*arXiv:2507.21504 — 2025*
[[arXiv]](https://arxiv.org/html/2507.21504v1)

Identifies **tool selection, parameter mapping, and execution sequencing** as the
three main failure modes in agentic tool use. A tool that fails silently or returns
ambiguous errors corrupts all three. Actionable error signals that distinguish
"wrong tool" from "wrong parameters" from "right tool, wrong path" allow agents to
self-correct precisely.

---

## Navigation Guidance in Server Instructions

Codescout's `server_instructions.md` provides a decision tree for tool
selection: what to use when. Research validates that this meta-level guidance
is not optional.

### "Configuring Agentic AI Coding Tools: An Exploratory Study"
*arXiv:2602.14690 — 2026*
[[arXiv]](https://arxiv.org/html/2602.14690v1)

Surveyed 2,926 GitHub repositories. Finds that *"Claude Code users employ the
broadest range of mechanisms"* including skills and context files, and that
advanced configuration patterns (workflow scripts, decision guidance) are under-
adopted despite clear effectiveness. The paper implicitly validates front-loading
selection guidance in system prompts — the most effective configurations specify
not just what tools exist but when to use them.

### "On the Impact of AGENTS.md Files on the Efficiency of AI Coding Agents"
*arXiv:2601.20404 — 2026*
[[arXiv]](https://arxiv.org/html/2601.20404v1)

Controlled experiment: agents with AGENTS.md context files show **lower runtime
and token consumption** while maintaining task completion. The efficiency gain
comes from the agent avoiding exploratory wrong turns — it already knows the
structure and conventions. This is exactly what the tool-selection guidance section
of `server_instructions.md` provides.

---

## Summary: What the Literature Says codescout Gets Right

| codescout Pattern | Research Backing | Finding |
|---|---|---|
| Compact exploring mode (200-item cap) | RAG-MCP (2025) | All-tools-upfront: 13.6% accuracy |
| On-demand detail via `detail_level: "full"` | MCP-Zero (2025) | 98% token reduction with on-demand discovery |
| Overflow hints + `by_file` distribution | SWE-agent NeurIPS'24 | Informative truncation keeps agent on track |
| `list_*` / `find_*` / `search_*` naming | Tool Preferences Unreliable (2025) | Description edits cause >10x selection change |
| Server instructions with decision tree | Configuring Tools (2026) | Selection guidance reduces token waste |
| `RecoverableError` vs `anyhow::bail!` | BiasBusters (2025) | Conflating errors causes persistent tool-avoidance bias |
| `read_file` blocks source code | SWE-Pruner (2025) | Reads = 76% of all agent tokens |
| No raw directory dumps | AgentDiet (2025) | 40–60% of tokens are irrelevant listing waste |
| Unambiguous JSON field names in output | How Good Are LLMs at Tool Outputs (2025) | Semantic ambiguity in keys causes wrong-value errors |
| CLAUDE.md / project memory as onboarding | AGENTS.md Impact (2026) | Context files lower runtime + token consumption |

The research converges on a single principle: **the agent's context window is not
a dump site — it is a limited workspace**. Every design choice in codescout's
output layer is an application of that principle.
