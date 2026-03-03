# Semantic Memories Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `remember`/`recall`/`forget` actions to the memory tool, storing embedded memories in `embeddings.db` with auto-classification and vector search.

**Architecture:** Extend `open_db()` to create `memories` + `vec_memories` tables. Add a `classify_bucket()` heuristic. Extend the `Memory` tool with three new action branches that embed content via the project's configured embedder and store/search/delete in SQLite.

**Tech Stack:** Rust, rusqlite, sqlite-vec (vec0), existing `Embedder` trait, serde_json

---

### Task 1: Add `memories` and `vec_memories` Tables to `open_db`

**Files:**
- Modify: `src/embed/index.rs:61-129` (the `open_db` function)
- Test: `src/embed/index.rs` (existing test module at L1389)

**Step 1: Write the failing test**

In `src/embed/index.rs` tests module, add:

```rust
#[test]
fn open_db_creates_memories_table() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_db(dir.path()).unwrap();
    // Should be able to query the memories table without error
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn open_db_creates_vec_memories_table() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_db(dir.path()).unwrap();
    // vec_memories should exist (vec0 virtual table)
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='vec_memories'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(sql.contains("vec0"), "expected vec0 virtual table, got: {sql}");
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p code-explorer open_db_creates_memories -- --nocapture`
Expected: FAIL — `no such table: memories`

**Step 3: Implement the schema changes**

In `open_db()` at `src/embed/index.rs`, add after the existing `drift_report` CREATE TABLE and before the migrations:

```rust
        CREATE TABLE IF NOT EXISTS memories (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            bucket     TEXT NOT NULL DEFAULT 'unstructured',
            title      TEXT NOT NULL,
            content    TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_memories_bucket ON memories(bucket);
```

The `vec_memories` virtual table depends on knowing the embedding dimension, which isn't available at `open_db` time (it's set later via `set_meta("embedding_dims", ...)`). So create it lazily — add a new function `ensure_vec_memories` that creates the virtual table if it doesn't exist:

```rust
pub fn ensure_vec_memories(conn: &Connection) -> Result<()> {
    // Check if vec_memories already exists
    let exists: bool = conn
        .prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='vec_memories'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false);
    if exists {
        return Ok(());
    }

    // Need embedding dims from meta
    let dims = match get_meta(conn, "embedding_dims")? {
        Some(s) => s.parse::<usize>().unwrap_or(0),
        None => return Ok(()), // No index built yet, skip
    };
    if dims == 0 {
        return Ok(());
    }

    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE vec_memories USING vec0(\
         embedding float[{dims}] distance_metric=cosine)"
    ))?;
    Ok(())
}
```

**Step 4: Update the `open_db_creates_vec_memories_table` test**

The vec_memories test needs an embedding_dims meta value to trigger creation. Update:

```rust
#[test]
fn open_db_creates_vec_memories_table() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_db(dir.path()).unwrap();
    // Set embedding dims so ensure_vec_memories can create the table
    set_meta(&conn, "embedding_dims", "384").unwrap();
    ensure_vec_memories(&conn).unwrap();
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='vec_memories'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(sql.contains("vec0"), "expected vec0 virtual table, got: {sql}");
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p code-explorer open_db_creates_memories open_db_creates_vec_memories -- --nocapture`
Expected: PASS

**Step 6: Commit**

```bash
git add src/embed/index.rs
git commit -m "feat: add memories + vec_memories tables to embeddings.db"
```

---

### Task 2: Add Memory CRUD Functions in `src/embed/index.rs`

**Files:**
- Modify: `src/embed/index.rs` — add `insert_memory`, `search_memories`, `delete_memory`, `get_memory_by_title`
- Test: `src/embed/index.rs` tests module

**Step 1: Write failing tests**

