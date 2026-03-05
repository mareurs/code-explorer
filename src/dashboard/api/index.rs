use super::super::routes::DashboardState;
use crate::embed::index as embed_index;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

pub async fn get_index(State(state): State<DashboardState>) -> Json<Value> {
    let db_path = state.project_root.join(".codescout").join("embeddings.db");
    if !db_path.exists() {
        return Json(json!({
            "available": false,
            "reason": "No semantic index. Run `codescout index` to build one."
        }));
    }

    let conn = match embed_index::open_db(&state.project_root) {
        Ok(c) => c,
        Err(e) => {
            return Json(json!({
                "available": false,
                "reason": format!("Failed to open index DB: {}", e)
            }));
        }
    };

    let stats = embed_index::index_stats(&conn).unwrap_or(embed_index::IndexStats {
        file_count: 0,
        chunk_count: 0,
        embedding_count: 0,
        model: None,
        indexed_at: None,
    });

    let staleness = embed_index::check_index_staleness(&conn, &state.project_root).unwrap_or(
        embed_index::Staleness {
            stale: true,
            behind_commits: 0,
        },
    );

    Json(json!({
        "available": true,
        "file_count": stats.file_count,
        "chunk_count": stats.chunk_count,
        "embedding_count": stats.embedding_count,
        "model": stats.model,
        "stale": staleness.stale,
        "behind_commits": staleness.behind_commits,
    }))
}

#[derive(Deserialize)]
pub struct DriftParams {
    pub threshold: Option<f32>,
}

pub async fn get_drift(
    State(state): State<DashboardState>,
    Query(params): Query<DriftParams>,
) -> Json<Value> {
    let db_path = state.project_root.join(".codescout").join("embeddings.db");
    if !db_path.exists() {
        return Json(json!({ "available": false, "files": [] }));
    }

    let conn = match embed_index::open_db(&state.project_root) {
        Ok(c) => c,
        Err(_) => return Json(json!({ "available": false, "files": [] })),
    };

    let threshold = params.threshold.unwrap_or(0.1);
    let rows = embed_index::query_drift_report(&conn, Some(threshold), None).unwrap_or_default();

    let files: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "path": r.file_path,
                "avg_drift": r.avg_drift,
                "max_drift": r.max_drift,
                "chunks_added": r.chunks_added,
                "chunks_removed": r.chunks_removed,
            })
        })
        .collect();

    Json(json!({
        "available": true,
        "threshold": threshold,
        "files": files,
    }))
}
