//! Configuration and project management tools.

use anyhow::anyhow;
use serde_json::{json, Value};
use super::{Tool, ToolContext};

pub struct ActivateProject;
pub struct GetCurrentConfig;

#[async_trait::async_trait]
impl Tool for ActivateProject {
    fn name(&self) -> &str { "activate_project" }
    fn description(&self) -> &str {
        "Switch the active project to the given path. All subsequent tool calls \
         operate relative to this project root."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string", "description": "Absolute path to the project root" }
            }
        })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("activate_project: not yet wired to Agent::activate"))
    }
}

#[async_trait::async_trait]
impl Tool for GetCurrentConfig {
    fn name(&self) -> &str { "get_current_config" }
    fn description(&self) -> &str { "Display the active project config and server settings." }
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> anyhow::Result<Value> {
        Err(anyhow!("get_current_config: not yet wired to Agent"))
    }
}
