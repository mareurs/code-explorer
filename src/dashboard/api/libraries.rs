use super::super::routes::DashboardState;
use crate::library::registry::LibraryRegistry;
use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

pub async fn get_libraries(State(state): State<DashboardState>) -> Json<Value> {
    let registry_path = state.project_root.join(".codescout").join("libraries.json");
    let registry = LibraryRegistry::load(&registry_path).unwrap_or_else(|_| LibraryRegistry::new());

    let libs: Vec<Value> = registry
        .all()
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "path": e.path.display().to_string(),
                "language": e.language,
                "indexed": e.indexed,
                "version": e.version,
            })
        })
        .collect();

    Json(json!({ "libraries": libs }))
}
