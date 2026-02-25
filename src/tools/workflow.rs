//! Workflow and onboarding tools.

use anyhow::anyhow;
use serde_json::{json, Value};
use super::{Tool, ToolContext};

pub struct Onboarding;
pub struct CheckOnboardingPerformed;
pub struct ExecuteShellCommand;

#[async_trait::async_trait]
impl Tool for Onboarding {
    fn name(&self) -> &str { "onboarding" }
    fn description(&self) -> &str {
        "Perform initial project discovery: detect languages, list top-level structure, \
         summarize key files. Stores the result as a memory entry."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("onboarding: not yet implemented"))
    }
}

#[async_trait::async_trait]
impl Tool for CheckOnboardingPerformed {
    fn name(&self) -> &str { "check_onboarding_performed" }
    fn description(&self) -> &str {
        "Check whether project onboarding has been performed for the active project."
    }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("check_onboarding_performed: not yet wired to MemoryStore"))
    }
}

#[async_trait::async_trait]
impl Tool for ExecuteShellCommand {
    fn name(&self) -> &str { "execute_shell_command" }
    fn description(&self) -> &str {
        "Run a shell command in the active project root and return stdout/stderr."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": { "type": "string" },
                "timeout_secs": { "type": "integer", "default": 30 }
            }
        })
    }
    async fn call(&self, input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        let command = input["command"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'command' parameter"))?;
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await?;
        Ok(json!({
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
            "exit_code": output.status.code()
        }))
    }
}
