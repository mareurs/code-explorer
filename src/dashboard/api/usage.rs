use super::super::routes::DashboardState;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct UsageParams {
    pub window: Option<String>,
}

pub async fn get_usage(
    State(state): State<DashboardState>,
    Query(params): Query<UsageParams>,
) -> Json<Value> {
    let db_path = state.project_root.join(".codescout").join("usage.db");
    if !db_path.exists() {
        return Json(json!({
            "available": false,
            "reason": "No usage data. Tool statistics are recorded when the MCP server runs."
        }));
    }

    let conn = match crate::usage::db::open_db(&state.project_root) {
        Ok(c) => c,
        Err(e) => {
            return Json(json!({
                "available": false,
                "reason": format!("Failed to open usage DB: {}", e)
            }));
        }
    };

    let window = params.window.as_deref().unwrap_or("30d");
    match crate::usage::db::query_stats(&conn, window) {
        Ok(stats) => {
            let mut val = serde_json::to_value(stats).unwrap_or_default();
            val["available"] = json!(true);
            Json(val)
        }
        Err(e) => Json(json!({
            "available": false,
            "reason": format!("Query failed: {}", e)
        })),
    }
}
