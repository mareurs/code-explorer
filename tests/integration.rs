//! Integration tests: multi-tool workflows through the server handler.
//!
//! These tests exercise realistic tool sequences that a coding agent would
//! perform, ensuring tools compose correctly end-to-end.

use std::sync::Arc;

use code_explorer::agent::Agent;
use code_explorer::lsp::LspManager;
use code_explorer::tools::{Tool, ToolContext};
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
        lsp: Arc::new(LspManager::new()),
    };
    (dir, ctx)
}

// ---------------------------------------------------------------------------
// Workflow: Read → Search → Replace
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_read_search_replace() {
    use code_explorer::tools::file::{EditLines, ReadFile, SearchPattern};
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

    // Step 3: Replace line 2 containing "Hello" with "Greetings" via EditLines
    let replace_result = EditLines
        .call(
            json!({
                "path": &lib_path,
                "start_line": 2,
                "delete_count": 1,
                "new_text": "    format!(\"Greetings, {}!\", name)\n",
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(replace_result["status"], "ok");

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
    use code_explorer::tools::ast::{ListDocs, ListFunctions};

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
    use code_explorer::tools::config::{ActivateProject, GetConfig};
    use code_explorer::tools::memory::{ListMemories, ReadMemory, WriteMemory};

    let (dir, ctx) = project_with_files(&[("src/main.rs", "fn main() {}\n")]).await;

    // Step 1: Activate the project
    let activate_result = ActivateProject
        .call(json!({ "path": dir.path().display().to_string() }), &ctx)
        .await
        .unwrap();
    assert_eq!(activate_result["status"], "ok");

    // Step 2: Get config
    let config = GetConfig.call(json!({}), &ctx).await.unwrap();
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
    use code_explorer::tools::file::CreateFile;
    use code_explorer::tools::git::GitBlame;

    let dir = tempdir().unwrap();

    // Initialize a git repo
    let repo = git2::Repository::init(dir.path()).unwrap();
    std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();

    let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
    let ctx = ToolContext {
        agent,
        lsp: Arc::new(LspManager::new()),
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
    use code_explorer::embed::index;
    use code_explorer::tools::config::ActivateProject;
    use code_explorer::tools::semantic::{IndexProject, SemanticSearch};

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
    use code_explorer::tools::file::ListDir;
    use code_explorer::tools::workflow::Onboarding;

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
    use code_explorer::tools::symbol::FindSymbol;

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
    use code_explorer::tools::file::CreateFile;

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
        lsp: Arc::new(LspManager::new()),
    };

    // 3. Write should succeed — worktree guard bypassed because project was
    //    explicitly activated at startup
    let result = CreateFile
        .call(json!({ "path": "test.txt", "content": "hello" }), &ctx)
        .await;
    assert!(result.is_ok(), "expected write to succeed, got: {result:?}");

    drop(dir);
}