```rust
#[test]
fn insert_and_search_memory() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_db(dir.path()).unwrap();
    set_meta(&conn, "embedding_dims", "3").unwrap();
    ensure_vec_memories(&conn).unwrap();

    let embedding = vec![0.1_f32, 0.2, 0.3];
    let id = insert_memory(
        &conn,
        "code",
        "test title",
        "test content about patterns",
        &embedding,
    )
    .unwrap();
    assert!(id > 0);

    // Search with same embedding should find it
    let results = search_memories(&conn, &embedding, None, 5).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "test title");
    assert_eq!(results[0].bucket, "code");
}

#[test]
fn delete_memory_removes_from_both_tables() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_db(dir.path()).unwrap();
    set_meta(&conn, "embedding_dims", "3").unwrap();
    ensure_vec_memories(&conn).unwrap();

    let embedding = vec![0.1_f32, 0.2, 0.3];
    let id = insert_memory(&conn, "code", "to delete", "content", &embedding).unwrap();
    delete_memory(&conn, id).unwrap();

    let results = search_memories(&conn, &embedding, None, 5).unwrap();
    assert!(results.is_empty());
}

#[test]
fn search_memories_filters_by_bucket() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_db(dir.path()).unwrap();
    set_meta(&conn, "embedding_dims", "3").unwrap();
    ensure_vec_memories(&conn).unwrap();

    let e1 = vec![0.1_f32, 0.2, 0.3];
    let e2 = vec![0.3_f32, 0.2, 0.1];
    insert_memory(&conn, "code", "code mem", "patterns", &e1).unwrap();
    insert_memory(&conn, "system", "sys mem", "build stuff", &e2).unwrap();

    let code_only = search_memories(&conn, &e1, Some("code"), 5).unwrap();
    assert_eq!(code_only.len(), 1);
    assert_eq!(code_only[0].bucket, "code");
}
```

**Step 2: Run to verify failures**

Run: `cargo test -p code-explorer insert_and_search_memory delete_memory_removes search_memories_filters -- --nocapture`
Expected: FAIL — functions don't exist

**Step 3: Implement the functions**

Add a `MemoryResult` struct and the three functions:

```rust
pub struct MemoryResult {
    pub id: i64,
    pub bucket: String,
    pub title: String,
    pub content: String,
    pub similarity: f32,
    pub created_at: String,
}

pub fn insert_memory(
    conn: &Connection,
    bucket: &str,
    title: &str,
    content: &str,
    embedding: &[f32],
) -> Result<i64> {
    let now = utc_now_display();
    conn.execute(
        "INSERT INTO memories (bucket, title, content, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![bucket, title, content, now, now],
    )?;
    let id = conn.last_insert_rowid();

    let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
    conn.execute(
        "INSERT INTO vec_memories (id, embedding) VALUES (?1, ?2)",
        params![id, blob],
    )?;
    Ok(id)
}

pub fn search_memories(
    conn: &Connection,
    query_embedding: &[f32],
    bucket_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<MemoryResult>> {
    let query_blob: Vec<u8> = query_embedding
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();

    let knn = "SELECT id, distance FROM vec_memories \
               WHERE embedding MATCH vec_f32(?1) ORDER BY distance LIMIT ?2";

    let (sql, has_bucket) = match bucket_filter {
        Some(_) => (
            format!(
                "SELECT m.id, m.bucket, m.title, m.content, m.created_at, \
                 COALESCE(knn.distance, 1.0) AS distance \
                 FROM memories m JOIN ({knn}) knn ON m.id = knn.id \
                 WHERE m.bucket = ?3 ORDER BY distance ASC"
            ),
            true,
        ),
        None => (
            format!(
                "SELECT m.id, m.bucket, m.title, m.content, m.created_at, \
                 COALESCE(knn.distance, 1.0) AS distance \
                 FROM memories m JOIN ({knn}) knn ON m.id = knn.id \
                 ORDER BY distance ASC"
            ),
            false,
        ),
    };

    let mut stmt = conn.prepare(&sql)?;
    let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<MemoryResult> {
        let distance: f64 = row.get(5)?;
        Ok(MemoryResult {
            id: row.get(0)?,
            bucket: row.get(1)?,
            title: row.get(2)?,
            content: row.get(3)?,
            created_at: row.get(4)?,
            similarity: (1.0_f32 - distance as f32).clamp(0.0, 1.0),
        })
    };

    let rows = if has_bucket {
        stmt.query_map(
            params![query_blob, limit as i64, bucket_filter.unwrap()],
            map_row,
        )?
    } else {
        stmt.query_map(params![query_blob, limit as i64], map_row)?
    };

    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn delete_memory(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM vec_memories WHERE id = ?1", params![id])?;
    conn.execute("DELETE FROM memories WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn upsert_memory_by_title(
    conn: &Connection,
    bucket: &str,
    title: &str,
    content: &str,
    embedding: &[f32],
) -> Result<i64> {
    // Check if a memory with this title already exists
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM memories WHERE title = ?1",
            params![title],
            |r| r.get(0),
        )
        .optional()?;

    if let Some(id) = existing {
        let now = utc_now_display();
        conn.execute(
            "UPDATE memories SET bucket = ?1, content = ?2, updated_at = ?3 WHERE id = ?4",
            params![bucket, content, now, id],
        )?;
        let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        conn.execute(
            "UPDATE vec_memories SET embedding = ?1 WHERE id = ?2",
            params![blob, id],
        )?;
        Ok(id)
    } else {
        insert_memory(conn, bucket, title, content, embedding)
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p code-explorer insert_and_search_memory delete_memory_removes search_memories_filters -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/embed/index.rs
git commit -m "feat: add memory CRUD functions (insert, search, delete, upsert)"
```

