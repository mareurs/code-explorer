//! Semantic search tools backed by the embedding index.

use super::{Tool, ToolContext};
use serde_json::{json, Value};

pub struct SemanticSearch;
pub struct IndexProject;
pub struct IndexStatus;

#[async_trait::async_trait]
impl Tool for SemanticSearch {
    fn name(&self) -> &str {
        "semantic_search"
    }
    fn description(&self) -> &str {
        "Find code by natural language description or code snippet. \
         Returns ranked chunks with file path, line range, and similarity score."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language description or code snippet to search for"
                },
                "limit": { "type": "integer", "default": 10 },
                "detail_level": {
                    "type": "string",
                    "description": "Output detail: omit for compact preview (default), 'full' for complete chunk content"
                },
                "offset": {
                    "type": "integer",
                    "description": "Skip this many results (focused mode pagination)"
                },
                "scope": {
                    "type": "string",
                    "description": "Search scope: 'project' (default), 'libraries', 'all', or 'lib:<name>' for a specific library"
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        use super::output::OutputGuard;

        let query = input["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'query' parameter"))?;
        let limit = input["limit"].as_u64().unwrap_or(10) as usize;
        let guard = OutputGuard::from_input(&input);

        let (root, model) = {
            let inner = ctx.agent.inner.read().await;
            let p = inner
                .active_project
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No active project. Use activate_project first."))?;
            (p.root.clone(), p.config.embeddings.model.clone())
        };

        let scope = crate::library::scope::Scope::parse(input["scope"].as_str());
        let source_filter = match &scope {
            crate::library::scope::Scope::Project => Some("project".to_string()),
            crate::library::scope::Scope::Library(name) => Some(format!("lib:{}", name)),
            crate::library::scope::Scope::Libraries => Some("libraries".to_string()),
            crate::library::scope::Scope::All => None,
        };

        // Async: cached embedder + HTTP embed
        let embedder = ctx.agent.get_or_create_embedder(&model).await?;
        let query_embedding = crate::embed::embed_one(embedder.as_ref(), query).await?;

        // Sync SQLite off async runtime
        let root2 = root.clone();
        let (results, staleness) = tokio::task::spawn_blocking(move || {
            let conn = crate::embed::index::open_db(&root2)?;
            let results = crate::embed::index::search_scoped(
                &conn,
                &query_embedding,
                limit,
                source_filter.as_deref(),
            )?;
            let staleness = crate::embed::index::check_index_staleness(&conn, &root2).ok();
            anyhow::Ok((results, staleness))
        })
        .await??;

        // Transform results based on mode
        let result_items: Vec<Value> = results
            .iter()
            .map(|r| {
                let content_field = if guard.should_include_body() {
                    // Focused mode: full content
                    r.content.clone()
                } else {
                    // Exploring mode: preview (first 150 chars)
                    let preview_len = 150.min(r.content.len());
                    let mut preview = r.content[..preview_len].to_string();
                    if r.content.len() > preview_len {
                        preview.push_str("...");
                    }
                    preview
                };
                json!({
                    "file_path": r.file_path,
                    "language": r.language,
                    "content": content_field,
                    "start_line": r.start_line,
                    "end_line": r.end_line,
                    "score": r.score,
                    "source": r.source,
                })
            })
            .collect();

        // Apply pagination/capping
        let (result_items, overflow) = guard.cap_items(
            result_items,
            "Use detail_level='full' with offset for pagination",
        );
        let total = overflow.as_ref().map_or(result_items.len(), |o| o.total);
        let mut result = json!({ "results": result_items, "total": total });
        if let Some(ov) = overflow {
            result["overflow"] = OutputGuard::overflow_json(&ov);
        }
        // Check index freshness
        if let Some(staleness) = staleness {
            if staleness.stale {
                result["stale"] = json!(true);
                result["behind_commits"] = json!(staleness.behind_commits);
                result["hint"] = json!("Index is behind HEAD. Run index_project to update.");
            }
        }
        Ok(result)
    }
}

