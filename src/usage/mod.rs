pub mod db;

use crate::agent::Agent;
use anyhow::Result;
use rmcp::model::Content;
use serde_json::Value;
use std::time::Instant;

pub struct UsageRecorder {
    agent: Agent,
}

impl UsageRecorder {
    pub fn new(agent: Agent) -> Self {
        Self { agent }
    }

    pub async fn record_content<F, Fut>(&self, tool_name: &str, f: F) -> Result<Vec<Content>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Content>>>,
    {
        let start = Instant::now();
        let result = f().await;
        let latency_ms = start.elapsed().as_millis() as i64;
        // Best-effort — never let recording fail the tool call
        let _ = self.write_content(tool_name, latency_ms, &result).await;
        result
    }

    async fn write_content(
        &self,
        tool_name: &str,
        latency_ms: i64,
        result: &Result<Vec<Content>>,
    ) -> Result<()> {
        let project_root = self.agent.with_project(|p| Ok(p.root.clone())).await?;
        let conn = db::open_db(&project_root)?;
        let (outcome, overflowed, error_msg) = classify_content_result(result);
        db::write_record(
            &conn,
            tool_name,
            latency_ms,
            outcome,
            overflowed,
            error_msg.as_deref(),
        )?;
        Ok(())
    }
}

fn classify_content_result(result: &Result<Vec<Content>>) -> (&'static str, bool, Option<String>) {
    match result {
        Err(e) => ("error", false, Some(e.to_string())),
        Ok(blocks) => {
            // Parse the text of the first content block as JSON and inspect it
            // for the same "error" / "overflow" sentinel keys that classify_result uses.
            let text = blocks
                .first()
                .and_then(|c| c.as_text())
                .map(|t| t.text.as_str())
                .unwrap_or("");
            if let Ok(v) = serde_json::from_str::<Value>(text) {
                if let Some(msg) = v.get("error").and_then(Value::as_str) {
                    return ("recoverable_error", false, Some(msg.to_string()));
                }
                if v.get("overflow").is_some() {
                    return ("success", true, None);
                }
            }
            ("success", false, None)
        }
    }
}

#[cfg(test)]
mod content_tests {
    use super::*;
    use rmcp::model::Content;

    #[test]
    fn classify_content_error_result() {
        let r: anyhow::Result<Vec<Content>> = Err(anyhow::anyhow!("boom"));
        let (outcome, overflowed, msg) = classify_content_result(&r);
        assert_eq!(outcome, "error");
        assert!(!overflowed);
        assert_eq!(msg.as_deref(), Some("boom"));
    }

    #[test]
    fn classify_content_recoverable_error() {
        let text = serde_json::json!({"error": "path not found"}).to_string();
        let r: anyhow::Result<Vec<Content>> = Ok(vec![Content::text(text)]);
        let (outcome, overflowed, msg) = classify_content_result(&r);
        assert_eq!(outcome, "recoverable_error");
        assert!(!overflowed);
        assert_eq!(msg.as_deref(), Some("path not found"));
    }

    #[test]
    fn classify_content_overflow() {
        let text = serde_json::json!({"symbols": [], "overflow": {"shown": 200, "total": 500}})
            .to_string();
        let r: anyhow::Result<Vec<Content>> = Ok(vec![Content::text(text)]);
        let (outcome, overflowed, _) = classify_content_result(&r);
        assert_eq!(outcome, "success");
        assert!(overflowed);
    }

    #[test]
    fn classify_content_clean_success() {
        let r: anyhow::Result<Vec<Content>> = Ok(vec![Content::text("plain text output")]);
        let (outcome, overflowed, msg) = classify_content_result(&r);
        assert_eq!(outcome, "success");
        assert!(!overflowed);
        assert!(msg.is_none());
    }

    #[test]
    fn classify_content_empty_blocks() {
        let r: anyhow::Result<Vec<Content>> = Ok(vec![]);
        let (outcome, overflowed, msg) = classify_content_result(&r);
        assert_eq!(outcome, "success");
        assert!(!overflowed);
        assert!(msg.is_none());
    }
}
