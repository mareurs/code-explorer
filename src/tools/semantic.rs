//! Semantic search tools backed by the embedding index.

use super::format::format_overflow;
use super::{parse_bool_param, Tool, ToolContext};
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
                },
                "include_memories": {
                    "type": "boolean",
                    "default": false,
                    "description": "If true, also search semantic memories and include them in results tagged with source='memory'."
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        use super::output::OutputGuard;

        let query = super::require_str_param(&input, "query")?;
        let limit = input["limit"].as_u64().unwrap_or(10) as usize;
        let include_memories = parse_bool_param(&input["include_memories"]);
        let guard = OutputGuard::from_input(&input);

        let (root, model, library_registry) = {
            let inner = ctx.agent.inner.read().await;
            let p = inner.active_project.as_ref().ok_or_else(|| {
                super::RecoverableError::with_hint(
                    "No active project. Use activate_project first.",
                    "Call activate_project(\"/path/to/project\") to set the active project.",
                )
            })?;
            (
                p.root.clone(),
                p.config.embeddings.model.clone(),
                p.library_registry.clone(),
            )
        };

        let scope = crate::library::scope::Scope::parse(input["scope"].as_str());

        // Async: cached embedder + HTTP embed
        let embedder = ctx.agent.get_or_create_embedder(&model).await?;
        let query_embedding = crate::embed::embed_one(embedder.as_ref(), query).await?;

        // Sync SQLite off async runtime
        let root2 = root.clone();
        let model2 = model.clone();
        let scope2 = scope.clone();
        let library_registry2 = library_registry.clone();
        let (results, memory_results, staleness) = tokio::task::spawn_blocking(move || {
            // Guard: catch model/dimension mismatch before sqlite-vec sees the
            // wrong-dimensioned query vector and emits a cryptic error.
            // Only applicable for project-scoped searches (library DBs may use
            // a different model and have their own dimension validation).
            if matches!(scope2, crate::library::scope::Scope::Project) {
                let conn = crate::embed::index::open_db(&root2)?;
                crate::embed::index::check_model_mismatch(&conn, &model2).map_err(|e| {
                    super::RecoverableError::with_hint(
                        e.to_string(),
                        "Run index_project(force: true) to rebuild the index with the current model.",
                    )
                })?;
            }
            let results = crate::embed::index::search_multi_db(
                &root2,
                &query_embedding,
                limit,
                &scope2,
                &library_registry2,
            )?;
            let memory_results = if include_memories {
                let conn = crate::embed::index::open_db(&root2)?;
                crate::embed::index::ensure_vec_memories(&conn)?;
                crate::embed::index::search_memories(&conn, &query_embedding, None, limit)?
            } else {
                vec![]
            };
            let staleness = {
                let conn = crate::embed::index::open_db(&root2)?;
                crate::embed::index::check_index_staleness(&conn, &root2).ok()
            };
            anyhow::Ok((results, memory_results, staleness))
        })
        .await??;

        // Transform code results based on mode; keep score alongside for sorting
        let mut scored_items: Vec<(f32, Value)> = results
            .iter()
            .map(|r| {
                let content_field = if guard.should_include_body() {
                    // Focused mode: full content
                    r.content.clone()
                } else {
                    // Exploring mode: first line only, max 50 chars (matches text compact format)
                    let first_line = r.content.lines().next().unwrap_or("").trim();
                    let char_count = first_line.chars().count();
                    if char_count > 50 {
                        let truncated: String = first_line.chars().take(47).collect();
                        format!("{}...", truncated)
                    } else {
                        first_line.to_string()
                    }
                };
                (
                    r.score,
                    format_search_result_item(
                        &r.file_path,
                        r.start_line,
                        r.end_line,
                        &r.source,
                        content_field,
                    ),
                )
            })
            .collect();

        // Merge memory results if requested
        for mr in &memory_results {
            let content_field = if guard.should_include_body() {
                mr.content.clone()
            } else {
                let first_line = mr.content.lines().next().unwrap_or("").trim();
                if first_line.len() > 50 {
                    format!("{}...", &first_line[..47])
                } else {
                    first_line.to_string()
                }
            };
            scored_items.push((
                mr.similarity,
                format_search_result_item(
                    &format!("[memory:{}]", mr.title),
                    0,
                    0,
                    "memory",
                    content_field,
                ),
            ));
        }

        // Sort by score descending (high first), then discard scores — order is the signal
        scored_items
            .sort_by(|(sa, _), (sb, _)| sb.partial_cmp(sa).unwrap_or(std::cmp::Ordering::Equal));
        let result_items: Vec<Value> = scored_items.into_iter().map(|(_, item)| item).collect();

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
        // Check index freshness — framed as informational, not a quality signal
        if let Some(staleness) = staleness {
            if staleness.stale {
                result["git_sync"] = json!({
                    "status": "behind",
                    "behind_commits": staleness.behind_commits,
                    "note": "Recent commits not yet indexed — run index_project to include new code. Existing results are unaffected."
                });
            }
        }
        // Warn if write tools have modified files in this session that haven't been re-indexed.
        let dirty = ctx.agent.dirty_file_count().await;
        if dirty > 0 {
            result["unindexed_writes"] = json!({
                "status": "warn",
                "file_count": dirty,
                "note": format!(
                    "{} file{} modified in this session but not yet re-indexed — \
                     run index_project() to include recent changes in semantic search.",
                    dirty,
                    if dirty == 1 { " was" } else { "s were" }
                )
            });
        }
        // Check for stale libraries
        let stale = {
            let inner = ctx.agent.inner.read().await;
            if let Some(p) = &inner.active_project {
                p.library_registry
                    .stale_libraries()
                    .into_iter()
                    .map(|e| {
                        json!({
                            "name": e.name,
                            "indexed": e.version_indexed,
                            "current": e.version,
                            "hint": format!(
                                "{} was updated — run index_project(scope='lib:{}') to re-index",
                                e.name, e.name
                            )
                        })
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![]
            }
        };
        if !stale.is_empty() {
            result["stale_libraries"] = json!(stale);
        }
        Ok(result)
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_semantic_search(result))
    }
}

