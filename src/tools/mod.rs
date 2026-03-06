//! Tool trait and registry.
//!
//! Each tool is a struct that implements the `Tool` trait. Tools are
//! registered in the MCP server at startup.

pub mod ast;
pub mod command_summary;
pub mod config;
pub mod file;
pub mod file_summary;
pub(crate) mod format;
pub mod git;
pub mod github;
pub mod library;
pub mod memory;
pub mod output;
pub mod output_buffer;
pub mod progress;
pub mod semantic;
pub mod symbol;
pub mod usage;
pub use usage::GetUsageStats;
pub mod workflow;

use std::sync::Arc;

use anyhow::Result;
use rmcp::model::Content;
use serde_json::Value;

use crate::agent::Agent;
use crate::lsp::LspProvider;

/// Maximum estimated tokens for inline tool output.
/// Content exceeding this is buffered and summarized.
/// Token estimate: ~4 bytes per token.
pub(crate) const MAX_INLINE_TOKENS: usize = 2_500;

/// Byte equivalent of MAX_INLINE_TOKENS — used for byte-budget arithmetic
/// in truncation code (run_command buffer-only paths).
pub(crate) const TOOL_OUTPUT_BUFFER_THRESHOLD: usize = MAX_INLINE_TOKENS * 4;

/// Check whether content should be buffered based on estimated token count.
pub(crate) fn exceeds_inline_limit(text: &str) -> bool {
    text.len() / 4 > MAX_INLINE_TOKENS
}
/// Soft cap for compact summaries shown alongside `@tool_*` refs.
/// Truncation prefers whole-line boundaries. See [`truncate_compact`].
pub(crate) const COMPACT_SUMMARY_MAX_BYTES: usize = 2_000;
/// Hard cap — no summary will exceed this size regardless of line boundaries.
pub(crate) const COMPACT_SUMMARY_HARD_MAX_BYTES: usize = 3_000;

/// Shared context passed to every tool invocation.
///
/// Holds references to all shared resources (agent state, LSP manager,
/// and eventually parser pool, etc.). Extend this struct as new shared
/// resources are added — all tools get access automatically.
pub struct ToolContext {
    pub agent: Agent,
    pub lsp: Arc<dyn LspProvider>,
    pub output_buffer: Arc<output_buffer::OutputBuffer>,
    pub progress: Option<Arc<progress::ProgressReporter>>,
}

/// A recoverable tool error: the LLM gave bad input and can self-correct.
///
/// When a tool returns this error type, the MCP server serialises it as
/// `isError: false` with a JSON body containing `"error"` and optional
/// `"hint"` fields.  This prevents Claude Code from aborting sibling
/// parallel tool calls (which it does when it sees `isError: true`).
///
/// Use this for **expected, input-driven failures**: path not found,
/// unsupported file type, empty glob match, no index built yet, etc.
///
/// Keep returning plain `anyhow` errors (→ `isError: true`) for genuine
/// failures: panics, security violations, LSP crashes.
#[derive(Debug)]
pub struct RecoverableError {
    /// Human-readable description of what went wrong.
    pub message: String,
    /// Optional LLM-facing suggestion for how to correct the call.
    pub hint: Option<String>,
}

impl RecoverableError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: Some(hint.into()),
        }
    }
}

impl std::fmt::Display for RecoverableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for RecoverableError {}

/// Convenience: extract a required parameter from a JSON `Value`, returning
/// `RecoverableError` (not a fatal error) if it is missing.
pub fn require_param<'a>(
    input: &'a serde_json::Value,
    name: &str,
) -> anyhow::Result<&'a serde_json::Value> {
    input.get(name).ok_or_else(|| {
        RecoverableError::with_hint(
            format!("missing '{}' parameter", name),
            format!("Add the required '{}' parameter to the tool call.", name),
        )
        .into()
    })
}