---

### Task 3: Add `classify_bucket()` Heuristic

**Files:**
- Create: `src/memory/classify.rs`
- Modify: `src/memory/mod.rs` — add `pub mod classify;`

**Step 1: Write failing tests**

Create `src/memory/classify.rs` with tests first:

```rust
/// Classify memory content into a bucket based on keyword heuristics.
/// Returns "code", "system", or "unstructured".
pub fn classify_bucket(content: &str) -> &'static str {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_code_content() {
        assert_eq!(
            classify_bucket("The auth module uses the builder pattern for request construction"),
            "code"
        );
    }

    #[test]
    fn classifies_system_content() {
        assert_eq!(
            classify_bucket("CI pipeline requires docker and uses GitHub Actions for deployment"),
            "system"
        );
    }

    #[test]
    fn classifies_unstructured_by_default() {
        assert_eq!(
            classify_bucket("The user prefers verbose output when debugging"),
            "unstructured"
        );
    }

    #[test]
    fn code_keywords_beat_system_when_mixed() {
        assert_eq!(
            classify_bucket("The function uses a config struct pattern with builder methods"),
            "code"
        );
    }

    #[test]
    fn file_paths_trigger_code() {
        assert_eq!(
            classify_bucket("Check src/tools/memory.rs for the implementation"),
            "code"
        );
    }
}
```

**Step 2: Run to verify failures**

Run: `cargo test -p code-explorer classify_bucket -- --nocapture`
Expected: FAIL — `todo!()` panics

**Step 3: Implement the heuristic**

Replace `todo!()` with:

```rust
pub fn classify_bucket(content: &str) -> &'static str {
    let lower = content.to_lowercase();

    let code_keywords = [
        "function", "method", "struct", "class", "trait", "impl", "pattern",
        "api", "endpoint", "convention", "naming", "import", "module", "crate",
        "package", "type", "interface", "refactor", "abstraction", "generic",
        "lifetime", "async", "iterator", "closure", "macro", "enum", "variant",
    ];
    let system_keywords = [
        "build", "deploy", "ci", "config", "environment", "docker", "infra",
        "database", "migration", "permission", "secret", "credential", "server",
        "port", "host", "pipeline", "cargo test", "npm", "pip", "github actions",
        "dockerfile", "kubernetes", "nginx", "ssl", "certificate",
    ];

    // File path heuristic: paths containing / with code extensions
    let has_code_path = lower.contains(".rs")
        || lower.contains(".ts")
        || lower.contains(".py")
        || lower.contains(".go")
        || lower.contains(".java")
        || lower.contains(".js")
        || lower.contains(".kt");

    let code_score: usize = code_keywords.iter().filter(|k| lower.contains(*k)).count()
        + if has_code_path { 2 } else { 0 };
    let system_score: usize = system_keywords.iter().filter(|k| lower.contains(*k)).count();

    if code_score == 0 && system_score == 0 {
        return "unstructured";
    }
    if code_score >= system_score {
        "code"
    } else {
        "system"
    }
}
```