#[async_trait::async_trait]
impl Tool for IndexProject {
    fn name(&self) -> &str {
        "index_project"
    }
    fn description(&self) -> &str {
        "Build or incrementally update the semantic search index for the active project. \
         Use scope='lib:<name>' to index a registered library (replaces index_library)."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "force": { "type": "boolean", "default": false,
                    "description": "Force full reindex, ignoring cached file hashes" },
                "scope": {
                    "type": "string",
                    "default": "project",
                    "description": "Scope to index: 'project' (default) to index the active project, or 'lib:<name>' to index a registered library. Replaces index_library."
                }
            }
        })
    }
    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<Value> {
        use crate::agent::IndexingState;

        let scope_str = input["scope"].as_str().unwrap_or("project");

        // Library scope: delegate to library indexing logic (replaces index_library tool)
        if let Some(lib_name) = scope_str.strip_prefix("lib:") {
            let force = parse_bool_param(&input["force"]);

            let (root, lib_path) = {
                let inner = ctx.agent.inner.read().await;
                let project = inner.active_project.as_ref().ok_or_else(|| {
                    crate::tools::RecoverableError::with_hint(
                        "No active project. Use activate_project first.",
                        "Call activate_project(\"/path/to/project\") to set the active project.",
                    )
                })?;
                let entry = project.library_registry.lookup(lib_name).ok_or_else(|| {
                    crate::tools::RecoverableError::with_hint(
                        format!("Library '{}' not found in registry.", lib_name),
                        "Use list_libraries to see registered libraries.",
                    )
                })?;
                (project.root.clone(), entry.path.clone())
            };

            let source = format!("lib:{}", lib_name);
            crate::embed::index::build_library_index(&root, &lib_path, &source, force).await?;

            // Read current version from lockfile and write back
            let versions = crate::library::versions::resolve_dependency_versions(&root);
            let current_version = crate::library::versions::find_version(&versions, lib_name);

            {
                let mut inner = ctx.agent.inner.write().await;
                let project = inner.active_project.as_mut().unwrap();
                if let Some(entry) = project.library_registry.lookup_mut(lib_name) {
                    entry.indexed = true;
                    if let Some(ver) = &current_version {
                        entry.version = Some(ver.clone());
                        entry.version_indexed = Some(ver.clone());
                        entry.nudge_dismissed = false;
                    }
                }
                let registry_path = project.root.join(".codescout").join("libraries.json");
                project.library_registry.save(&registry_path)?;
            }

            // Write version_indexed to lib_meta table
            if let Some(ver) = &current_version {
                let ver2 = ver.clone();
                let root2 = root.clone();
                let lib_name2 = lib_name.to_string();
                tokio::task::spawn_blocking(move || {
                    let lib_conn =
                        crate::embed::index::open_lib_db(&root2, &lib_name2)?;
                    lib_conn.execute(
                        "INSERT OR REPLACE INTO lib_meta (key, value) VALUES ('version_indexed', ?)",
                        rusqlite::params![ver2],
                    )?;
                    anyhow::Ok(())
                })
                .await??;
            }

            let source2 = source.clone();
            let root2 = root.clone();
            let (file_count, chunk_count) = tokio::task::spawn_blocking(move || {
                let conn = crate::embed::index::open_db(&root2)?;
                let by_source = crate::embed::index::index_stats_by_source(&conn)?;
                let lib_stats = by_source.get(&source2);
                anyhow::Ok((
                    lib_stats.map_or(0, |s| s.file_count),
                    lib_stats.map_or(0, |s| s.chunk_count),
                ))
            })
            .await??;

            return Ok(json!({
                "status": "ok",
                "library": lib_name,
                "source": source,
                "files_indexed": file_count,
                "chunks": chunk_count,
            }));
        }

        let force = parse_bool_param(&input["force"]);
        let root = ctx.agent.require_project_root().await?;

        // Guard against concurrent runs.
        {
            let mut state = ctx.agent.indexing.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*state, IndexingState::Running { .. }) {
                return Ok(json!({
                    "status": "already_running",
                    "hint": "Use index_status() to check progress."
                }));
            }
            *state = IndexingState::Running {
                done: 0,
                total: 0,
                eta_secs: None,
            };
        }

        let state_arc = ctx.agent.indexing.clone();
        let progress = ctx.progress.clone();
        // Signal start immediately (step 0 = initializing).
        if let Some(p) = &progress {
            p.report(0, None).await;
        }

        // Separate clone: progress_cb captures this; state_arc is used after build_index returns.
        let state_arc_cb = ctx.agent.indexing.clone();
        let progress_cb: Option<crate::embed::index::ProgressCb> =
            Some(Box::new(move |done, total, eta_secs| {
                let mut s = state_arc_cb.lock().unwrap_or_else(|e| e.into_inner());
                *s = IndexingState::Running {
                    done,
                    total,
                    eta_secs,
                };
            }));

        // Capture the dirty-files Arc before spawning so the task can clear it on success.
        let dirty_files_arc = ctx.agent.dirty_files_arc().await;

        tokio::spawn(async move {
            let result = crate::embed::index::build_index(&root, force, progress_cb).await;

            // Gather post-index stats *before* locking the mutex so that a
            // MutexGuard (which is !Send) is never held across an await point.
            let stats = if result.is_ok() {
                tokio::task::spawn_blocking({
                    let root = root.clone();
                    move || {
                        crate::embed::index::open_db(&root)
                            .and_then(|conn| crate::embed::index::index_stats(&conn))
                            .map(|s| (s.file_count, s.chunk_count))
                            .unwrap_or((0, 0))
                    }
                })
                .await
                .unwrap_or((0, 0))
            } else {
                (0, 0)
            };

            {
                // Drop the MutexGuard before any `.await` — MutexGuard is !Send.
                let mut state = state_arc.lock().unwrap_or_else(|e| e.into_inner());
                *state = match result {
                    Ok(report) => {
                        // Indexing succeeded — files are now fresh, clear the dirty set.
                        if let Some(ref arc) = dirty_files_arc {
                            arc.lock().unwrap_or_else(|e| e.into_inner()).clear();
                        }
                        IndexingState::Done {
                            files_indexed: report.indexed,
                            files_deleted: report.deleted,
                            detail: report.skipped_msg,
                            total_files: stats.0,
                            total_chunks: stats.1,
                        }
                    }
                    Err(e) => IndexingState::Failed(e.to_string()),
                };
            }
            // Signal completion (step 1 of 1).
            if let Some(p) = &progress {
                p.report(1, Some(1)).await;
            }
        });

        Ok(json!({
            "status": "started",
            "hint": "Indexing is running in the background. Use index_status() to check when complete."
        }))
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_index_project(result))
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
            let p = inner.active_project.as_ref().ok_or_else(|| {
                super::RecoverableError::with_hint(
                    "No active project. Use activate_project first.",
                    "Call activate_project(\"/path/to/project\") to set the active project.",
                )
            })?;
            (
                p.root.clone(),
                p.config.embeddings.model.clone(),
                p.config.embeddings.drift_detection_enabled,
            )
        };

        let db_path = crate::embed::index::project_db_path(&root);
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
            "queryable": true,
            "configured_model": model,
            "indexed_with_model": stats.model,
            "indexed_at": stats.indexed_at,
            "file_count": stats.file_count,
            "chunk_count": stats.chunk_count,
            "embedding_count": stats.embedding_count,
            "db_path": db_path_str,
            "by_source": by_source_json,
        });

        // Git-sync info — framed as informational, not a quality signal
        if let Some(staleness) = staleness {
            result["git_sync"] = if staleness.stale {
                json!({
                    "status": "behind",
                    "behind_commits": staleness.behind_commits,
                    "note": "Recent commits are not yet indexed. All previously indexed code is still queryable — run index_project to include new code."
                })
            } else {
                json!({ "status": "up_to_date" })
            };
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
                    "hint": "Drift detection is opted out. Re-enable it in .codescout/project.toml:\n[embeddings]\ndrift_detection_enabled = true"
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
                let mut drift_result = json!({
                    "note": "Drift scores are informational — they do not affect query results. High drift means code has changed since indexing; run index_project to update.",
                    "results": items,
                    "total": total,
                });
                if let Some(ov) = overflow {
                    drift_result["overflow"] = OutputGuard::overflow_json(&ov);
                }
                result["drift"] = drift_result;
            }
        }

        // Append background indexing state if not idle.
        {
            use crate::agent::IndexingState;
            let state = ctx.agent.indexing.lock().unwrap_or_else(|e| e.into_inner());
            match &*state {
                IndexingState::Idle => {}
                IndexingState::Running {
                    done,
                    total,
                    eta_secs,
                } => {
                    result["indexing"] = json!({
                        "status": "running",
                        "done": done,
                        "total": total,
                        "eta_secs": eta_secs,
                    });
                }
                IndexingState::Done {
                    files_indexed,
                    files_deleted,
                    detail,
                    total_files,
                    total_chunks,
                } => {
                    result["indexing"] = json!({
                        "status": "done",
                        "files_indexed": files_indexed,
                        "files_deleted": files_deleted,
                        "detail": detail,
                        "total_files": total_files,
                        "total_chunks": total_chunks,
                    });
                }
                IndexingState::Failed(e) => {
                    result["indexing"] = json!({ "status": "failed", "error": e });
                }
            }
        }

        // Append per-library indexing states (non-idle only)
        let lib_states = ctx.agent.library_states_summary();
        if !lib_states.is_empty() {
            result["libraries"] = serde_json::to_value(&lib_states)?;
        }

        Ok(result)
    }

    fn format_compact(&self, result: &Value) -> Option<String> {
        Some(format_index_status(result))
    }
}