/// Convenience: extract a required string parameter from a JSON `Value`.
pub fn require_str_param<'a>(input: &'a serde_json::Value, name: &str) -> anyhow::Result<&'a str> {
    require_param(input, name)?.as_str().ok_or_else(|| {
        RecoverableError::with_hint(
            format!("'{}' must be a string", name),
            format!("Provide '{}' as a string value.", name),
        )
        .into()
    })
}

/// Convenience: extract a required u64 parameter from a JSON `Value`.
pub fn require_u64_param(input: &serde_json::Value, name: &str) -> anyhow::Result<u64> {
    let val = require_param(input, name)?;
    // Accept both JSON numbers and string-encoded integers (LLMs sometimes quote them).
    if let Some(n) = val.as_u64() {
        return Ok(n);
    }
    if let Some(s) = val.as_str() {
        if let Ok(n) = s.trim().parse::<u64>() {
            return Ok(n);
        }
    }
    Err(RecoverableError::with_hint(
        format!("'{}' must be a non-negative integer", name),
        format!("Provide '{}' as a non-negative integer.", name),
    )
    .into())
}

/// Parse a boolean parameter from a JSON `Value`.
///
/// MCP clients (including Claude Code) may serialize boolean parameters as
/// JSON strings (`"true"` / `"false"`) rather than native JSON booleans.
/// This helper accepts both representations, defaulting to `false`.
pub fn parse_bool_param(val: &serde_json::Value) -> bool {
    val.as_bool()
        .or_else(|| val.as_str().and_then(|s| s.parse::<bool>().ok()))
        .unwrap_or(false)
}

/// Block write operations when git worktrees exist but the agent hasn't
/// explicitly called `activate_project` to confirm which project to write to.
///
/// Returns `Ok(())` when writes are allowed:
/// - Agent explicitly activated a project via `activate_project`
/// - No git worktrees exist (no ambiguity)
///
/// Returns `RecoverableError` when writes should be blocked:
/// - Worktrees exist AND the project was only implicitly set at startup
pub async fn guard_worktree_write(ctx: &ToolContext) -> anyhow::Result<()> {
    if ctx.agent.is_project_explicitly_activated().await {
        return Ok(());
    }
    let root = ctx.agent.require_project_root().await?;
    let worktrees = crate::util::path_security::list_git_worktrees(&root);
    if worktrees.is_empty() {
        return Ok(());
    }
    let wt_list: Vec<String> = worktrees.iter().map(|p| p.display().to_string()).collect();
    let hint = format!(
        "Call activate_project(\"{}\") to select the write target (or use \"{}\") for the main repo.",
        wt_list[0],
        root.display()
    );
    Err(RecoverableError::with_hint(
        format!(
            "Write blocked: git worktrees detected but activate_project has not been called. \
             Worktrees: [{}]",
            wt_list.join(", ")
        ),
        hint,
    )
    .into())
}

/// Truncate a compact summary to fit within output size limits, preserving line structure.
///
/// Returns `text` verbatim when `text.len() <= soft_max`. Otherwise, finds the last `\n`
/// within `hard_max` bytes and truncates there (keeping whole lines). When no newline
/// exists within `hard_max`, truncates at `hard_max` bytes directly.
/// Returns the largest byte offset `<= n` that lands on a UTF-8 char boundary.
/// Prevents `&str[..n]` panics when `n` points into a multi-byte character.
fn floor_char_boundary(s: &str, n: usize) -> usize {
    let n = n.min(s.len());
    (0..=n).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0)
}

/// Return `&s[..max_bytes]` rounded down to a UTF-8 char boundary.
///
/// Safe alternative to `&s[..n]` which panics when `n` falls inside a
/// multi-byte character.
pub fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    &s[..floor_char_boundary(s, max_bytes)]
}

/// Always appends `"\n… (truncated)"` when content is cut.
fn truncate_compact(text: &str, soft_max: usize, hard_max: usize) -> String {
    if text.len() <= soft_max {
        return text.to_string();
    }

    // Find the last newline within hard_max bytes — prefer to break at a line boundary.
    // floor_char_boundary ensures the slice never starts mid-char (e.g. box-drawing chars).
    let search_end = floor_char_boundary(text, hard_max);
    if let Some(nl_pos) = text[..search_end].rfind('\n') {
        return format!("{}\n… (truncated)", &text[..nl_pos]);
    }

    // No newline within hard_max — hard-truncate at a char boundary.
    format!("{}… (truncated)", &text[..search_end])
}