#[async_trait::async_trait]
impl Tool for IndexProject {
    fn name(&self) -> &str {
        "index_project"
    }
    fn description(&self) -> &str {
        "Build or incrementally update the semantic search index for the active project."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "force": { "type": "boolean", "default": false,
                    "description": "Force full reindex, ignoring cached file hashes" }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let force = input["force"].as_bool().unwrap_or(false);
        let root = ctx.agent.require_project_root().await?;

        let report = crate::embed::index::build_index(&root, force).await?;

        let conn = crate::embed::index::open_db(&root)?;
        let stats = crate::embed::index::index_stats(&conn)?;

        // Top 5 most-drifted files
        let drift_summary: Vec<Value> = report
            .drift
            .iter()
            .filter(|d| d.avg_drift > 0.05)
            .take(5)
            .map(|d| {
                json!({
                    "file": d.file_path,
                    "avg_drift": format!("{:.2}", d.avg_drift),
                    "max_drift": format!("{:.2}", d.max_drift),
                    "added": d.chunks_added,
                    "removed": d.chunks_removed,
                })
            })
            .collect();

        let mut result = json!({
            "status": "ok",
            "files_indexed": report.indexed,
            "files_deleted": report.deleted,
            "detail": report.skipped_msg,
            "total_files": stats.file_count,
            "total_chunks": stats.chunk_count,
        });

        if !drift_summary.is_empty() {
            result["drift_summary"] = json!(drift_summary);
        }

        Ok(result)
    }
}