**Step 4: Add module declaration**

In `src/memory/mod.rs`, add: `pub mod classify;`

**Step 5: Run tests**

Run: `cargo test -p code-explorer classify_bucket -- --nocapture`
Expected: PASS

**Step 6: Commit**

```bash
git add src/memory/classify.rs src/memory/mod.rs
git commit -m "feat: add keyword-based bucket classification heuristic"
```

---

### Task 4: Add `remember` Action to the Memory Tool

**Files:**
- Modify: `src/tools/memory.rs:219-347` (the `impl Tool for Memory` block)
- Test: `src/tools/memory.rs` tests module

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn memory_remember_stores_and_embeds() {
    let (_dir, ctx) = test_ctx_with_project().await;
    let tool = Memory;
    let result = tool
        .call(
            json!({
                "action": "remember",
                "content": "The auth module uses JWT tokens for session management",
                "title": "auth approach",
                "bucket": "code"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(result, json!("ok"));
}
```

Note: this test requires the embedder to be available. For unit tests, we'll need a mock or to skip embedding. The test should at minimum verify the action is recognized and dispatched. Integration with real embedding is tested separately.

**Step 2: Run to verify failure**

Run: `cargo test -p code-explorer memory_remember -- --nocapture`
Expected: FAIL — `unknown action 'remember'`

**Step 3: Implement the `remember` action**

In `src/tools/memory.rs`, extend the `Memory` tool:

1. Update `input_schema()`: add `"remember"`, `"recall"`, `"forget"` to the action enum. Add `title`, `bucket`, `query`, `limit`, `id` properties.

2. Add the `remember` match arm in `call()`:

```rust
"remember" => {
    let content = super::require_str_param(&input, "content")?;
    let title = input["title"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| extract_title(content));
    let bucket = input["bucket"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            crate::memory::classify::classify_bucket(content).to_string()
        });

    let (root, model) = {
        let inner = ctx.agent.inner.read().await;
        let p = inner.active_project.as_ref().ok_or_else(|| {
            super::RecoverableError::with_hint(
                "No active project.",
                "Call activate_project first.",
            )
        })?;
        (p.root.clone(), p.config.embeddings.model.clone())
    };

    let embedder = ctx.agent.get_or_create_embedder(&model).await?;
    let embedding = crate::embed::embed_one(embedder.as_ref(), content).await?;

    let root2 = root.clone();
    let bucket2 = bucket.clone();
    let title2 = title.clone();
    let content2 = content.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = crate::embed::index::open_db(&root2)?;
        crate::embed::index::ensure_vec_memories(&conn)?;
        crate::embed::index::insert_memory(
            &conn, &bucket2, &title2, &content2, &embedding,
        )?;
        anyhow::Ok(())
    })
    .await??;

    Ok(json!("ok"))
}
```

3. Add a helper `extract_title()`:

```rust
fn extract_title(content: &str) -> String {
    // First sentence or first 80 chars, whichever is shorter
    let first_sentence_end = content
        .find(". ")
        .or_else(|| content.find(".\n"))
        .map(|i| i + 1)
        .unwrap_or(content.len());
    let end = first_sentence_end.min(80).min(content.len());
    let mut title = content[..end].to_string();
    if end < content.len() && !title.ends_with('.') {
        title.push_str("...");
    }
    title
}
```

**Step 4: Run tests**

Run: `cargo test -p code-explorer memory_remember -- --nocapture`
Expected: PASS (or skip embedding — test verifies action dispatch)

**Step 5: Commit**

```bash
git add src/tools/memory.rs
git commit -m "feat: add 'remember' action to memory tool"
```

---

### Task 5: Add `recall` Action to the Memory Tool

**Files:**
- Modify: `src/tools/memory.rs`
- Test: `src/tools/memory.rs` tests module

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn memory_recall_unknown_action_before_impl() {
    let (_dir, ctx) = test_ctx_with_project().await;
    let tool = Memory;
    let result = tool
        .call(
            json!({
                "action": "recall",
                "query": "how does auth work"
            }),
            &ctx,
        )
        .await;
    // Should succeed (even if no results)
    assert!(result.is_ok());
    let val = result.unwrap();
    assert!(val["results"].is_array());
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p code-explorer memory_recall -- --nocapture`
Expected: FAIL — `unknown action 'recall'`