/// Build a single search result item JSON object.
/// Field order: file_path → score → start_line → end_line → language → source → content
/// (score is the primary quality signal and must appear before content to let the
/// LLM assess relevance before paying the token cost of reading the chunk)
fn format_search_result_item(
    file_path: &str,
    start_line: usize,
    end_line: usize,
    source: &str,
    content: String,
) -> Value {
    // Field order: identity → location → metadata → content (bulk payload last)
    let mut map = serde_json::Map::new();
    map.insert("file_path".into(), json!(file_path));
    map.insert("start_line".into(), json!(start_line));
    map.insert("end_line".into(), json!(end_line));
    if source != "project" {
        map.insert("source".into(), json!(source));
    }
    map.insert("content".into(), json!(content));
    Value::Object(map)
}

fn format_semantic_search(val: &Value) -> String {
    let results = match val["results"].as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };
    let total = val["total"].as_u64().unwrap_or(results.len() as u64);

    if results.is_empty() {
        return "0 results".to_string();
    }

    let result_word = if total == 1 { "result" } else { "results" };
    let mut out = format!("{total} {result_word}\n");

    // Build rows: (location, preview)
    let rows: Vec<(String, String)> = results
        .iter()
        .map(|r| {
            let file = r["file_path"].as_str().unwrap_or("?");
            let start = r["start_line"].as_u64().unwrap_or(0);
            let end = r["end_line"].as_u64().unwrap_or(0);
            let location = if start > 0 && end > 0 && start != end {
                format!("{file}:{start}-{end}")
            } else if start > 0 {
                format!("{file}:{start}")
            } else {
                file.to_string()
            };

            // Content preview: first line, truncated to ~50 chars
            let content = r["content"].as_str().unwrap_or("");
            let first_line = content.lines().next().unwrap_or("").trim();
            let preview = if first_line.len() > 50 {
                format!("{}...", &first_line[..47])
            } else {
                first_line.to_string()
            };

            (location, preview)
        })
        .collect();

    let max_loc_len = rows.iter().map(|(l, _)| l.len()).max().unwrap_or(0);

    for (location, preview) in &rows {
        out.push('\n');
        out.push_str("  ");
        out.push_str(location);
        if !preview.is_empty() {
            let loc_pad = max_loc_len - location.len();
            for _ in 0..loc_pad {
                out.push(' ');
            }
            out.push_str("  ");
            out.push_str(preview);
        }
    }

    // Git sync info (informational only — does not affect result quality)
    if val["git_sync"]["status"].as_str() == Some("behind") {
        out.push('\n');
        if let Some(n) = val["git_sync"]["behind_commits"].as_u64() {
            out.push_str(&format!(
                "\n  {n} commits not yet indexed (results still valid — run index_project to include new code)"
            ));
        }
    }

    // Overflow
    if let Some(overflow) = val.get("overflow").filter(|o| o.is_object()) {
        out.push('\n');
        out.push_str(&format_overflow(overflow));
    }

    out
}

