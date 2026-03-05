use super::super::routes::DashboardState;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
pub struct ErrorParams {
    pub limit: Option<i64>,
}

pub async fn get_errors(
    State(state): State<DashboardState>,
    Query(params): Query<ErrorParams>,
) -> Json<Value> {
    let db_path = state.project_root.join(".codescout").join("usage.db");
    if !db_path.exists() {
        return Json(json!({ "available": false, "errors": [] }));
    }

    let conn = match crate::usage::db::open_db(&state.project_root) {
        Ok(c) => c,
        Err(_) => return Json(json!({ "available": false, "errors": [] })),
    };

    let limit = params.limit.unwrap_or(20);
    let errors = crate::usage::db::recent_errors(&conn, limit).unwrap_or_default();
    Json(json!({
        "available": true,
        "errors": errors,
    }))
}