**Step 3: Implement the `recall` action**

```rust
"recall" => {
    let query = super::require_str_param(&input, "query")?;
    let limit = input["limit"].as_u64().unwrap_or(5) as usize;
    let bucket_filter = input["bucket"].as_str();

    let (root, model) = {
        let inner = ctx.agent.inner.read().await;
        let p = inner.active_project.as_ref().ok_or_else(|| {
            super::RecoverableError::with_hint(
                "No active project.",
                "Call activate_project first.",
            )
        })?;
        (p.root.clone(), p.config.embeddings.model.clone())
    };

    let embedder = ctx.agent.get_or_create_embedder(&model).await?;
    let query_embedding =
        crate::embed::embed_one(embedder.as_ref(), query).await?;

    let bucket = bucket_filter.map(|s| s.to_string());
    let results = tokio::task::spawn_blocking(move || {
        let conn = crate::embed::index::open_db(&root)?;
        crate::embed::index::ensure_vec_memories(&conn)?;
        crate::embed::index::search_memories(
            &conn,
            &query_embedding,
            bucket.as_deref(),
            limit,
        )
    })
    .await??;

    let items: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "bucket": r.bucket,
                "title": r.title,
                "content": r.content,
                "similarity": format!("{:.2}", r.similarity),
                "created_at": r.created_at,
            })
        })
        .collect();

    Ok(json!({ "results": items }))
}
```

**Step 4: Run tests**

Run: `cargo test -p code-explorer memory_recall -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/tools/memory.rs
git commit -m "feat: add 'recall' action to memory tool"
```

---

### Task 6: Add `forget` Action to the Memory Tool

**Files:**
- Modify: `src/tools/memory.rs`
- Test: `src/tools/memory.rs` tests module

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn memory_forget_unknown_action_before_impl() {
    let (_dir, ctx) = test_ctx_with_project().await;
    let tool = Memory;
    let result = tool
        .call(json!({ "action": "forget", "id": 999 }), &ctx)
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), json!("ok"));
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p code-explorer memory_forget -- --nocapture`
Expected: FAIL — `unknown action 'forget'`

**Step 3: Implement the `forget` action**

```rust
"forget" => {
    let id = input["id"]
        .as_i64()
        .ok_or_else(|| {
            super::RecoverableError::with_hint(
                "Missing required parameter 'id'",
                "Pass the numeric id from a recall result",
            )
        })?;

    let root = {
        let inner = ctx.agent.inner.read().await;
        let p = inner.active_project.as_ref().ok_or_else(|| {
            super::RecoverableError::with_hint(
                "No active project.",
                "Call activate_project first.",
            )
        })?;
        p.root.clone()
    };

    tokio::task::spawn_blocking(move || {
        let conn = crate::embed::index::open_db(&root)?;
        crate::embed::index::delete_memory(&conn, id)?;
        anyhow::Ok(())
    })
    .await??;

    Ok(json!("ok"))
}
```

**Step 4: Run tests**

Run: `cargo test -p code-explorer memory_forget -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/tools/memory.rs
git commit -m "feat: add 'forget' action to memory tool"
```

---

### Task 7: Cross-Embed Markdown Memories on `write`

**Files:**
- Modify: `src/tools/memory.rs` — the `"write"` match arm
- Test: `src/tools/memory.rs` tests module

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn memory_write_also_embeds_as_structured() {
    let (_dir, ctx) = test_ctx_with_project().await;
    let tool = Memory;

    // Write a structured memory
    tool.call(
        json!({ "action": "write", "topic": "architecture", "content": "Three layer design" }),
        &ctx,
    )
    .await
    .unwrap();

    // Should be findable via recall
    let result = tool
        .call(json!({ "action": "recall", "query": "layer design" }), &ctx)
        .await
        .unwrap();
    let results = result["results"].as_array().unwrap();
    assert!(
        results.iter().any(|r| r["title"].as_str() == Some("architecture")),
        "expected structured memory in recall results, got: {results:?}"
    );
}
```