/// A single MCP tool exposed to the LLM.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Tool name as exposed over MCP (e.g. "find_symbol")
    fn name(&self) -> &str;

    /// Short description shown to the LLM
    fn description(&self) -> &str;

    /// JSON Schema for the input parameters
    fn input_schema(&self) -> Value;

    /// Execute the tool with the given input (already parsed from JSON)
    async fn call(&self, input: Value, ctx: &ToolContext) -> Result<Value>;

    /// Optional human-readable formatting for the tool result.
    /// When Some, call_content() emits dual-audience blocks:
    ///   1. Compact JSON (audience: assistant)
    ///   2. Formatted plain text (audience: user)
    /// Compact plain-text summary used in the buffer path alongside `@tool_*` refs.
    /// Return `None` for the generic "Result stored in @tool_xxx (N bytes)" fallback.
    fn format_compact(&self, _result: &Value) -> Option<String> {
        None
    }

    /// Human-readable display text for the MCP user-facing channel.
    ///
    /// Defaults to `format_compact()`. Override for richer display when the user
    /// channel differs from the buffer summary.
    ///
    /// Not yet called — wire in `call_content` at the TODO comment when either
    /// Claude Code issue #13600 (audience filtering) or #3174 (notifications/message)
    /// ships.
    fn format_for_user_channel(&self, result: &Value) -> Option<String> {
        self.format_compact(result)
    }

    /// Returns the JSON path to the most useful field in a buffered result.
    ///
    /// Used to build a specific, actionable hint when the tool result is stored
    /// in an `@tool_*` buffer. The default (`"$.field"`) is a generic placeholder;
    /// override to guide agents directly to the right extraction path (e.g.
    /// `"$.symbols[0].body"` for `find_symbol` with `include_body=true`).
    fn json_path_hint(&self, _val: &Value) -> String {
        "$.field".to_string()
    }

    /// Returns MCP content blocks for this tool call.
    ///
    /// Large output (> threshold) is stored in the output buffer and a compact
    /// summary is returned. Small output is returned as pretty-printed JSON.
    /// Override directly for full control over content blocks.
    async fn call_content(&self, input: Value, ctx: &ToolContext) -> Result<Vec<Content>> {
        let val = self.call(input, ctx).await?;
        let json = serde_json::to_string(&val).unwrap_or_else(|_| val.to_string());

        if exceeds_inline_limit(&json) {
            let json_len = json.len();
            let ref_id = ctx.output_buffer.store_tool(self.name(), json);
            let raw_summary = self
                .format_compact(&val)
                .unwrap_or_else(|| format!("Result stored in {} ({} bytes)", ref_id, json_len));
            let summary = truncate_compact(
                &raw_summary,
                COMPACT_SUMMARY_MAX_BYTES,
                COMPACT_SUMMARY_HARD_MAX_BYTES,
            );
            // Return a *structured* JSON response so agents consistently look for
            // the `output_id` field — the same field name `run_command` uses for its
            // `@cmd_*` refs.  The previous prose format ("summary\nFull result: @ref")
            // caused agents to either miss the ref or confuse it with the summary text.
            let jp = self.json_path_hint(&val);
            let hint = format!(
                "read_file(\"{ref_id}\", json_path=\"{jp}\") to extract a specific field, \
                 or read_file(\"{ref_id}\", start_line=N, end_line=M) to browse sections"
            );
            let buffered = serde_json::json!({
                "output_id": ref_id,
                "summary": summary,
                "hint": hint,
            });
            return Ok(vec![Content::text(
                serde_json::to_string_pretty(&buffered)
                    .unwrap_or_else(|_| format!("{{\"output_id\":\"{ref_id}\"}}")),
            )]);
        }

        // Small output — return pretty JSON to the assistant.
        // TODO(#13600/#3174): emit self.format_for_user_channel(&val) to user channel here.
        Ok(vec![Content::text(
            serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string()),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_context_has_progress_field() {
        // Compile-only test: ensures the progress field exists and has the right type.
        fn _check_progress_field_type(_ctx: &ToolContext) {
            let _p: &Option<std::sync::Arc<progress::ProgressReporter>> = &_ctx.progress;
        }
    }

    #[test]
    fn parse_bool_param_handles_all_variants() {
        use serde_json::json;
        // Native JSON booleans
        assert!(parse_bool_param(&json!(true)));
        assert!(!parse_bool_param(&json!(false)));
        // String booleans (sent by Claude Code MCP client)
        assert!(parse_bool_param(&json!("true")));
        assert!(!parse_bool_param(&json!("false")));
        // Missing / null / wrong type → false
        assert!(!parse_bool_param(&json!(null)));
        assert!(!parse_bool_param(&json!(42)));
        assert!(!parse_bool_param(&json!("yes")));
    }

    #[test]
    fn recoverable_error_stores_message() {
        let e = RecoverableError::new("path not found");
        assert_eq!(e.message, "path not found");
        assert!(e.hint.is_none());
    }

    #[test]
    fn recoverable_error_stores_hint() {
        let e = RecoverableError::with_hint("path not found", "use list_dir to explore");
        assert_eq!(e.message, "path not found");
        assert_eq!(e.hint.as_deref(), Some("use list_dir to explore"));
    }

    #[test]
    fn recoverable_error_display_shows_message() {
        let e = RecoverableError::with_hint("file missing", "check the path");
        assert_eq!(e.to_string(), "file missing");
    }

    #[test]
    fn require_u64_param_accepts_integer() {
        let input = serde_json::json!({ "n": 42 });
        assert_eq!(require_u64_param(&input, "n").unwrap(), 42);
    }

    #[test]
    fn require_u64_param_accepts_string_encoded_integer() {
        // LLMs sometimes quote integers — we must tolerate this.
        let input = serde_json::json!({ "n": "11" });
        assert_eq!(require_u64_param(&input, "n").unwrap(), 11);
    }

    #[test]
    fn require_u64_param_rejects_non_numeric_string() {
        let input = serde_json::json!({ "n": "abc" });
        assert!(require_u64_param(&input, "n").is_err());
    }

    #[test]
    fn require_u64_param_rejects_negative_string() {
        let input = serde_json::json!({ "n": "-5" });
        assert!(require_u64_param(&input, "n").is_err());
    }
    #[test]
    fn recoverable_error_downcasts_from_anyhow() {
        let e: anyhow::Error = RecoverableError::new("test error").into();
        assert!(
            e.downcast_ref::<RecoverableError>().is_some(),
            "must be recoverable via downcast"
        );
    }

    // ---- call_content auto-buffering tests ----

    async fn bare_ctx() -> ToolContext {
        ToolContext {
            agent: crate::agent::Agent::new(None).await.unwrap(),
            lsp: crate::lsp::LspManager::new_arc(),
            output_buffer: std::sync::Arc::new(output_buffer::OutputBuffer::new(20)),
            progress: None,
        }
    }

    struct EchoTool {
        result: Value,
        user_summary: Option<String>,
    }

    #[async_trait::async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo_tool"
        }
        fn description(&self) -> &str {
            "test"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({})
        }
        async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
            Ok(self.result.clone())
        }
        fn format_compact(&self, _result: &Value) -> Option<String> {
            self.user_summary.clone()
        }
    }

    #[tokio::test]
    async fn call_content_passthrough_small_output() {
        let ctx = bare_ctx().await;
        let result = serde_json::json!({"key": "value"});
        let tool = EchoTool {
            result: result.clone(),
            user_summary: None,
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        // Small output: no buffering — content should contain the JSON
        assert_eq!(content.len(), 1, "small output should not be buffered");
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
        assert!(text.contains("key"));
    }

    #[tokio::test]
    async fn call_content_small_output_ignores_format_compact() {
        // Even when format_compact returns Some, call_content must return exactly
        // 1 block with pretty JSON — the compact text is NOT injected into small outputs.
        let ctx = bare_ctx().await;
        let result = serde_json::json!({"key": "value"});
        let tool = EchoTool {
            result: result.clone(),
            user_summary: Some("compact summary".to_string()),
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        assert_eq!(
            content.len(),
            1,
            "small output must produce exactly 1 block, got: {:?}",
            content
        );
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
        assert!(
            text.contains("key"),
            "block must contain the JSON key, got: {}",
            text
        );
        assert!(
            !text.contains("compact summary"),
            "compact summary must NOT appear in small-output block, got: {}",
            text
        );
    }

    #[tokio::test]
    async fn call_content_buffers_large_output() {
        let ctx = bare_ctx().await;
        // Build a Value that serializes to >> 5_000 bytes (well above the buffer threshold)
        let big_array: Vec<Value> = (0..500)
            .map(|i| {
                serde_json::json!({
                    "index": i,
                    "name": format!("symbol_{}", i),
                    "file": "src/tools/symbol.rs"
                })
            })
            .collect();
        let result = serde_json::json!({ "symbols": big_array });
        let tool = EchoTool {
            result,
            user_summary: None,
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        // Must return exactly 1 Content item
        assert_eq!(content.len(), 1);
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
        // Contains a @tool_ ref handle
        assert!(text.contains("@tool_"), "expected @tool_ ref in: {}", text);
    }

    #[tokio::test]
    async fn call_content_uses_format_compact_in_buffer_summary() {
        let ctx = bare_ctx().await;
        let big_array: Vec<Value> = (0..500)
            .map(|i| {
                serde_json::json!({
                    "index": i,
                    "name": format!("symbol_{}", i)
                })
            })
            .collect();
        let result = serde_json::json!({ "symbols": big_array });
        let tool = EchoTool {
            result,
            user_summary: Some("Found 500 symbols".to_string()),
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
        assert!(
            text.contains("Found 500 symbols"),
            "expected summary in: {}",
            text
        );
        assert!(text.contains("@tool_"), "expected ref handle in: {}", text);
    }

    #[tokio::test]
    async fn call_content_generic_fallback_without_format_compact() {
        let ctx = bare_ctx().await;
        let big_array: Vec<Value> = (0..500)
            .map(|i| {
                serde_json::json!({
                    "index": i,
                    "name": format!("symbol_{}", i)
                })
            })
            .collect();
        let result = serde_json::json!({ "symbols": big_array });
        let tool = EchoTool {
            result,
            user_summary: None,
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
        // No format_compact → generic fallback message with byte count and ref
        assert!(
            text.contains("bytes") || text.contains("stored"),
            "expected fallback in: {}",
            text
        );
        assert!(text.contains("@tool_"), "expected ref handle in: {}", text);
    }

    // ---- threshold + summary-cap tests ----

    #[tokio::test]
    async fn call_content_buffers_at_token_threshold() {
        // Build a Value whose JSON is ~12 KB — above MAX_INLINE_TOKENS (2500 tokens ≈ 10 KB).
        let ctx = bare_ctx().await;
        let items: Vec<Value> = (0..150)
            .map(|i| {
                serde_json::json!({
                    "file": format!("src/tools/file_{}.rs", i),
                    "line": i,
                    "content": format!("let x_{} = some_function_call_{};\n", i, i)
                })
            })
            .collect();
        let result = serde_json::json!({ "matches": items, "total": items.len() });

        // Sanity: confirm the JSON exceeds the token-based threshold (~10 KB)
        let json_len = serde_json::to_string(&result).unwrap().len();
        assert!(
            json_len > MAX_INLINE_TOKENS * 4,
            "test data must exceed token threshold ({} bytes), got {} bytes",
            MAX_INLINE_TOKENS * 4,
            json_len
        );

        let tool = EchoTool {
            result,
            user_summary: Some("150 matches".to_string()),
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
        assert!(
            text.contains("@tool_"),
            "output exceeding token limit must be buffered, got: {}",
            &text[..text.len().min(200)]
        );
    }

    #[tokio::test]
    async fn call_content_does_not_buffer_under_token_limit() {
        // ~2 KB result — well under MAX_INLINE_TOKENS, must stay inline (no @tool_ ref)
        let ctx = bare_ctx().await;
        let items: Vec<Value> = (0..30)
            .map(|i| serde_json::json!({ "file": format!("src/a_{}.rs", i), "line": i }))
            .collect();
        let result = serde_json::json!({ "matches": items });

        let json_len = serde_json::to_string(&result).unwrap().len();
        assert!(
            json_len < 5_000,
            "test data must be < 5 KB, got {} bytes",
            json_len
        );

        let tool = EchoTool {
            result,
            user_summary: Some("30 matches".to_string()),
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");
        assert!(
            !text.contains("@tool_"),
            "small output must not be buffered, got: {}",
            &text[..text.len().min(200)]
        );
    }

    #[tokio::test]
    async fn call_content_caps_compact_summary() {
        // format_compact returns a 4 KB summary — must be truncated to ≤ 3 KB (hard max)
        let ctx = bare_ctx().await;
        let items: Vec<Value> = (0..200)
            .map(|i| serde_json::json!({ "idx": i, "name": "x".repeat(50) }))
            .collect();
        let result = serde_json::json!({ "items": items });

        // Summary deliberately larger than hard cap
        let big_summary = format!("{}\n", "summary line ".repeat(300)); // ~3.9 KB
        assert!(
            big_summary.len() > 3_000,
            "summary must be > hard cap for this test"
        );

        let tool = EchoTool {
            result,
            user_summary: Some(big_summary),
        };
        let content = tool
            .call_content(serde_json::json!({}), &ctx)
            .await
            .unwrap();
        let text = content[0].as_text().map(|t| t.text.as_str()).unwrap_or("");

        // Output is now a JSON object — parse it to check individual fields
        let parsed: serde_json::Value =
            serde_json::from_str(text).expect("call_content must return valid JSON");
        assert!(
            parsed["output_id"]
                .as_str()
                .unwrap_or("")
                .starts_with("@tool_"),
            "must have output_id: {parsed}"
        );
        // The summary field must be capped. truncate_compact appends "\n… (truncated)"
        // (~15 bytes) after the hard-max boundary, so allow a small suffix slack.
        let summary = parsed["summary"].as_str().unwrap_or("");
        assert!(
            summary.len() <= COMPACT_SUMMARY_HARD_MAX_BYTES + 20,
            "summary must be capped; got {} bytes",
            summary.len()
        );
        assert!(
            summary.contains("truncated"),
            "must include truncation note: {}",
            &summary[..summary.len().min(200)]
        );
        // hint must be present and reference the output_id
        let hint = parsed["hint"].as_str().unwrap_or("");
        assert!(
            hint.contains("@tool_"),
            "hint must reference the output_id: {hint}"
        );
    }

    // ---- truncate_compact tests ----

    #[test]
    fn truncate_compact_under_soft_cap_returns_verbatim() {
        let text = "line1\nline2\nline3";
        assert_eq!(super::truncate_compact(text, 2_000, 3_000), text);
    }

    #[test]
    fn truncate_compact_exact_soft_cap_returns_verbatim() {
        // Exactly at the soft cap — no truncation
        let text = "x".repeat(2_000);
        assert_eq!(super::truncate_compact(&text, 2_000, 3_000), text);
    }

    #[test]
    fn truncate_compact_at_line_boundary() {
        // Line 1 is 1,800 bytes; line 2 is 600 bytes → total 2,401 (> soft_max=2_000)
        // Last '\n' is at byte 1,800, which is ≤ hard_max=3_000 → truncate there
        let line1 = "a".repeat(1_800);
        let line2 = "b".repeat(600);
        let text = format!("{}\n{}", line1, line2);

        let result = super::truncate_compact(&text, 2_000, 3_000);

        assert!(result.starts_with(&line1), "should keep line1 intact");
        assert!(!result.contains(&line2), "should drop line2");
        assert!(
            result.contains("… (truncated)"),
            "should append truncation note"
        );
    }

    #[test]
    fn truncate_compact_no_newlines_uses_hard_cap() {
        // Single 5,000-byte line — no '\n' → hard-cap at 3,000 bytes
        let text = "x".repeat(5_000);
        let result = super::truncate_compact(&text, 2_000, 3_000);

        assert!(
            result.starts_with(&"x".repeat(3_000)),
            "should keep first 3,000 bytes"
        );
        assert!(result.ends_with("… (truncated)"), "should append note");
        // Sanity check: result is not longer than hard_max + note
        assert!(result.len() <= 3_000 + 20);
    }

    #[test]
    fn truncate_compact_preserves_text_exactly_at_hard_cap() {
        // Text is 2,500 bytes (> soft) with a single newline at position 2,400.
        // Line boundary (2,400) is between soft (2,000) and hard (3,000) — use it.
        let line1 = "a".repeat(2_400);
        let line2 = "b".repeat(99);
        let text = format!("{}\n{}", line1, line2);

        let result = super::truncate_compact(&text, 2_000, 3_000);

        assert!(result.starts_with(&line1), "should keep line1");
        assert!(!result.contains(&line2), "should not include line2");
        assert!(result.contains("… (truncated)"));
    }

    #[test]
    fn truncate_compact_unicode_does_not_panic() {
        // Regression test for the read_file crash on docs/ARCHITECTURE.md.
        // Box-drawing chars (─, │, ┌, etc.) are 3 bytes each in UTF-8.
        // A hard_max that lands mid-char must NOT cause a panic.
        let box_line: String = std::iter::repeat('─').take(700).collect(); // 2100 bytes
        let prefix = "x".repeat(100);
        let text = format!("{}\n{}", prefix, box_line); // >2000 bytes, no newline after 101

        // Must not panic regardless of where hard_max falls inside multi-byte chars.
        let result = super::truncate_compact(&text, 2_000, 3_000);
        assert!(result.contains("… (truncated)"), "should be truncated");
        // Result must be valid UTF-8 (no mid-char slices)
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn floor_char_boundary_lands_on_boundary() {
        let s = "ab─cd"; // 'a'=1, 'b'=1, '─'=3 bytes (E2 94 80), 'c'=1, 'd'=1
                         // bytes: 0='a', 1='b', 2-4='─', 5='c', 6='d'
        assert_eq!(super::floor_char_boundary(s, 0), 0);
        assert_eq!(super::floor_char_boundary(s, 2), 2); // before '─'
        assert_eq!(super::floor_char_boundary(s, 3), 2); // inside '─' → back to 2
        assert_eq!(super::floor_char_boundary(s, 4), 2); // inside '─' → back to 2
        assert_eq!(super::floor_char_boundary(s, 5), 5); // after '─'
        assert_eq!(super::floor_char_boundary(s, 6), 6);
        assert_eq!(super::floor_char_boundary(s, 100), s.len()); // clamp to len
    }

    #[test]
    fn safe_truncate_avoids_mid_char_split() {
        let s = "ab\u{2500}cd"; // 'a'=1, 'b'=1, '\u{2500}'=3 bytes, 'c'=1, 'd'=1
        assert_eq!(super::safe_truncate(s, 0), "");
        assert_eq!(super::safe_truncate(s, 2), "ab");
        assert_eq!(super::safe_truncate(s, 3), "ab"); // inside 3-byte char → round down
        assert_eq!(super::safe_truncate(s, 4), "ab"); // still inside
        assert_eq!(super::safe_truncate(s, 5), "ab\u{2500}");
        assert_eq!(super::safe_truncate(s, 100), s); // clamp to len
    }
}