#[async_trait::async_trait]
impl Tool for IndexStatus {
    fn name(&self) -> &str {
        "index_status"
    }
    fn description(&self) -> &str {
        "Show index stats: file count, chunk count, model, last update. \
         Optionally query semantic drift scores when threshold or path is provided."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "threshold": {
                    "type": "number",
                    "description": "Minimum avg_drift to include (default: 0.1). Range 0.0-1.0. When provided, includes drift data in response."
                },
                "path": {
                    "type": "string",
                    "description": "Glob pattern to filter drift files (e.g. 'src/tools/%'). Uses SQL LIKE syntax."
                },
                "detail_level": {
                    "type": "string",
                    "enum": ["exploring", "full"],
                    "description": "Output detail for drift: 'exploring' (default) shows scores only, 'full' includes most-drifted chunk content."
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        let (root, model, drift_enabled) = {
            let inner = ctx.agent.inner.read().await;
            let p = inner
                .active_project
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No active project. Use activate_project first."))?;
            (
                p.root.clone(),
                p.config.embeddings.model.clone(),
                p.config.embeddings.drift_detection_enabled,
            )
        };

        let db_path = crate::embed::index::db_path(&root);
        if !db_path.exists() {
            return Ok(json!({
                "indexed": false,
                "message": "No index found. Run index_project first.",
            }));
        }

        // Sync SQLite off async runtime
        let db_path_str = db_path.display().to_string();
        let root2 = root.clone();
        let (stats, by_source, staleness, last_commit) = tokio::task::spawn_blocking(move || {
            let conn = crate::embed::index::open_db(&root2)?;
            let stats = crate::embed::index::index_stats(&conn)?;
            let by_source = crate::embed::index::index_stats_by_source(&conn)?;
            let staleness = crate::embed::index::check_index_staleness(&conn, &root2).ok();
            let last_commit = crate::embed::index::get_last_indexed_commit(&conn)
                .ok()
                .flatten();
            anyhow::Ok((stats, by_source, staleness, last_commit))
        })
        .await??;

        let by_source_json: serde_json::Map<String, Value> = by_source
            .iter()
            .map(|(source, ss)| {
                (
                    source.clone(),
                    json!({ "files": ss.file_count, "chunks": ss.chunk_count }),
                )
            })
            .collect();

        // Build base response
        let mut result = json!({
            "indexed": true,
            "configured_model": model,
            "indexed_with_model": stats.model,
            "file_count": stats.file_count,
            "chunk_count": stats.chunk_count,
            "embedding_count": stats.embedding_count,
            "db_path": db_path_str,
            "by_source": by_source_json,
        });

        // Add staleness info
        if let Some(staleness) = staleness {
            result["stale"] = json!(staleness.stale);
            if staleness.stale {
                result["behind_commits"] = json!(staleness.behind_commits);
            }
            if let Some(commit) = last_commit {
                result["last_indexed_commit"] = json!(commit);
            }
        }

        // Include drift data when threshold or path is provided
        let wants_drift = input.get("threshold").is_some() || input.get("path").is_some();
        if wants_drift {
            use super::output::OutputGuard;

            if !drift_enabled {
                result["drift"] = json!({
                    "status": "disabled",
                    "hint": "Drift detection is opted out. Re-enable it in .code-explorer/project.toml:\n[embeddings]\ndrift_detection_enabled = true"
                });
            } else {
                let threshold = input["threshold"].as_f64().map(|v| v as f32).unwrap_or(0.1);
                let path_filter = input["path"].as_str().map(|s| s.to_string());
                let guard = OutputGuard::from_input(&input);

                // Sync SQLite off async runtime
                let root3 = root.clone();
                let rows = tokio::task::spawn_blocking(move || {
                    let conn = crate::embed::index::open_db(&root3)?;
                    crate::embed::index::query_drift_report(
                        &conn,
                        Some(threshold),
                        path_filter.as_deref(),
                    )
                })
                .await??;

                let items: Vec<Value> = rows
                    .iter()
                    .map(|r| {
                        let mut obj = json!({
                            "file_path": r.file_path,
                            "avg_drift": r.avg_drift,
                            "max_drift": r.max_drift,
                            "chunks_added": r.chunks_added,
                            "chunks_removed": r.chunks_removed,
                        });
                        if guard.should_include_body() {
                            if let Some(chunk) = &r.max_drift_chunk {
                                obj["max_drift_chunk"] = json!(chunk);
                            }
                        }
                        obj
                    })
                    .collect();

                let (items, overflow) =
                    guard.cap_items(items, "Use detail_level='full' with offset for pagination");
                let total = overflow.as_ref().map_or(items.len(), |o| o.total);
                let mut drift_result = json!({ "results": items, "total": total });
                if let Some(ov) = overflow {
                    drift_result["overflow"] = OutputGuard::overflow_json(&ov);
                }
                result["drift"] = drift_result;
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::embed::index;
    use crate::lsp::LspManager;
    use std::sync::Arc;
    use tempfile::tempdir;

    async fn project_ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        (
            dir,
            ToolContext {
                agent,
                lsp: Arc::new(LspManager::new()),
            },
        )
    }

    async fn drift_enabled_ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        let ce_dir = dir.path().join(".code-explorer");
        std::fs::create_dir_all(&ce_dir).unwrap();
        std::fs::write(
            ce_dir.join("project.toml"),
            "[project]\nname = \"test\"\n\n[embeddings]\ndrift_detection_enabled = true\n",
        )
        .unwrap();
        // Create an empty DB so index_status doesn't early-return "no index"
        let _conn = index::open_db(dir.path()).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        (
            dir,
            ToolContext {
                agent,
                lsp: Arc::new(LspManager::new()),
            },
        )
    }

    #[tokio::test]
    async fn index_status_no_index() {
        let (_dir, ctx) = project_ctx().await;
        let result = IndexStatus.call(json!({}), &ctx).await.unwrap();
        assert_eq!(result["indexed"], false);
    }

    #[tokio::test]
    async fn index_status_with_data() {
        let (dir, ctx) = project_ctx().await;
        // Create the DB and insert some data
        let conn = index::open_db(dir.path()).unwrap();
        let chunk = crate::embed::schema::CodeChunk {
            id: None,
            file_path: "test.rs".to_string(),
            language: "rust".to_string(),
            content: "fn test() {}".to_string(),
            start_line: 1,
            end_line: 1,
            file_hash: "abc".to_string(),
            source: "project".to_string(),
        };
        index::insert_chunk(&conn, &chunk, &[0.1, 0.2, 0.3]).unwrap();
        index::upsert_file_hash(&conn, "test.rs", "abc", None).unwrap();
        drop(conn);

        let result = IndexStatus.call(json!({}), &ctx).await.unwrap();
        assert_eq!(result["indexed"], true);
        assert_eq!(result["file_count"], 1);
        assert_eq!(result["chunk_count"], 1);
        assert_eq!(result["embedding_count"], 1);
    }

    #[tokio::test]
    async fn tools_error_without_project() {
        let ctx = ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: Arc::new(LspManager::new()),
        };
        assert!(SemanticSearch
            .call(json!({ "query": "test" }), &ctx)
            .await
            .is_err());
        assert!(IndexProject.call(json!({}), &ctx).await.is_err());
        assert!(IndexStatus.call(json!({}), &ctx).await.is_err());
    }

    #[tokio::test]
    async fn index_stats_function() {
        let dir = tempdir().unwrap();
        let conn = index::open_db(dir.path()).unwrap();
        let stats = index::index_stats(&conn).unwrap();
        assert_eq!(stats.file_count, 0);
        assert_eq!(stats.chunk_count, 0);
        assert_eq!(stats.embedding_count, 0);
    }

    #[tokio::test]
    async fn semantic_search_schema_has_detail_level() {
        let schema = SemanticSearch.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("detail_level"),
            "should accept detail_level parameter"
        );
        assert!(
            props.contains_key("offset"),
            "should accept offset parameter"
        );
    }