Note: this test requires a working embedder. If the test environment doesn't have one, mark it `#[ignore]` and test manually. Or use a mock embedder.

**Step 2: Implement cross-embedding**

In the `"write"` match arm, after writing to markdown, also embed into `vec_memories` with `bucket: "structured"`:

```rust
"write" => {
    let topic = super::require_str_param(&input, "topic")?;
    let content = super::require_str_param(&input, "content")?;
    let private = input["private"].as_bool().unwrap_or(false);

    // Write markdown file (existing behavior)
    ctx.agent
        .with_project(|p| {
            if private {
                p.private_memory.write(topic, content)?;
            } else {
                p.memory.write(topic, content)?;
            }
            Ok(())
        })
        .await?;

    // Cross-embed into semantic store (best-effort, don't fail the write)
    if !private {
        let maybe = cross_embed_memory(ctx, topic, content).await;
        if let Err(e) = maybe {
            tracing::debug!("cross-embed memory failed (non-fatal): {e}");
        }
    }

    Ok(json!("ok"))
}
```

Add a helper:

```rust
async fn cross_embed_memory(
    ctx: &ToolContext,
    topic: &str,
    content: &str,
) -> anyhow::Result<()> {
    let (root, model) = {
        let inner = ctx.agent.inner.read().await;
        let p = inner
            .active_project
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no project"))?;
        (p.root.clone(), p.config.embeddings.model.clone())
    };

    let embedder = ctx.agent.get_or_create_embedder(&model).await?;
    let embedding = crate::embed::embed_one(embedder.as_ref(), content).await?;

    let topic_owned = topic.to_string();
    let content_owned = content.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = crate::embed::index::open_db(&root)?;
        crate::embed::index::ensure_vec_memories(&conn)?;
        crate::embed::index::upsert_memory_by_title(
            &conn,
            "structured",
            &topic_owned,
            &content_owned,
            &embedding,
        )?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
```

Similarly, update the `"delete"` arm to remove the cross-embedded entry.

**Step 3: Run tests**

Run: `cargo test -p code-explorer memory_write_also_embeds -- --nocapture`
Expected: PASS (or `#[ignore]` if no embedder in CI)

**Step 4: Commit**

```bash
git add src/tools/memory.rs
git commit -m "feat: cross-embed markdown memories into semantic store"
```

---

### Task 8: Add `include_memories` to `semantic_search`

**Files:**
- Modify: `src/tools/semantic.rs:20-44` (input_schema) and `src/tools/semantic.rs:45-137` (call)
- Test: `src/tools/semantic.rs` tests module

**Step 1: Write the failing test**

```rust
#[test]
fn semantic_search_schema_has_include_memories() {
    let schema = SemanticSearch.input_schema();
    assert!(schema["properties"]["include_memories"].is_object());
    assert_eq!(schema["properties"]["include_memories"]["type"], "boolean");
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p code-explorer semantic_search_schema_has_include_memories -- --nocapture`
Expected: FAIL

**Step 3: Implement**

1. Add `"include_memories"` to `input_schema()`.
2. In `call()`, after the existing code chunk search, if `include_memories` is true:
   - Call `search_memories` on the same connection
   - Map `MemoryResult` items to the same JSON shape with `"source": "memory"`
   - Merge with code results, sort by score descending

**Step 4: Run tests**

Run: `cargo test -p code-explorer semantic_search_schema_has_include_memories -- --nocapture`
Expected: PASS

**Step 5: Commit**

```bash
git add src/tools/semantic.rs
git commit -m "feat: add include_memories option to semantic_search"
```

---

