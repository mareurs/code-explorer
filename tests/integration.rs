//! Integration tests: multi-tool workflows through the server handler.
//!
//! These tests exercise realistic tool sequences that a coding agent would
//! perform, ensuring tools compose correctly end-to-end.

use codescout::agent::Agent;
use codescout::lsp::LspManager;
use codescout::tools::{Tool, ToolContext};
use serde_json::json;
use tempfile::tempdir;

/// Create a project context with files pre-populated.
async fn project_with_files(files: &[(&str, &str)]) -> (tempfile::TempDir, ToolContext) {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    let ctx = ToolContext {
        agent,
        lsp: LspManager::new_arc(),
        output_buffer: std::sync::Arc::new(codescout::tools::output_buffer::OutputBuffer::new(20)),
        progress: None,
    };
    (dir, ctx)
}

// ---------------------------------------------------------------------------
// Workflow: Read → Search → Replace
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_read_search_replace() {
    use codescout::tools::file::{EditFile, ReadFile, SearchPattern};
    let (dir, ctx) = project_with_files(&[
        (
            "src/main.txt",
            "fn main() {\n    println!(\"Hello, world!\");\n}\n",
        ),
        (
            "src/lib.txt",
            "pub fn greet(name: &str) -> String {\n    format!(\"Hello, {}!\", name)\n}\n",
        ),
    ])
    .await;

    // Step 1: Search for "Hello" across the project
    let search_result = SearchPattern
        .call(
            json!({ "pattern": "Hello", "path": dir.path().display().to_string() }),
            &ctx,
        )
        .await
        .unwrap();
    let matches = search_result["matches"].as_array().unwrap();
    assert!(
        matches.len() >= 2,
        "expected matches in both files: {:?}",
        search_result
    );

    // Step 2: Read the file we want to modify
    let lib_path = dir.path().join("src/lib.txt").display().to_string();
    let read_result = ReadFile
        .call(json!({ "path": &lib_path }), &ctx)
        .await
        .unwrap();
    assert!(read_result["content"].as_str().unwrap().contains("Hello"));

    // Step 3: Replace the "Hello" line with "Greetings" via EditFile
    let replace_result = EditFile
        .call(
            json!({
                "path": &lib_path,
                "old_string": "    format!(\"Hello, {}!\", name)",
                "new_string": "    format!(\"Greetings, {}!\", name)",
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(replace_result, json!("ok"));

    // Step 4: Verify the change
    let read_after = ReadFile
        .call(json!({ "path": &lib_path }), &ctx)
        .await
        .unwrap();
    assert!(read_after["content"]
        .as_str()
        .unwrap()
        .contains("Greetings"));
    assert!(!read_after["content"].as_str().unwrap().contains("Hello"));

    drop(dir);
}

// ---------------------------------------------------------------------------
// Workflow: List functions → Extract docstrings (AST)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_analyze_ast() {
    use codescout::tools::ast::{ListDocs, ListFunctions};

    let (dir, ctx) = project_with_files(&[
        (
            "math.rs",
            "/// Add two numbers.\nfn add(a: i32, b: i32) -> i32 { a + b }\n\n\
             /// Subtract two numbers.\nfn sub(a: i32, b: i32) -> i32 { a - b }\n",
        ),
        (
            "util.py",
            "def helper():\n    \"\"\"A helper function.\"\"\"\n    pass\n",
        ),
    ])
    .await;

    // Step 1: List functions in the Rust file
    let list_result = ListFunctions
        .call(json!({ "path": "math.rs" }), &ctx)
        .await
        .unwrap();
    assert_eq!(list_result["total"], 2);
    let func_names: Vec<&str> = list_result["functions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert!(func_names.contains(&"add"));
    assert!(func_names.contains(&"sub"));

    // Step 2: Extract docstrings
    let doc_result = ListDocs
        .call(json!({ "path": "math.rs" }), &ctx)
        .await
        .unwrap();
    assert_eq!(doc_result["total"], 2);
    let docs = doc_result["docstrings"].as_array().unwrap();
    assert_eq!(docs[0]["symbol_name"], "add");
    assert!(docs[0]["content"].as_str().unwrap().contains("Add two"));

    // Step 3: Also works for Python
    let py_list = ListFunctions
        .call(json!({ "path": "util.py" }), &ctx)
        .await
        .unwrap();
    assert_eq!(py_list["total"], 1);

    let py_docs = ListDocs
        .call(json!({ "path": "util.py" }), &ctx)
        .await
        .unwrap();
    assert!(py_docs["total"].as_u64().unwrap() >= 1);

    drop(dir);
}

// ---------------------------------------------------------------------------
// Workflow: Activate project → Memory roundtrip → Config
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_project_memory_config() {
    use codescout::tools::config::{ActivateProject, ProjectStatus};
    use codescout::tools::memory::{ListMemories, ReadMemory, WriteMemory};

    let (dir, ctx) = project_with_files(&[("src/main.rs", "fn main() {}\n")]).await;

    // Step 1: Activate the project
    let activate_result = ActivateProject
        .call(json!({ "path": dir.path().display().to_string() }), &ctx)
        .await
        .unwrap();
    assert_eq!(activate_result["status"], "ok");

    // Step 2: Get project status
    let config = ProjectStatus.call(json!({}), &ctx).await.unwrap();
    assert!(config["config"].is_object());
    assert!(config["project_root"].is_string());

    // Step 3: Write memory
    WriteMemory
        .call(
            json!({ "topic": "architecture/decisions", "content": "We chose Rust for performance." }),
            &ctx,
        )
        .await
        .unwrap();

    // Step 4: Read it back
    let read = ReadMemory
        .call(json!({ "topic": "architecture/decisions" }), &ctx)
        .await
        .unwrap();
    assert!(read["content"]
        .as_str()
        .unwrap()
        .contains("Rust for performance"));

    // Step 5: List memories
    let list = ListMemories.call(json!({}), &ctx).await.unwrap();
    let topics: Vec<&str> = list["topics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(topics.contains(&"architecture/decisions"));

    drop(dir);
}

// ---------------------------------------------------------------------------
// Workflow: Create file → Git init + add + commit → Blame + Log
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_git_blame() {
    use codescout::tools::file::CreateFile;
    use codescout::tools::git::GitBlame;

    let dir = tempdir().unwrap();

    // Initialize a git repo
    let repo = git2::Repository::init(dir.path()).unwrap();
    std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();

    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    let ctx = ToolContext {
        agent,
        lsp: LspManager::new_arc(),
        output_buffer: std::sync::Arc::new(codescout::tools::output_buffer::OutputBuffer::new(20)),
        progress: None,
    };

    // Step 1: Create a file via tool
    CreateFile
        .call(
            json!({
                "path": dir.path().join("hello.rs").display().to_string(),
                "content": "fn hello() {\n    println!(\"hi\");\n}\n"
            }),
            &ctx,
        )
        .await
        .unwrap();

    // Step 2: Commit it
    {
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("hello.rs")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
            .unwrap();
    }

    // Step 3: Blame
    let blame_result = GitBlame
        .call(json!({ "path": "hello.rs" }), &ctx)
        .await
        .unwrap();
    let lines = blame_result["lines"].as_array().unwrap();
    assert!(!lines.is_empty(), "blame should return lines");
    assert_eq!(lines[0]["author"], "Test");

    drop(dir);
}

// ---------------------------------------------------------------------------
// Workflow: Ollama index → semantic search (requires live Ollama)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires running Ollama with nomic-embed-text"]
async fn workflow_ollama_index_and_search() {
    use codescout::embed::index;
    use codescout::tools::config::ActivateProject;
    use codescout::tools::semantic::{IndexProject, SemanticSearch};

    let (dir, ctx) = project_with_files(&[
        (
            "src/auth.rs",
            "/// Verify a user's password against a stored hash.\n\
             pub fn verify_password(hash: &str, input: &str) -> bool {\n\
             bcrypt::verify(input, hash).unwrap_or(false)\n\
             }\n\
             \n\
             /// Issue a JWT token for the given user ID.\n\
             pub fn issue_jwt(user_id: u64, secret: &str) -> String {\n\
             format!(\"jwt:{}:{}\", user_id, secret)\n\
             }\n",
        ),
        (
            "src/db.rs",
            "use rusqlite::Connection;\n\
             \n\
             /// Open a SQLite connection to the given path.\n\
             pub fn open_db(path: &str) -> Connection {\n\
             Connection::open(path).expect(\"failed to open db\")\n\
             }\n\
             \n\
             /// Insert a new user record.\n\
             pub fn insert_user(conn: &Connection, name: &str, email: &str) {\n\
             conn.execute(\"INSERT INTO users (name, email) VALUES (?1, ?2)\", \
             [name, email]).unwrap();\n\
             }\n",
        ),
        (
            ".code-explorer/project.toml",
            "[project]\nname = \"test\"\n\n\
             [embeddings]\nmodel = \"ollama:nomic-embed-text\"\n",
        ),
    ])
    .await;

    // Activate project so tools know the root
    ActivateProject
        .call(json!({ "path": dir.path().display().to_string() }), &ctx)
        .await
        .unwrap();

    // Index the project
    let index_result = IndexProject.call(json!({}), &ctx).await.unwrap();
    assert_eq!(index_result["status"], "ok");
    let files_indexed = index_result["files_indexed"].as_u64().unwrap();
    assert!(
        files_indexed >= 2,
        "expected at least auth.rs and db.rs indexed"
    );

    // Search for authentication-related code
    let auth_results = SemanticSearch
        .call(
            json!({ "query": "password verification authentication", "limit": 5 }),
            &ctx,
        )
        .await
        .unwrap();
    let hits = auth_results["results"].as_array().unwrap();
    assert!(!hits.is_empty(), "expected at least one result");

    // The top result should be from auth.rs (it contains password/auth code)
    let top_hit = &hits[0];
    assert!(
        top_hit["file_path"].as_str().unwrap().contains("auth"),
        "top result for 'password verification' should be auth.rs, got: {:?}",
        top_hit["file_path"]
    );
    assert!(
        top_hit["score"].as_f64().unwrap() > 0.5,
        "score should be reasonably high"
    );

    // Search for database code
    let db_results = SemanticSearch
        .call(
            json!({ "query": "open database connection sqlite", "limit": 5 }),
            &ctx,
        )
        .await
        .unwrap();
    let db_hits = db_results["results"].as_array().unwrap();
    assert!(!db_hits.is_empty());
    assert!(
        db_hits[0]["file_path"].as_str().unwrap().contains("db"),
        "top result for 'sqlite database' should be db.rs, got: {:?}",
        db_hits[0]["file_path"]
    );

    // Verify the index is queryable without re-indexing (incremental: force=false skips unchanged)
    let conn = index::open_db(dir.path()).unwrap();
    let stats = index::index_stats(&conn).unwrap();
    assert!(stats.chunk_count > 0);
    assert_eq!(
        stats.file_count,
        stats.embedding_count.min(files_indexed as usize)
    );

    drop(dir);
}

// ---------------------------------------------------------------------------
// Workflow: Onboarding → List dir
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_onboarding_explore() {
    use codescout::tools::file::ListDir;
    use codescout::tools::workflow::Onboarding;

    let (dir, ctx) = project_with_files(&[
        ("src/main.rs", "fn main() {}\n"),
        ("src/lib.rs", "pub mod utils;\n"),
        ("Cargo.toml", "[package]\nname = \"test\"\n"),
    ])
    .await;

    // Step 1: Run onboarding (first call does full discovery)
    let onboard = Onboarding.call(json!({}), &ctx).await.unwrap();
    assert!(onboard["languages"].is_array());

    // Step 2: Calling again returns status (already onboarded)
    let status = Onboarding.call(json!({}), &ctx).await.unwrap();
    assert_eq!(status["onboarded"], true);

    // Step 3: List directory

    // Step 3: List directory
    let list = ListDir
        .call(json!({ "path": dir.path().display().to_string() }), &ctx)
        .await
        .unwrap();
    let entries = list["entries"].as_array().unwrap();
    let entry_strs: Vec<&str> = entries.iter().filter_map(|e| e.as_str()).collect();
    // Entries are full paths, check that src/ and Cargo.toml appear
    assert!(
        entry_strs
            .iter()
            .any(|e| e.contains("src") && e.ends_with('/')),
        "missing src dir: {:?}",
        entry_strs
    );
    assert!(
        entry_strs.iter().any(|e| e.ends_with("Cargo.toml")),
        "missing Cargo.toml: {:?}",
        entry_strs
    );

    drop(dir);
}

/// Exercises find_symbol with file, directory, nested directory, glob, and
/// name_path patterns — all via tree-sitter fallback (no LSP).
#[tokio::test]
async fn workflow_find_symbol_path_types() {
    use codescout::tools::symbol::FindSymbol;

    let (_dir, ctx) = project_with_files(&[
        (
            "src/main.rs",
            "fn main() {}\nfn add(a: i32, b: i32) -> i32 { a + b }\n",
        ),
        (
            "src/lib.rs",
            "pub fn helper() -> bool { true }\npub struct Calculator;\nimpl Calculator { pub fn compute() -> i32 { 42 } }\n",
        ),
        (
            "src/utils/math.rs",
            "pub fn multiply(a: i32, b: i32) -> i32 { a * b }\n",
        ),
    ])
    .await;

    // 1. File path — baseline
    let r = FindSymbol
        .call(
            json!({ "pattern": "add", "relative_path": "src/main.rs" }),
            &ctx,
        )
        .await
        .unwrap();
    let syms = r["symbols"].as_array().unwrap();
    assert!(
        syms.iter().any(|s| s["name"] == "add"),
        "file path should find 'add': {r:?}"
    );

    // 2. Directory path — bug #1 regression
    let r = FindSymbol
        .call(json!({ "pattern": "helper", "relative_path": "src" }), &ctx)
        .await
        .unwrap();
    let syms = r["symbols"].as_array().unwrap();
    assert!(
        syms.iter().any(|s| s["name"] == "helper"),
        "directory path should find 'helper': {r:?}"
    );

    // 3. Nested directory path — bug #1 nested
    let r = FindSymbol
        .call(
            json!({ "pattern": "multiply", "relative_path": "src/utils" }),
            &ctx,
        )
        .await
        .unwrap();
    let syms = r["symbols"].as_array().unwrap();
    assert!(
        syms.iter().any(|s| s["name"] == "multiply"),
        "nested directory path should find 'multiply': {r:?}"
    );

    // 4. Glob path
    let r = FindSymbol
        .call(
            json!({ "pattern": "add", "relative_path": "src/**/*.rs" }),
            &ctx,
        )
        .await
        .unwrap();
    let syms = r["symbols"].as_array().unwrap();
    assert!(
        syms.iter().any(|s| s["name"] == "add"),
        "glob path should find 'add': {r:?}"
    );

    // 5. Name_path pattern project-wide — bug #2 regression
    // tree-sitter uses "Calculator/compute" as name_path (no "impl" prefix)
    let r = FindSymbol
        .call(json!({ "pattern": "Calculator/compute" }), &ctx)
        .await
        .unwrap();
    let syms = r["symbols"].as_array().unwrap();
    assert!(
        syms.iter().any(|s| s["name"] == "compute"),
        "name_path pattern should find 'compute' project-wide: {r:?}"
    );
}

#[tokio::test]
async fn write_allowed_when_project_provided_at_startup_even_with_worktrees() {
    use codescout::tools::file::CreateFile;

    // 1. Create a temp project dir with fake worktree metadata
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();

    // Simulate a linked worktree: .git/worktrees/feat/gitdir pointing to some path
    let wt_entry = dir.path().join(".git").join("worktrees").join("feat");
    std::fs::create_dir_all(&wt_entry).unwrap();
    let fake_wt_root = dir.path().join("..").join("my-worktree");
    std::fs::create_dir_all(&fake_wt_root).unwrap();
    let gitdir_content = format!("{}/.git\n", fake_wt_root.display());
    std::fs::write(wt_entry.join("gitdir"), &gitdir_content).unwrap();

    // 2. Create Agent via new(Some(path)) — project_explicitly_activated is now true
    //    (the server operator already chose the write target at startup)
    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    assert!(agent.is_project_explicitly_activated().await);
    let ctx = ToolContext {
        agent,
        lsp: LspManager::new_arc(),
        output_buffer: std::sync::Arc::new(codescout::tools::output_buffer::OutputBuffer::new(20)),
        progress: None,
    };

    // 3. Write should succeed — worktree guard bypassed because project was
    //    explicitly activated at startup
    let result = CreateFile
        .call(json!({ "path": "test.txt", "content": "hello" }), &ctx)
        .await;
    assert!(result.is_ok(), "expected write to succeed, got: {result:?}");

    drop(dir);
}

// ---------------------------------------------------------------------------
// Workflow: run_command large output → buffer → grep via buffer ref
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn integration_run_command_buffer_round_trip() {
    use codescout::tools::workflow::RunCommand;

    let (dir, ctx) = project_with_files(&[("README.md", "# test\n")]).await;

    // Generate > 50 lines so output gets buffered
    let r1 = RunCommand
        .call(json!({ "command": "seq 1 100", "timeout_secs": 10 }), &ctx)
        .await
        .unwrap();
    let output_id = r1["output_id"]
        .as_str()
        .expect("seq 1 100 (100 lines) should be buffered and return output_id");
    assert!(
        output_id.starts_with("@cmd_"),
        "output_id should start with @cmd_, got: {}",
        output_id
    );

    // Query the buffer ref with grep
    let r2 = RunCommand
        .call(
            json!({
                "command": format!("grep '^50$' {}", output_id),
                "timeout_secs": 10
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(r2["exit_code"], 0, "grep should succeed: {r2:?}");
    assert_eq!(
        r2["stdout"].as_str().unwrap().trim(),
        "50",
        "grep result should be '50': {r2:?}"
    );

    drop(dir);
}

// ---------------------------------------------------------------------------
// Workflow: read_file on large file → file_id buffer → grep via buffer ref
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn integration_read_file_large_then_query_via_buffer() {
    use codescout::tools::file::ReadFile;
    use codescout::tools::workflow::RunCommand;

    // Build a file with 250 lines (above the 200-line FILE_BUFFER_THRESHOLD)
    let content: String = (1..=250).map(|i| format!("entry {}\n", i)).collect();
    let (dir, ctx) = project_with_files(&[("big.txt", &content)]).await;
    let path = dir.path().join("big.txt").display().to_string();

    let r1 = ReadFile.call(json!({ "path": &path }), &ctx).await.unwrap();
    let file_id = r1["file_id"]
        .as_str()
        .expect("250-line file should be buffered and return file_id");
    assert!(
        file_id.starts_with("@file_"),
        "file_id should start with @file_, got: {}",
        file_id
    );

    // Query the buffer with grep
    let r2 = RunCommand
        .call(
            json!({
                "command": format!("grep 'entry 200' {}", file_id),
                "timeout_secs": 10
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(r2["exit_code"], 0, "grep should succeed: {r2:?}");
    assert!(
        r2["stdout"].as_str().unwrap().contains("entry 200"),
        "grep result should contain 'entry 200': {r2:?}"
    );

    drop(dir);
}

// ---------------------------------------------------------------------------
// Workflow: dangerous command → speed bump → acknowledged execution
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn integration_speed_bump_two_round_trips() {
    use codescout::tools::workflow::RunCommand;

    let (dir, ctx) = project_with_files(&[("README.md", "# test\n")]).await;

    // First call: dangerous command should return a pending_ack handle, not run immediately
    let r1 = RunCommand
        .call(
            json!({ "command": "rm -rf /tmp/ce_integration_test_nonexistent_dir" }),
            &ctx,
        )
        .await
        .expect("dangerous command should return Ok(pending_ack), not Err");

    let handle = r1["pending_ack"]
        .as_str()
        .expect("result should have pending_ack string field");
    assert!(
        handle.starts_with("@ack_"),
        "handle should start with @ack_, got: {handle}"
    );
    assert!(
        r1.get("reason").is_some(),
        "result should include reason, got: {r1}"
    );
    assert!(
        r1["hint"].as_str().unwrap_or("").contains("@ack_"),
        "hint should reference the ack handle, got: {r1}"
    );

    // Second call: submit the @ack_* handle → command actually executes
    let r2 = RunCommand.call(json!({ "command": handle }), &ctx).await;

    match &r2 {
        Ok(v) => {
            // Should have exit_code, not another pending_ack
            assert!(
                v.get("pending_ack").is_none(),
                "ack'd command should not re-block, got: {v}"
            );
        }
        Err(e) => {
            // Acceptable only if it is NOT a dangerous-block error
            let msg = e.to_string().to_lowercase();
            assert!(
                !msg.contains("dangerous"),
                "ack'd command should not be blocked as dangerous, got: {msg}"
            );
        }
    }

    drop(dir);
}