fn format_index_project(result: &Value) -> String {
    let status = result["status"].as_str().unwrap_or("?");
    format!("index {status}")
}

fn format_index_status(result: &Value) -> String {
    let indexed = result["indexed"].as_bool().unwrap_or(false);
    if !indexed {
        return "not indexed".to_string();
    }
    let files = result["file_count"].as_u64().unwrap_or(0);
    let chunks = result["chunk_count"].as_u64().unwrap_or(0);

    let mut out = format!("good · queryable · {files} files · {chunks} chunks");

    if let Some(model) = result["indexed_with_model"].as_str() {
        out.push_str(&format!(" · {model}"));
    }
    if let Some(ts) = result["indexed_at"].as_str() {
        out.push_str(&format!(" · {ts}"));
    }
    if result["git_sync"]["status"].as_str() == Some("behind") {
        if let Some(behind) = result["git_sync"]["behind_commits"]
            .as_u64()
            .filter(|&n| n > 0)
        {
            out.push_str(&format!(
                " · {behind} commits not yet indexed (queryable, run index_project to catch up)"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::embed::index;
    use crate::lsp::LspManager;
    use tempfile::tempdir;

    #[tokio::test]
    async fn index_project_sets_initial_running_state() {
        use crate::agent::IndexingState;
        // Call index_project on a project with no embedder configured.
        // It will start and quickly fail in the background, but the
        // initial Running state is set synchronously before the spawn.
        let (_dir, ctx) = project_ctx().await;
        let _ = IndexProject.call(json!({}), &ctx).await;

        // State should not be left Idle after a call — it was either
        // set to Running (still in progress) or transitioned to Done/Failed.
        let state = ctx.agent.indexing.lock().unwrap().clone();
        assert!(!matches!(state, IndexingState::Idle));
    }

    async fn project_ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        (
            dir,
            ToolContext {
                agent,
                lsp: LspManager::new_arc(),
                output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(
                    20,
                )),
                progress: None,
            },
        )
    }

    async fn drift_enabled_ctx() -> (tempfile::TempDir, ToolContext) {
        let dir = tempdir().unwrap();
        let ce_dir = dir.path().join(".codescout");
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
                lsp: LspManager::new_arc(),
                output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(
                    20,
                )),
                progress: None,
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
    async fn index_status_shows_running_progress() {
        use crate::agent::IndexingState;
        let (dir, ctx) = project_ctx().await;
        // Create the DB so index_status doesn't early-return "no index"
        let conn = crate::embed::index::open_db(dir.path()).unwrap();
        drop(conn);

        // Simulate mid-run state
        {
            let mut state = ctx.agent.indexing.lock().unwrap();
            *state = IndexingState::Running {
                done: 10,
                total: 50,
                eta_secs: Some(20),
            };
        }

        let result = IndexStatus.call(json!({}), &ctx).await.unwrap();
        let indexing = &result["indexing"];
        assert_eq!(indexing["status"], "running");
        assert_eq!(indexing["done"], 10);
        assert_eq!(indexing["total"], 50);
        assert_eq!(indexing["eta_secs"], 20);

        // Verify eta_secs: None renders as JSON null (e.g. on last file or initial state)
        {
            let mut state = ctx.agent.indexing.lock().unwrap();
            *state = IndexingState::Running {
                done: 50,
                total: 50,
                eta_secs: None,
            };
        }
        let result2 = IndexStatus.call(json!({}), &ctx).await.unwrap();
        assert_eq!(result2["indexing"]["eta_secs"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn tools_error_without_project() {
        let ctx = ToolContext {
            agent: Agent::new(None).await.unwrap(),
            lsp: LspManager::new_arc(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
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
        // Helper mirrors the inline logic in SemanticSearch::call
        let truncate = |content: &str| -> String {
            let first_line = content.lines().next().unwrap_or("").trim();
            let char_count = first_line.chars().count();
            if char_count > 50 {
                let truncated: String = first_line.chars().take(47).collect();
                format!("{}...", truncated)
            } else {
                first_line.to_string()
            }
        };

        // ASCII: long content truncated to 47 chars + "..."
        let long_ascii = "x".repeat(100);
        let preview = truncate(&long_ascii);
        assert_eq!(preview.chars().count(), 50); // 47 + "..."
        assert!(preview.ends_with("..."));

        // Unicode: multi-byte chars must not panic
        // Each '日' is 3 bytes; 51 chars = 153 bytes — old [..47] would panic mid-codepoint
        let long_unicode = "日".repeat(51);
        let preview_unicode = truncate(&long_unicode);
        assert_eq!(preview_unicode.chars().count(), 50); // 47 '日' + "..."
        assert!(preview_unicode.ends_with("..."));

        // Emoji: also multi-byte
        let emoji_line = "🦀".repeat(51);
        let preview_emoji = truncate(&emoji_line);
        assert_eq!(preview_emoji.chars().count(), 50);

        // Multi-line: only first line is used
        let multiline = "first line\nsecond line\nthird line";
        assert_eq!(truncate(multiline), "first line");

        // Short content: no truncation
        assert_eq!(truncate("short"), "short");
    }

    #[tokio::test]
    async fn semantic_search_schema_has_scope() {
        let schema = SemanticSearch.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("scope"), "should accept scope parameter");
    }

    #[test]
    fn semantic_search_schema_has_include_memories() {
        let schema = SemanticSearch.input_schema();
        assert!(schema["properties"]["include_memories"].is_object());
        assert_eq!(schema["properties"]["include_memories"]["type"], "boolean");
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
        assert_eq!(result["git_sync"]["status"], "behind");
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
        let ce_dir = dir.path().join(".codescout");
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
            lsp: LspManager::new_arc(),
            output_buffer: std::sync::Arc::new(crate::tools::output_buffer::OutputBuffer::new(20)),
            progress: None,
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

    // --- format_semantic_search tests ---

    #[test]
    fn semantic_search_basic() {
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "src/tools/output.rs",
                    "language": "rust",
                    "content": "pub struct OutputGuard {\n    mode: OutputMode,\n}",
                    "start_line": 35,
                    "end_line": 50,
                    "score": 0.923,
                    "source": "project"
                },
                {
                    "file_path": "src/tools/mod.rs",
                    "language": "rust",
                    "content": "pub trait Tool {\n    fn name(&self) -> &str;\n}",
                    "start_line": 120,
                    "end_line": 140,
                    "score": 0.81,
                    "source": "project"
                }
            ],
            "total": 2
        });
        let result = format_semantic_search(&val);
        assert!(result.starts_with("2 results\n"));
        assert!(result.contains("src/tools/output.rs:35-50"));
        assert!(result.contains("src/tools/mod.rs:120-140"));
        assert!(result.contains("pub struct OutputGuard {"));
        assert!(result.contains("pub trait Tool {"));
    }

    #[test]
    fn semantic_search_single_result() {
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "src/main.rs",
                    "language": "rust",
                    "content": "fn main() {}",
                    "start_line": 1,
                    "end_line": 1,
                    "score": 0.95,
                    "source": "project"
                }
            ],
            "total": 1
        });
        let result = format_semantic_search(&val);
        assert!(result.starts_with("1 result\n"));
        assert!(!result.starts_with("1 results"));
        assert!(result.contains("src/main.rs:1"));
    }

    #[test]
    fn semantic_search_empty() {
        let val = serde_json::json!({
            "results": [],
            "total": 0
        });
        assert_eq!(format_semantic_search(&val), "0 results");
    }

    #[test]
    fn semantic_search_missing_results() {
        let val = serde_json::json!({});
        assert_eq!(format_semantic_search(&val), "");
    }

    #[test]
    fn semantic_search_with_staleness() {
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "src/a.rs",
                    "content": "fn foo() {}",
                    "start_line": 1,
                    "end_line": 5,
                    "score": 0.9,
                    "source": "project"
                }
            ],
            "total": 1,
            "git_sync": { "status": "behind", "behind_commits": 5 }
        });
        let result = format_semantic_search(&val);
        assert!(result.contains("5 commits not yet indexed"));
        assert!(result.contains("index_project"));
    }

    #[test]
    fn semantic_search_with_overflow() {
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "src/a.rs",
                    "content": "fn foo() {}",
                    "start_line": 1,
                    "end_line": 5,
                    "score": 0.9,
                    "source": "project"
                }
            ],
            "total": 50,
            "overflow": {
                "shown": 10,
                "total": 50,
                "hint": "Use detail_level='full' with offset for pagination"
            }
        });
        let result = format_semantic_search(&val);
        assert!(result.contains("10 of 50"));
    }

    #[test]
    fn semantic_search_long_content_truncated() {
        let long_content = "a".repeat(80);
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "src/a.rs",
                    "content": long_content,
                    "start_line": 1,
                    "end_line": 10,
                    "score": 0.85,
                    "source": "project"
                }
            ],
            "total": 1
        });
        let result = format_semantic_search(&val);
        assert!(result.contains("..."));
        assert!(!result.contains(&"a".repeat(80)));
    }

    #[test]
    fn semantic_search_score_alignment() {
        let val = serde_json::json!({
            "results": [
                {
                    "file_path": "a.rs",
                    "content": "short",
                    "start_line": 1, "end_line": 1,
                    "score": 0.9, "source": "project"
                },
                {
                    "file_path": "very/long/path/to/file.rs",
                    "content": "long path",
                    "start_line": 100, "end_line": 200,
                    "score": 0.85, "source": "project"
                }
            ],
            "total": 2
        });
        let result = format_semantic_search(&val);
        assert!(result.contains("a.rs:1"));
        assert!(result.contains("very/long/path/to/file.rs:100-200"));
    }

    // --- format_index_status tests ---

    #[test]
    fn format_index_status_shows_model_and_timestamp() {
        let result = serde_json::json!({
            "indexed": true,
            "file_count": 42,
            "chunk_count": 1234,
            "git_sync": { "status": "up_to_date" },
            "indexed_with_model": "text-embedding-3-small",
            "indexed_at": "2026-03-01 14:22"
        });
        let out = format_index_status(&result);
        assert!(
            out.contains("42 files"),
            "should show file count, got: {out}"
        );
        assert!(
            out.contains("1234 chunks"),
            "should show chunk count, got: {out}"
        );
        assert!(
            out.contains("text-embedding-3-small"),
            "should show model, got: {out}"
        );
        assert!(
            out.contains("2026-03-01"),
            "should show timestamp, got: {out}"
        );
    }

    #[test]
    fn format_index_status_stale_shows_commit_count() {
        let result = serde_json::json!({
            "indexed": true,
            "file_count": 10,
            "chunk_count": 100,
            "git_sync": { "status": "behind", "behind_commits": 5 }
        });
        let out = format_index_status(&result);
        assert!(
            out.contains("5 commits not yet indexed"),
            "should note git sync lag, got: {out}"
        );
    }

    #[test]
    fn search_result_item_content_is_last_field() {
        // Regression: content must be last — it is the bulk payload.
        // Score is excluded from output; ranking is communicated by result order.
        let item =
            format_search_result_item("src/foo.rs", 10, 20, "project", "fn hello() {}".to_string());
        let keys: Vec<&str> = item
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();

        assert!(
            keys.iter().all(|k| *k != "score"),
            "score must not appear in output, got key order: {keys:?}"
        );
        let content_pos = keys.iter().position(|k| *k == "content").unwrap();
        assert_eq!(
            content_pos,
            keys.len() - 1,
            "content must be the last field, got key order: {keys:?}"
        );
    }

    #[tokio::test]
    async fn semantic_search_uses_scope_for_library_search() {
        // Scope "lib:nonexistent" should succeed gracefully (empty results, no error).
        // This tests that SemanticSearch wires scope through to search_multi_db rather
        // than using the old source_filter path.
        let (_dir, ctx) = project_ctx().await;
        let tool = SemanticSearch;
        // No embedder is configured, so embed_one will fail before we reach the DB.
        // We only need to verify the scope parsing / wiring doesn't panic or error
        // in an unexpected way — RecoverableError for missing embedder is acceptable.
        // To avoid the embedder call entirely, we set up a dummy project DB.
        let result = tool
            .call(
                json!({"query": "runtime", "scope": "lib:nonexistent"}),
                &ctx,
            )
            .await;
        // Either Ok (empty results from nonexistent lib) or a RecoverableError about
        // the missing embedder — both are acceptable. A plain anyhow error from
        // source_filter/search_scoped code paths would be a regression.
        match &result {
            Ok(val) => {
                // If we somehow get results (e.g. future test embedder), total >= 0
                assert!(val["total"].as_u64().unwrap_or(0) == 0);
            }
            Err(e) => {
                // Only acceptable error is about embedder / model config — not about
                // a missing source_filter or broken DB path.
                let msg = e.to_string();
                assert!(
                    msg.contains("model") || msg.contains("embed") || msg.contains("project"),
                    "unexpected error (not embedder-related): {msg}"
                );
            }
        }
    }
}