    #[test]
    fn preview_truncation_works() {
        let long_content = "x".repeat(500);
        let preview_len = 150.min(long_content.len());
        let mut preview = long_content[..preview_len].to_string();
        if long_content.len() > preview_len {
            preview.push_str("...");
        }
        assert_eq!(preview.len(), 153); // 150 + "..."
        assert!(preview.ends_with("..."));

        let short_content = "short";
        let preview_len2 = 150.min(short_content.len());
        let preview2 = short_content[..preview_len2].to_string();
        assert_eq!(preview2, "short"); // no truncation for short content
    }

    #[tokio::test]
    async fn semantic_search_schema_has_scope() {
        let schema = SemanticSearch.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("scope"), "should accept scope parameter");
    }

    #[tokio::test]
    async fn index_status_includes_by_source() {
        let (dir, ctx) = project_ctx().await;
        // Create the DB and insert some data with different sources
        let conn = index::open_db(dir.path()).unwrap();
        let chunk = crate::embed::schema::CodeChunk {
            id: None,
            file_path: "test.rs".to_string(),
            language: "rust".to_string(),
            content: "fn test() {}".to_string(),
            start_line: 1,
            end_line: 1,
            file_hash: "abc".to_string(),
            source: "project".to_string(),
        };
        index::insert_chunk(&conn, &chunk, &[0.1, 0.2, 0.3]).unwrap();
        index::upsert_file_hash(&conn, "test.rs", "abc", None).unwrap();
        drop(conn);

        let result = IndexStatus.call(json!({}), &ctx).await.unwrap();
        assert_eq!(result["indexed"], true);
        assert!(
            result["by_source"].is_object(),
            "should include by_source breakdown"
        );
        assert!(
            result["by_source"]["project"].is_object(),
            "should have project source entry"
        );
    }

    #[tokio::test]
    async fn semantic_search_staleness_detection() {
        let (dir, _ctx) = project_ctx().await;

        // Init git repo with a commit
        let repo = git2::Repository::init(dir.path()).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@test.com").unwrap();
        let mut git_index = repo.index().unwrap();
        let tree_oid = git_index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        // Create DB without last_indexed_commit → should be stale
        let conn = crate::embed::index::open_db(dir.path()).unwrap();
        let staleness = crate::embed::index::check_index_staleness(&conn, dir.path()).unwrap();
        assert!(staleness.stale);
    }

    #[tokio::test]
    async fn index_status_shows_staleness() {
        let (dir, ctx) = project_ctx().await;

        // Init git repo with a commit
        let repo = git2::Repository::init(dir.path()).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@test.com").unwrap();
        let mut git_index = repo.index().unwrap();
        let tree_oid = git_index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        // Create DB without last_indexed_commit
        let conn = crate::embed::index::open_db(dir.path()).unwrap();
        crate::embed::index::upsert_file_hash(&conn, "a.rs", "abc", None).unwrap();
        drop(conn);

        let result = IndexStatus.call(json!({}), &ctx).await.unwrap();
        assert_eq!(result["indexed"], true);
        assert_eq!(result["stale"], true);
    }

    #[tokio::test]
    async fn drift_enabled_by_default() {
        // drift_detection_enabled defaults to true — drift query should NOT
        // return the "disabled" status when no explicit config is present.
        let (_dir, ctx) = project_ctx().await;
        let result = IndexStatus
            .call(json!({"threshold": 0.1}), &ctx)
            .await
            .unwrap();
        assert_ne!(
            result["drift"]["status"], "disabled",
            "drift should be enabled by default, got: {:?}",
            result["drift"]
        );
    }

    #[tokio::test]
    async fn drift_disabled_when_opted_out() {
        // Explicit opt-out via project.toml should return "disabled" in drift key.
        let dir = tempdir().unwrap();
        let ce_dir = dir.path().join(".code-explorer");
        std::fs::create_dir_all(&ce_dir).unwrap();
        std::fs::write(
            ce_dir.join("project.toml"),
            "[project]\nname = \"test\"\n\n[embeddings]\ndrift_detection_enabled = false\n",
        )
        .unwrap();
        // Create an empty DB so index_status doesn't early-return "no index"
        let _conn = index::open_db(dir.path()).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let ctx = ToolContext {
            agent,
            lsp: Arc::new(LspManager::new()),
        };
        let result = IndexStatus
            .call(json!({"threshold": 0.1}), &ctx)
            .await
            .unwrap();
        assert_eq!(result["drift"]["status"], "disabled");
        assert!(result["drift"]["hint"]
            .as_str()
            .unwrap()
            .contains("drift_detection_enabled"));
    }

    #[tokio::test]
    async fn drift_returns_empty_without_data() {
        let (_dir, ctx) = drift_enabled_ctx().await;
        let result = IndexStatus
            .call(json!({"threshold": 0.1}), &ctx)
            .await
            .unwrap();
        assert_eq!(result["drift"]["results"], json!([]));
    }

    #[tokio::test]
    async fn drift_returns_rows() {
        let (_dir, ctx) = drift_enabled_ctx().await;
        let root = {
            let inner = ctx.agent.inner.read().await;
            inner.active_project.as_ref().unwrap().root.clone()
        };
        let conn = crate::embed::index::open_db(&root).unwrap();
        crate::embed::index::upsert_drift_report(&conn, "a.rs", 0.5, 0.8, Some("fn x()"), 1, 0)
            .unwrap();
        crate::embed::index::upsert_drift_report(&conn, "b.rs", 0.02, 0.05, None, 0, 0).unwrap();
        drop(conn);

        // Default threshold 0.1 should filter out b.rs
        let result = IndexStatus
            .call(json!({"threshold": 0.1}), &ctx)
            .await
            .unwrap();
        let results = result["drift"]["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["file_path"], "a.rs");
    }

    #[tokio::test]
    async fn drift_respects_threshold() {
        let (_dir, ctx) = drift_enabled_ctx().await;
        let root = {
            let inner = ctx.agent.inner.read().await;
            inner.active_project.as_ref().unwrap().root.clone()
        };
        let conn = crate::embed::index::open_db(&root).unwrap();
        crate::embed::index::upsert_drift_report(&conn, "a.rs", 0.5, 0.8, None, 1, 0).unwrap();
        drop(conn);

        let result = IndexStatus
            .call(json!({"threshold": 0.6}), &ctx)
            .await
            .unwrap();
        let results = result["drift"]["results"].as_array().unwrap();
        assert!(results.is_empty()); // avg_drift 0.5 < threshold 0.6
    }

    #[tokio::test]
    async fn concurrent_semantic_search_does_not_deadlock() {
        let (_dir, ctx) = project_ctx().await;
        let ctx = std::sync::Arc::new(ctx);
        let input = json!({"query": "test"});

        // Run two searches concurrently — neither should hang.
        // They'll likely error (no embedder available in test), but that's fine.
        // The point is they complete without deadlocking.
        let ctx1 = ctx.clone();
        let input1 = input.clone();
        let ctx2 = ctx.clone();
        let input2 = input.clone();

        let (r1, r2) = tokio::join!(
            async move { SemanticSearch.call(input1, &ctx1).await },
            async move { SemanticSearch.call(input2, &ctx2).await },
        );

        // Both should complete (either Ok or Err, but not hang)
        // We expect errors since there's no embedder in test environment
        let _ = r1;
        let _ = r2;
    }
}