### Task 9: Update Tool Description and Prompt Surfaces

**Files:**
- Modify: `src/tools/memory.rs:224-228` — update description string
- Modify: `src/prompts/server_instructions.md` — add remember/recall/forget to memory tool docs
- Modify: `src/prompts/onboarding_prompt.md` — mention semantic memories if applicable
- Modify: `src/tools/workflow.rs` — update `build_system_prompt_draft` if it references memory

**Step 1: Update the `Memory` tool description**

```rust
fn description(&self) -> &str {
    "Persistent project memory — action: \"read\", \"write\", \"list\", \"delete\". \
     topic is a path-like key (e.g. 'debugging/async-patterns'). \
     Pass private=true to use the gitignored private store.\n\
     Semantic memory — action: \"remember\", \"recall\", \"forget\". \
     Stores embedded, searchable knowledge classified into buckets (code/system/unstructured). \
     Use 'remember' to store insights, 'recall' to search by meaning, 'forget' to delete by id."
}
```

**Step 2: Update `server_instructions.md`**

Add a memory section in the tool reference table showing remember/recall/forget alongside the existing actions.

**Step 3: Run full test suite**

Run: `cargo test`
Expected: All 932+ tests pass

**Step 4: Run clippy and fmt**

Run: `cargo clippy -- -D warnings && cargo fmt`
Expected: Clean

**Step 5: Commit**

```bash
git add src/tools/memory.rs src/prompts/server_instructions.md src/prompts/onboarding_prompt.md src/tools/workflow.rs
git commit -m "docs: update tool descriptions and prompts for semantic memories"
```

---

### Task 10: Final Integration Test and Cleanup

**Files:**
- Modify: `src/tools/memory.rs` — add integration-style test
- Modify: `src/embed/index.rs` — ensure `memories` table is untouched by `build_index`

**Step 1: Write integration test**

```rust
#[tokio::test]
#[ignore] // requires embedder
async fn remember_recall_forget_roundtrip() {
    let (_dir, ctx) = test_ctx_with_project().await;
    let tool = Memory;

    // Remember
    tool.call(
        json!({
            "action": "remember",
            "content": "The LSP client uses a JSON-RPC transport layer",
            "bucket": "code"
        }),
        &ctx,
    )
    .await
    .unwrap();

    // Recall
    let result = tool
        .call(json!({ "action": "recall", "query": "LSP transport" }), &ctx)
        .await
        .unwrap();
    let results = result["results"].as_array().unwrap();
    assert!(!results.is_empty(), "expected at least one recall result");
    let id = results[0]["id"].as_i64().unwrap();

    // Forget
    tool.call(json!({ "action": "forget", "id": id }), &ctx)
        .await
        .unwrap();

    // Recall again — should be empty
    let result = tool
        .call(json!({ "action": "recall", "query": "LSP transport" }), &ctx)
        .await
        .unwrap();
    let results = result["results"].as_array().unwrap();
    assert!(results.is_empty(), "expected empty after forget");
}
```

**Step 2: Verify `build_index` doesn't touch memories**

Check that `build_index` in `src/embed/index.rs` only operates on `chunks`/`chunk_embeddings`/`files` tables. Add a test:

```rust
#[test]
fn build_index_does_not_clear_memories() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_db(dir.path()).unwrap();
    set_meta(&conn, "embedding_dims", "3").unwrap();
    ensure_vec_memories(&conn).unwrap();

    insert_memory(&conn, "code", "keep me", "important", &[0.1, 0.2, 0.3]).unwrap();

    // Simulate what build_index does to chunks
    conn.execute("DELETE FROM chunks", []).unwrap();
    conn.execute("DELETE FROM chunk_embeddings", []).unwrap();
    conn.execute("DELETE FROM files", []).unwrap();

    // Memories should survive
    let results = search_memories(&conn, &[0.1, 0.2, 0.3], None, 5).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "keep me");
}
```

**Step 3: Run full suite**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: All pass, clean clippy

**Step 4: Final commit**

```bash
git add -A
git commit -m "feat: complete semantic memories implementation with integration tests"
```
