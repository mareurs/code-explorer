# Memory Staleness Detection — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Detect when code changes make project memories stale, surfaced via `project_status`, using path anchors (TOML sidecars) and semantic anchors (embedding similarity).

**Architecture:** Each markdown memory gets a `.anchors.toml` sidecar listing source file dependencies with SHA-256 hashes. Semantic anchors (automatic, embedding-based) are stored in `memory_anchors` table in `embeddings.db`. `project_status` checks both layers. `build_index` piggybacks reverse-drift flags on high-drift files.

**Tech Stack:** Rust, toml/serde for sidecar I/O, rusqlite for `memory_anchors` table, existing `embed::index` and `embed::drift` infrastructure.

**Design doc:** `docs/plans/2026-03-06-memory-staleness-detection-design.md`

---

### Task 1: TOML Sidecar Data Model — `src/memory/anchors.rs`

**Files:**
- Create: `src/memory/anchors.rs`
- Modify: `src/memory/mod.rs` (add `pub mod anchors;`)

**Step 1: Write the failing test**

In `src/memory/anchors.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_anchor_file() {
        let dir = tempdir().unwrap();
        let anchors_path = dir.path().join("architecture.anchors.toml");

        let anchors = AnchorFile {
            anchors: vec![
                PathAnchor {
                    path: "src/server.rs".to_string(),
                    hash: "abc123".to_string(),
                },
                PathAnchor {
                    path: "src/tools/mod.rs".to_string(),
                    hash: "def456".to_string(),
                },
            ],
        };

        write_anchor_file(&anchors_path, &anchors).unwrap();
        let loaded = read_anchor_file(&anchors_path).unwrap();
        assert_eq!(loaded.anchors.len(), 2);
        assert_eq!(loaded.anchors[0].path, "src/server.rs");
        assert_eq!(loaded.anchors[0].hash, "abc123");
    }

    #[test]
    fn read_missing_returns_empty() {
        let dir = tempdir().unwrap();
        let anchors_path = dir.path().join("nonexistent.anchors.toml");
        let loaded = read_anchor_file(&anchors_path).unwrap();
        assert!(loaded.anchors.is_empty());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test anchors::tests --lib -- --nocapture`
Expected: FAIL — module doesn't exist yet.

**Step 3: Write minimal implementation**

In `src/memory/anchors.rs`:

```rust
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PathAnchor {
    pub path: String,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnchorFile {
    #[serde(default)]
    pub anchors: Vec<PathAnchor>,
}

/// Read an anchor TOML sidecar. Returns empty AnchorFile if file doesn't exist.
pub fn read_anchor_file(path: &Path) -> Result<AnchorFile> {
    if !path.exists() {
        return Ok(AnchorFile::default());
    }
    let content = std::fs::read_to_string(path)?;
    let af: AnchorFile = toml::from_str(&content)?;
    Ok(af)
}

/// Write an anchor TOML sidecar with a helpful header comment.
pub fn write_anchor_file(path: &Path, anchor_file: &AnchorFile) -> Result<()> {
    let header = "# Source files this memory depends on.\n\
                  # Edit this list to track additional files or remove irrelevant ones.\n\
                  # codescout will warn when these files change significantly.\n\n";
    let body = toml::to_string_pretty(anchor_file)?;
    std::fs::write(path, format!("{header}{body}"))?;
    Ok(())
}
```

In `src/memory/mod.rs`, add near the top:

```rust
pub mod anchors;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test anchors::tests --lib`
Expected: PASS (both tests).

**Step 5: Commit**

```bash
git add src/memory/anchors.rs src/memory/mod.rs
git commit -m "feat(memory): anchor TOML sidecar read/write"
```

---

### Task 2: Path Extraction from Memory Content

**Files:**
- Modify: `src/memory/anchors.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn extract_paths_from_content() {
    let content = "## Key Abstractions\n\
                   | `Tool` trait | `src/tools/mod.rs:228` | Core tool abstraction |\n\
                   | `OutputGuard` | `src/tools/output.rs` | Progressive disclosure |\n\
                   See also `Cargo.toml` and `docs/ARCHITECTURE.md`.\n\
                   Not a path: src without extension or random text.";
    let paths = extract_paths(content);
    assert!(paths.contains(&"src/tools/mod.rs".to_string()));
    assert!(paths.contains(&"src/tools/output.rs".to_string()));
    assert!(paths.contains(&"Cargo.toml".to_string()));
    assert!(paths.contains(&"docs/ARCHITECTURE.md".to_string()));
    // Should not contain line-number suffixed version as a separate entry
    assert!(!paths.contains(&"src/tools/mod.rs:228".to_string()));
}

#[test]
fn extract_paths_deduplicates() {
    let content = "See `src/server.rs` and also `src/server.rs` again.";
    let paths = extract_paths(content);
    assert_eq!(paths.len(), 1);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test anchors::tests::extract_paths --lib`
Expected: FAIL — function doesn't exist.

**Step 3: Write minimal implementation**

```rust
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

static PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[`\s\|(])((src/[\w/._-]+\.\w+|\.codescout/[\w/._-]+\.\w+|Cargo\.toml|CLAUDE\.md|docs/[\w/._-]+\.\w+))"
    ).unwrap()
});

/// Extract file paths mentioned in memory content. Deduplicates and strips line-number suffixes.
pub fn extract_paths(content: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for cap in PATH_RE.captures_iter(content) {
        let mut path = cap[2].to_string();
        // Strip :line_number suffix (e.g. "src/tools/mod.rs:228" → "src/tools/mod.rs")
        if let Some(colon_pos) = path.rfind(':') {
            if path[colon_pos + 1..].chars().all(|c| c.is_ascii_digit()) {
                path.truncate(colon_pos);
            }
        }
        if seen.insert(path.clone()) {
            result.push(path);
        }
    }
    result
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test anchors::tests::extract_paths --lib`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/memory/anchors.rs
git commit -m "feat(memory): extract file paths from memory content"
```

---

### Task 3: Anchor Seeding and Merging on Memory Write

**Files:**
- Modify: `src/memory/anchors.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn seed_anchors_only_for_existing_files() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    // Create one real file
    std::fs::create_dir_all(root.join("src/tools")).unwrap();
    std::fs::write(root.join("src/tools/mod.rs"), "fn main() {}").unwrap();

    let content = "Uses `src/tools/mod.rs` and `src/nonexistent.rs`.";
    let anchors = seed_anchors(root, content).unwrap();
    assert_eq!(anchors.anchors.len(), 1);
    assert_eq!(anchors.anchors[0].path, "src/tools/mod.rs");
    assert!(!anchors.anchors[0].hash.is_empty());
}

#[test]
fn merge_preserves_user_added_paths() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "a").unwrap();
    std::fs::write(root.join("src/b.rs"), "b").unwrap();
    std::fs::write(root.join("src/c.rs"), "c").unwrap();

    // Existing sidecar has user-added path src/b.rs (not in content)
    let existing = AnchorFile {
        anchors: vec![
            PathAnchor { path: "src/a.rs".into(), hash: "old_hash".into() },
            PathAnchor { path: "src/b.rs".into(), hash: "user_added".into() },
        ],
    };
    // New content only mentions src/a.rs and src/c.rs
    let new_seed = seed_anchors(root, "Uses `src/a.rs` and `src/c.rs`.").unwrap();
    let merged = merge_anchors(root, &existing, &new_seed).unwrap();

    let paths: Vec<&str> = merged.anchors.iter().map(|a| a.path.as_str()).collect();
    assert!(paths.contains(&"src/a.rs"));  // kept, hash refreshed
    assert!(paths.contains(&"src/b.rs"));  // user-added, preserved
    assert!(paths.contains(&"src/c.rs"));  // newly seeded
    // Hash for src/a.rs should be refreshed (not "old_hash")
    let a = merged.anchors.iter().find(|a| a.path == "src/a.rs").unwrap();
    assert_ne!(a.hash, "old_hash");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test anchors::tests::seed_anchors --lib`
Expected: FAIL — functions don't exist.

**Step 3: Write minimal implementation**

```rust
use crate::embed::index::hash_file as compute_hash;

/// Seed anchors from memory content. Only includes files that exist on disk.
/// Computes current SHA-256 hash for each.
pub fn seed_anchors(project_root: &Path, content: &str) -> Result<AnchorFile> {
    let paths = extract_paths(content);
    let mut anchors = Vec::new();
    for p in paths {
        let full = project_root.join(&p);
        if full.is_file() {
            let hash = compute_hash(&full)?;
            anchors.push(PathAnchor { path: p, hash });
        }
    }
    Ok(AnchorFile { anchors })
}

/// Merge existing sidecar with newly seeded anchors.
/// - Keeps user-added paths (in existing but not in new seed)
/// - Adds newly seeded paths
/// - Refreshes hashes for all paths that exist on disk
pub fn merge_anchors(
    project_root: &Path,
    existing: &AnchorFile,
    new_seed: &AnchorFile,
) -> Result<AnchorFile> {
    let mut seen = HashSet::new();
    let mut anchors = Vec::new();

    // Start with all new-seed paths (hashes already fresh)
    for a in &new_seed.anchors {
        if seen.insert(a.path.clone()) {
            anchors.push(a.clone());
        }
    }

    // Add user-curated paths not in the new seed, refresh their hashes
    for a in &existing.anchors {
        if seen.insert(a.path.clone()) {
            let full = project_root.join(&a.path);
            if full.is_file() {
                let hash = compute_hash(&full)?;
                anchors.push(PathAnchor {
                    path: a.path.clone(),
                    hash,
                });
            }
            // If file no longer exists, silently drop it
        }
    }

    Ok(AnchorFile { anchors })
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test anchors::tests --lib`
Expected: PASS (all tests).

**Step 5: Commit**

```bash
git add src/memory/anchors.rs
git commit -m "feat(memory): anchor seeding from content and merge logic"
```

---

### Task 4: Path Staleness Check

**Files:**
- Modify: `src/memory/anchors.rs`

**Step 1: Write the failing test**

```rust
#[derive(Debug, PartialEq)]
enum AnchorStatus {
    Fresh,
    Changed,
    Deleted,
}

#[test]
fn check_staleness_detects_changes() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "version 1").unwrap();

    // Create anchors with current hash
    let anchors = seed_anchors(root, "Uses `src/a.rs`.").unwrap();
    let original_hash = anchors.anchors[0].hash.clone();

    // Check before any changes — should be fresh
    let report = check_path_staleness(root, &anchors).unwrap();
    assert!(report.stale_files.is_empty());

    // Modify the file
    std::fs::write(root.join("src/a.rs"), "version 2").unwrap();
    let report = check_path_staleness(root, &anchors).unwrap();
    assert_eq!(report.stale_files.len(), 1);
    assert_eq!(report.stale_files[0].path, "src/a.rs");
    assert_eq!(report.stale_files[0].status, AnchorStatus::Changed);

    // Delete the file
    std::fs::remove_file(root.join("src/a.rs")).unwrap();
    let report = check_path_staleness(root, &anchors).unwrap();
    assert_eq!(report.stale_files[0].status, AnchorStatus::Deleted);
}

#[test]
fn check_staleness_all_fresh() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "stable").unwrap();

    let anchors = seed_anchors(root, "Uses `src/a.rs`.").unwrap();
    let report = check_path_staleness(root, &anchors).unwrap();
    assert!(report.stale_files.is_empty());
    assert!(report.is_fresh());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test anchors::tests::check_staleness --lib`
Expected: FAIL — types and function don't exist.

**Step 3: Write minimal implementation**

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum AnchorStatus {
    Changed,
    Deleted,
}

#[derive(Debug, Clone)]
pub struct StaleFile {
    pub path: String,
    pub status: AnchorStatus,
}

#[derive(Debug)]
pub struct StalenessReport {
    pub stale_files: Vec<StaleFile>,
}

impl StalenessReport {
    pub fn is_fresh(&self) -> bool {
        self.stale_files.is_empty()
    }
}

/// Check path anchors against current file state.
/// Returns which anchored files have changed or been deleted.
pub fn check_path_staleness(project_root: &Path, anchor_file: &AnchorFile) -> Result<StalenessReport> {
    let mut stale_files = Vec::new();
    for anchor in &anchor_file.anchors {
        let full = project_root.join(&anchor.path);
        if !full.exists() {
            stale_files.push(StaleFile {
                path: anchor.path.clone(),
                status: AnchorStatus::Deleted,
            });
        } else {
            let current_hash = compute_hash(&full)?;
            if current_hash != anchor.hash {
                stale_files.push(StaleFile {
                    path: anchor.path.clone(),
                    status: AnchorStatus::Changed,
                });
            }
        }
    }
    Ok(StalenessReport { stale_files })
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test anchors::tests::check_staleness --lib`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/memory/anchors.rs
git commit -m "feat(memory): path anchor staleness check"
```

---

### Task 5: Wire Anchor Creation into Memory Write Path

**Files:**
- Modify: `src/tools/memory.rs` (the `"write"` action in `Memory::call`)
- Modify: `src/memory/anchors.rs` (add `anchor_path_for_topic` helper)

**Step 1: Write the failing test**

In `src/tools/memory.rs` tests:

```rust
#[tokio::test]
async fn write_creates_anchor_sidecar() {
    let (ctx, dir) = test_ctx_with_project();
    // Create a source file in the temp project
    std::fs::create_dir_all(dir.path().join("src/tools")).unwrap();
    std::fs::write(dir.path().join("src/tools/mod.rs"), "pub fn tool() {}").unwrap();

    let input = json!({
        "action": "write",
        "topic": "architecture",
        "content": "## Tools\nThe tool trait lives in `src/tools/mod.rs`."
    });
    let result = Memory.call(input, &ctx).await.unwrap();
    assert_eq!(result, json!("ok"));

    // Check sidecar was created
    let sidecar = dir.path().join(".codescout/memories/architecture.anchors.toml");
    assert!(sidecar.exists(), "anchor sidecar should be created");
    let af = crate::memory::anchors::read_anchor_file(&sidecar).unwrap();
    assert_eq!(af.anchors.len(), 1);
    assert_eq!(af.anchors[0].path, "src/tools/mod.rs");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test memory::tests::write_creates_anchor_sidecar --lib`
Expected: FAIL — sidecar not created yet.

**Step 3: Write minimal implementation**

In `src/memory/anchors.rs`, add helper:

```rust
/// Get the anchor sidecar path for a given memory topic within a memories directory.
pub fn anchor_path_for_topic(memories_dir: &Path, topic: &str) -> std::path::PathBuf {
    memories_dir.join(format!("{}.anchors.toml", topic))
}

/// Seed or merge anchors for a memory topic after a write.
pub fn update_anchors_on_write(
    project_root: &Path,
    memories_dir: &Path,
    topic: &str,
    content: &str,
) -> Result<()> {
    let sidecar_path = anchor_path_for_topic(memories_dir, topic);
    let existing = read_anchor_file(&sidecar_path)?;
    let new_seed = seed_anchors(project_root, content)?;

    let merged = if existing.anchors.is_empty() {
        new_seed
    } else {
        merge_anchors(project_root, &existing, &new_seed)?
    };

    // Only write sidecar if there are anchors to track
    if !merged.anchors.is_empty() {
        write_anchor_file(&sidecar_path, &merged)?;
    }
    Ok(())
}
```

In `src/tools/memory.rs`, in the `"write"` action, after `cross_embed_memory` and before the final `Ok(json!("ok"))`:

```rust
// Seed/merge path anchors (best-effort, non-fatal)
if !private {
    if let Ok(root) = ctx.agent.require_project_root().await {
        let memories_dir = root.join(".codescout").join("memories");
        if let Err(e) = crate::memory::anchors::update_anchors_on_write(
            &root, &memories_dir, topic, content,
        ) {
            tracing::debug!("anchor update failed (non-fatal): {e}");
        }
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test memory::tests::write_creates_anchor_sidecar --lib`
Expected: PASS.

Then run full suite: `cargo test --lib`

**Step 5: Commit**

```bash
git add src/memory/anchors.rs src/tools/memory.rs
git commit -m "feat(memory): wire anchor creation into memory write path"
```

---

### Task 6: Wire Staleness Check into `project_status`

**Files:**
- Modify: `src/tools/config.rs` (`ProjectStatus::call`)

**Step 1: Write the failing test**

In `src/tools/config.rs` tests:

```rust
#[tokio::test]
async fn project_status_includes_memory_staleness() {
    let (ctx, dir) = test_ctx_with_project();

    // Create a memory with an anchor
    let memories_dir = dir.path().join(".codescout/memories");
    std::fs::create_dir_all(&memories_dir).unwrap();
    std::fs::write(memories_dir.join("architecture.md"), "# Arch").unwrap();

    // Create anchored file and sidecar
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/server.rs"), "fn main() {}").unwrap();

    let anchors = crate::memory::anchors::seed_anchors(
        dir.path(), "Uses `src/server.rs`."
    ).unwrap();
    crate::memory::anchors::write_anchor_file(
        &memories_dir.join("architecture.anchors.toml"), &anchors
    ).unwrap();

    // Before change — should be fresh
    let result = ProjectStatus.call(json!({}), &ctx).await.unwrap();
    let staleness = &result["memory_staleness"];
    assert!(staleness["stale"].as_array().unwrap().is_empty());
    assert!(staleness["fresh"].as_array().unwrap().contains(&json!("architecture")));

    // Modify the anchored file
    std::fs::write(dir.path().join("src/server.rs"), "fn changed() {}").unwrap();

    let result = ProjectStatus.call(json!({}), &ctx).await.unwrap();
    let staleness = &result["memory_staleness"];
    let stale = staleness["stale"].as_array().unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0]["topic"], "architecture");
    assert!(stale[0]["changed_files"].as_array().unwrap().contains(&json!("src/server.rs")));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test config::tests::project_status_includes_memory_staleness --lib`
Expected: FAIL — no `memory_staleness` key in output.

**Step 3: Write minimal implementation**

Add a helper function in `src/memory/anchors.rs`:

```rust
use serde_json::{json, Value};

/// Check all memory topics in a memories directory for staleness.
/// Returns a JSON value with { stale: [...], fresh: [...], untracked: [...] }.
pub fn check_all_memories(project_root: &Path, memories_dir: &Path) -> Result<Value> {
    let mut stale = Vec::new();
    let mut fresh = Vec::new();
    let mut untracked = Vec::new();

    // List all .md files in memories_dir (non-recursive for now)
    if !memories_dir.exists() {
        return Ok(json!({ "stale": stale, "fresh": fresh, "untracked": untracked }));
    }

    for entry in std::fs::read_dir(memories_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "md") {
            let topic = path.file_stem().unwrap().to_string_lossy().to_string();
            let sidecar = anchor_path_for_topic(memories_dir, &topic);

            if !sidecar.exists() {
                untracked.push(topic);
                continue;
            }

            let anchor_file = read_anchor_file(&sidecar)?;
            let report = check_path_staleness(project_root, &anchor_file)?;

            if report.is_fresh() {
                fresh.push(json!(topic));
            } else {
                let changed: Vec<&str> = report.stale_files.iter()
                    .filter(|f| f.status == AnchorStatus::Changed)
                    .map(|f| f.path.as_str())
                    .collect();
                let deleted: Vec<&str> = report.stale_files.iter()
                    .filter(|f| f.status == AnchorStatus::Deleted)
                    .map(|f| f.path.as_str())
                    .collect();
                let total_anchored = anchor_file.anchors.len();
                let total_stale = report.stale_files.len();
                let mut entry = json!({
                    "topic": topic,
                    "reason": format!("{} of {} anchored files changed", total_stale, total_anchored),
                });
                if !changed.is_empty() {
                    entry["changed_files"] = json!(changed);
                }
                if !deleted.is_empty() {
                    entry["deleted_files"] = json!(deleted);
                }
                stale.push(entry);
            }
        }
    }

    // Sort for deterministic output
    fresh.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    untracked.sort();

    Ok(json!({
        "stale": stale,
        "fresh": fresh,
        "untracked": untracked,
    }))
}
```

In `src/tools/config.rs`, in `ProjectStatus::call`, after the index section and before `Ok(result)`, add:

```rust
// --- Memory staleness section ---
let staleness_result = ctx.agent.with_project(|p| {
    let memories_dir = p.root.join(".codescout").join("memories");
    crate::memory::anchors::check_all_memories(&p.root, &memories_dir)
}).await;
match staleness_result {
    Ok(staleness) => { result["memory_staleness"] = staleness; }
    Err(e) => {
        tracing::debug!("memory staleness check failed: {e}");
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test config::tests::project_status_includes_memory_staleness --lib`
Expected: PASS.

Then: `cargo test --lib`

**Step 5: Commit**

```bash
git add src/memory/anchors.rs src/tools/config.rs
git commit -m "feat: wire memory staleness into project_status"
```

---

### Task 7: `refresh_anchors` Action on Memory Tool

**Files:**
- Modify: `src/tools/memory.rs` (add `"refresh_anchors"` match arm)
- Modify: `src/memory/anchors.rs` (add `refresh_hashes` function)

**Step 1: Write the failing test**

In `src/tools/memory.rs` tests:

```rust
#[tokio::test]
async fn refresh_anchors_clears_staleness() {
    let (ctx, dir) = test_ctx_with_project();
    let memories_dir = dir.path().join(".codescout/memories");
    std::fs::create_dir_all(&memories_dir).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.rs"), "v1").unwrap();

    // Write memory to create sidecar
    Memory.call(json!({
        "action": "write",
        "topic": "test-topic",
        "content": "References `src/a.rs`."
    }), &ctx).await.unwrap();

    // Modify file to make it stale
    std::fs::write(dir.path().join("src/a.rs"), "v2").unwrap();

    // Verify it's stale
    let report = crate::memory::anchors::check_path_staleness(
        dir.path(),
        &crate::memory::anchors::read_anchor_file(
            &memories_dir.join("test-topic.anchors.toml")
        ).unwrap(),
    ).unwrap();
    assert!(!report.is_fresh());

    // Refresh anchors
    let result = Memory.call(json!({
        "action": "refresh_anchors",
        "topic": "test-topic"
    }), &ctx).await.unwrap();
    assert_eq!(result, json!("ok"));

    // Verify it's fresh now
    let report = crate::memory::anchors::check_path_staleness(
        dir.path(),
        &crate::memory::anchors::read_anchor_file(
            &memories_dir.join("test-topic.anchors.toml")
        ).unwrap(),
    ).unwrap();
    assert!(report.is_fresh());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test memory::tests::refresh_anchors_clears_staleness --lib`
Expected: FAIL — unknown action.

**Step 3: Write minimal implementation**

In `src/memory/anchors.rs`:

```rust
/// Re-hash all anchored files without changing the anchor list.
/// Used to acknowledge "I reviewed this memory, it's still accurate."
pub fn refresh_hashes(project_root: &Path, memories_dir: &Path, topic: &str) -> Result<()> {
    let sidecar_path = anchor_path_for_topic(memories_dir, topic);
    let mut anchor_file = read_anchor_file(&sidecar_path)?;

    // Re-hash existing paths, remove entries for deleted files
    anchor_file.anchors.retain_mut(|a| {
        let full = project_root.join(&a.path);
        if let Ok(hash) = compute_hash(&full) {
            a.hash = hash;
            true
        } else {
            false // file deleted, drop anchor
        }
    });

    write_anchor_file(&sidecar_path, &anchor_file)?;
    Ok(())
}
```

In `src/tools/memory.rs`, add the match arm before the `_ =>` fallback in `Memory::call`:

```rust
"refresh_anchors" => {
    let topic = super::require_str_param(&input, "topic")?;
    let root = ctx.agent.require_project_root().await?;
    let memories_dir = root.join(".codescout").join("memories");

    // Check that the memory topic exists
    let topic_path = memories_dir.join(format!("{}.md", topic));
    if !topic_path.exists() {
        return Err(RecoverableError::with_hint(
            format!("topic '{}' not found", topic),
            "Use memory(action='list') to see available topics",
        ).into());
    }

    crate::memory::anchors::refresh_hashes(&root, &memories_dir, topic)?;
    Ok(json!("ok"))
}
```

Update the error message for unknown actions to include `refresh_anchors`:

```rust
"unknown action '{}'. Must be one of: read, write, list, delete, remember, recall, forget, refresh_anchors"
```

Also update `input_schema` to include `refresh_anchors` in the action enum and description.

**Step 4: Run tests to verify they pass**

Run: `cargo test memory::tests::refresh_anchors --lib`
Expected: PASS.

Then: `cargo test --lib`

**Step 5: Commit**

```bash
git add src/memory/anchors.rs src/tools/memory.rs
git commit -m "feat(memory): refresh_anchors action to clear staleness"
```

---

### Task 8: `memory_anchors` Table Schema for Semantic Anchors

**Files:**
- Modify: `src/embed/index.rs` (add table creation and CRUD)

**Step 1: Write the failing test**

In `src/embed/index.rs` tests:

```rust
#[test]
fn memory_anchors_table_created() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_db(dir.path()).unwrap();
    ensure_memory_anchors(&conn).unwrap();

    // Verify table exists by inserting
    insert_semantic_anchor(
        &conn, "markdown", "architecture", "src/server.rs", "abc123", 0.85
    ).unwrap();

    let anchors = get_semantic_anchors(&conn, "markdown", "architecture").unwrap();
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].file_path, "src/server.rs");
    assert!((anchors[0].similarity - 0.85).abs() < 0.01);
}

#[test]
fn memory_anchors_upsert_on_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_db(dir.path()).unwrap();
    ensure_memory_anchors(&conn).unwrap();

    insert_semantic_anchor(&conn, "markdown", "arch", "src/a.rs", "h1", 0.8).unwrap();
    insert_semantic_anchor(&conn, "markdown", "arch", "src/a.rs", "h2", 0.9).unwrap();

    let anchors = get_semantic_anchors(&conn, "markdown", "arch").unwrap();
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].file_hash, "h2");
    assert!((anchors[0].similarity - 0.9).abs() < 0.01);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test memory_anchors_table --lib`
Expected: FAIL — functions don't exist.

**Step 3: Write minimal implementation**

In `src/embed/index.rs`:

```rust
#[derive(Debug, Clone)]
pub struct SemanticAnchor {
    pub file_path: String,
    pub file_hash: String,
    pub similarity: f32,
    pub stale: bool,
}

pub fn ensure_memory_anchors(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS memory_anchors (
            id INTEGER PRIMARY KEY,
            memory_type TEXT NOT NULL,
            memory_key TEXT NOT NULL,
            file_path TEXT NOT NULL,
            file_hash TEXT NOT NULL,
            similarity REAL NOT NULL,
            created_at TEXT NOT NULL,
            stale INTEGER NOT NULL DEFAULT 0,
            UNIQUE(memory_type, memory_key, file_path)
        )"
    )?;
    Ok(())
}

pub fn insert_semantic_anchor(
    conn: &Connection,
    memory_type: &str,
    memory_key: &str,
    file_path: &str,
    file_hash: &str,
    similarity: f32,
) -> Result<()> {
    let now = utc_now_display();
    conn.execute(
        "INSERT INTO memory_anchors (memory_type, memory_key, file_path, file_hash, similarity, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(memory_type, memory_key, file_path)
         DO UPDATE SET file_hash=excluded.file_hash, similarity=excluded.similarity, created_at=excluded.created_at, stale=0",
        params![memory_type, memory_key, file_path, file_hash, similarity, now],
    )?;
    Ok(())
}

pub fn get_semantic_anchors(
    conn: &Connection,
    memory_type: &str,
    memory_key: &str,
) -> Result<Vec<SemanticAnchor>> {
    let mut stmt = conn.prepare(
        "SELECT file_path, file_hash, similarity, stale FROM memory_anchors
         WHERE memory_type = ?1 AND memory_key = ?2"
    )?;
    let rows = stmt.query_map(params![memory_type, memory_key], |r| {
        Ok(SemanticAnchor {
            file_path: r.get(0)?,
            file_hash: r.get(1)?,
            similarity: r.get::<_, f64>(2)? as f32,
            stale: r.get::<_, i64>(3)? != 0,
        })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn delete_semantic_anchors(
    conn: &Connection,
    memory_type: &str,
    memory_key: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM memory_anchors WHERE memory_type = ?1 AND memory_key = ?2",
        params![memory_type, memory_key],
    )?;
    Ok(())
}

pub fn mark_anchors_stale_for_file(conn: &Connection, file_path: &str) -> Result<usize> {
    let count = conn.execute(
        "UPDATE memory_anchors SET stale = 1 WHERE file_path = ?1",
        params![file_path],
    )?;
    Ok(count)
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test memory_anchors --lib`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/embed/index.rs
git commit -m "feat(embed): memory_anchors table schema and CRUD"
```

---

### Task 9: Semantic Anchor Creation on Memory Write

**Files:**
- Modify: `src/tools/memory.rs` (extend `cross_embed_memory` or add sibling function)

**Step 1: Write the failing test**

In `src/tools/memory.rs` tests:

```rust
#[tokio::test]
async fn write_creates_semantic_anchors() {
    // This test requires an embedder and indexed project.
    // Use the mock/test embedder if available, or mark #[ignore] for CI.
    // For unit testing, test the anchor insertion logic directly:
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::embed::index::open_db(dir.path()).unwrap();
    crate::embed::index::ensure_memory_anchors(&conn).unwrap();

    // Simulate: semantic search returned these top files
    let search_results = vec![
        ("src/server.rs", "hash1", 0.92_f32),
        ("src/tools/mod.rs", "hash2", 0.87),
        ("src/agent.rs", "hash3", 0.21), // below threshold
    ];

    let min_similarity = 0.3_f32;
    let path_anchor_files: HashSet<String> = HashSet::new(); // no path anchors to exclude

    for (file, hash, sim) in &search_results {
        if *sim >= min_similarity && !path_anchor_files.contains(*file) {
            crate::embed::index::insert_semantic_anchor(
                &conn, "markdown", "architecture", file, hash, *sim
            ).unwrap();
        }
    }

    let anchors = crate::embed::index::get_semantic_anchors(&conn, "markdown", "architecture").unwrap();
    assert_eq!(anchors.len(), 2); // agent.rs excluded (below threshold)
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test memory::tests::write_creates_semantic --lib`
Expected: FAIL (or pass if pure DB test — adjust as needed).

**Step 3: Write minimal implementation**

Add to `src/tools/memory.rs`:

```rust
/// Create semantic anchors for a markdown memory by embedding it and finding
/// similar code chunks. Excludes files already covered by path anchors.
async fn create_semantic_anchors(
    ctx: &ToolContext,
    topic: &str,
    content: &str,
    path_anchor_files: &HashSet<String>,
) -> anyhow::Result<()> {
    let (root, model) = {
        let inner = ctx.agent.inner.read().await;
        let p = inner.active_project.as_ref().ok_or_else(|| anyhow::anyhow!("no project"))?;
        (p.root.clone(), p.config.embeddings.model.clone())
    };

    let embedder = ctx.agent.get_or_create_embedder(&model).await?;
    let embedding = crate::embed::embed_one(embedder.as_ref(), content).await?;

    let path_anchors = path_anchor_files.clone();
    let topic_owned = topic.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = crate::embed::index::open_db(&root)?;
        crate::embed::index::ensure_memory_anchors(&conn)?;

        // Delete old semantic anchors for this memory
        crate::embed::index::delete_semantic_anchors(&conn, "markdown", &topic_owned)?;

        // Search for similar code chunks
        let results = crate::embed::index::search(&conn, &embedding, 10)?;

        // Deduplicate by file, keep highest similarity
        let mut best_per_file: std::collections::HashMap<String, (f32, String)> = std::collections::HashMap::new();
        for r in &results {
            let sim = (1.0_f32 - r.distance as f32).clamp(0.0, 1.0);
            if sim < 0.3 { continue; }
            if path_anchors.contains(&r.file_path) { continue; }
            let hash = crate::embed::index::get_file_hash(&conn, &r.file_path)?
                .unwrap_or_default();
            best_per_file.entry(r.file_path.clone())
                .and_modify(|(old_sim, _)| { if sim > *old_sim { *old_sim = sim; } })
                .or_insert((sim, hash));
        }

        for (file_path, (sim, hash)) in &best_per_file {
            crate::embed::index::insert_semantic_anchor(
                &conn, "markdown", &topic_owned, file_path, hash, *sim,
            )?;
        }
        anyhow::Ok(())
    }).await??;
    Ok(())
}
```

Wire it into the `"write"` action after `update_anchors_on_write`:

```rust
// Create semantic anchors (best-effort, non-fatal)
if !private {
    let path_files: HashSet<String> = {
        let sidecar_path = memories_dir.join(format!("{}.anchors.toml", topic));
        crate::memory::anchors::read_anchor_file(&sidecar_path)
            .map(|af| af.anchors.into_iter().map(|a| a.path).collect())
            .unwrap_or_default()
    };
    if let Err(e) = create_semantic_anchors(ctx, topic, content, &path_files).await {
        tracing::debug!("semantic anchor creation failed (non-fatal): {e}");
    }
}
```

**Step 4: Run tests**

Run: `cargo test --lib`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/tools/memory.rs
git commit -m "feat(memory): create semantic anchors on memory write"
```

---

### Task 10: Reverse Drift Hook in `build_index`

**Files:**
- Modify: `src/embed/index.rs` (`build_index` function)

**Step 1: Write the failing test**

In `src/embed/index.rs` tests:

```rust
#[test]
fn mark_anchors_stale_for_drifted_file() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open_db(dir.path()).unwrap();
    ensure_memory_anchors(&conn).unwrap();

    // Create anchors pointing to a file
    insert_semantic_anchor(&conn, "markdown", "arch", "src/server.rs", "h1", 0.9).unwrap();
    insert_semantic_anchor(&conn, "markdown", "conv", "src/server.rs", "h2", 0.8).unwrap();
    insert_semantic_anchor(&conn, "markdown", "arch", "src/other.rs", "h3", 0.7).unwrap();

    // Mark src/server.rs as stale
    let count = mark_anchors_stale_for_file(&conn, "src/server.rs").unwrap();
    assert_eq!(count, 2);

    let arch = get_semantic_anchors(&conn, "markdown", "arch").unwrap();
    let server_anchor = arch.iter().find(|a| a.file_path == "src/server.rs").unwrap();
    assert!(server_anchor.stale);
    let other_anchor = arch.iter().find(|a| a.file_path == "src/other.rs").unwrap();
    assert!(!other_anchor.stale);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test mark_anchors_stale_for_drifted --lib`
Expected: PASS (function already exists from Task 8). This is a correctness test.

**Step 3: Wire into `build_index`**

In `src/embed/index.rs`, inside the `build_index` function, after the drift computation block (after `drift_results.push(drift)`) and before the `set_meta` calls, add:

```rust
// Reverse drift hook: mark semantic anchors as stale for high-drift files
if config.embeddings.drift_detection_enabled {
    // Ensure table exists (no-op if already created)
    ensure_memory_anchors(&conn)?;
    let threshold = 0.3_f32; // TODO: read from config when memory section is added
    for drift in &drift_results {
        if drift.avg_drift >= threshold {
            let _ = mark_anchors_stale_for_file(&conn, &drift.file_path);
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test --lib`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/embed/index.rs
git commit -m "feat(embed): reverse drift hook marks memory anchors stale"
```

---

### Task 11: `MemorySection` Config for Thresholds

**Files:**
- Modify: `src/config/project.rs`

**Step 1: Write the failing test**

In `src/config/project.rs` tests:

```rust
#[test]
fn memory_section_defaults() {
    let config: ProjectConfig = toml::from_str("").unwrap();
    assert!((config.memory.staleness_drift_threshold - 0.3).abs() < 0.01);
    assert!((config.memory.semantic_anchor_min_similarity - 0.3).abs() < 0.01);
    assert_eq!(config.memory.semantic_anchor_top_n, 10);
}

#[test]
fn memory_section_override() {
    let config: ProjectConfig = toml::from_str(
        "[memory]\nstaleness_drift_threshold = 0.5\n"
    ).unwrap();
    assert!((config.memory.staleness_drift_threshold - 0.5).abs() < 0.01);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test config::tests::memory_section --lib`
Expected: FAIL — field doesn't exist.

**Step 3: Write minimal implementation**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySection {
    #[serde(default = "default_staleness_drift_threshold")]
    pub staleness_drift_threshold: f32,
    #[serde(default = "default_semantic_anchor_min_similarity")]
    pub semantic_anchor_min_similarity: f32,
    #[serde(default = "default_semantic_anchor_top_n")]
    pub semantic_anchor_top_n: usize,
}

fn default_staleness_drift_threshold() -> f32 { 0.3 }
fn default_semantic_anchor_min_similarity() -> f32 { 0.3 }
fn default_semantic_anchor_top_n() -> usize { 10 }

impl Default for MemorySection {
    fn default() -> Self {
        Self {
            staleness_drift_threshold: default_staleness_drift_threshold(),
            semantic_anchor_min_similarity: default_semantic_anchor_min_similarity(),
            semantic_anchor_top_n: default_semantic_anchor_top_n(),
        }
    }
}
```

Add to `ProjectConfig`:

```rust
#[serde(default)]
pub memory: MemorySection,
```

**Step 4: Run tests**

Run: `cargo test config::tests::memory_section --lib`
Expected: PASS.

Then: `cargo test --lib` (ensure existing config tests still pass with the new default field).

**Step 5: Commit**

```bash
git add src/config/project.rs
git commit -m "feat(config): add [memory] section with staleness thresholds"
```

---

### Task 12: Wire Config Thresholds Into Drift Hook and Anchor Creation

**Files:**
- Modify: `src/embed/index.rs` (`build_index` — use config threshold)
- Modify: `src/tools/memory.rs` (`create_semantic_anchors` — use config values)

**Step 1: Update `build_index` to read config threshold**

Replace the hardcoded `0.3_f32` threshold in the reverse drift hook:

```rust
let threshold = config.memory.staleness_drift_threshold;
```

**Step 2: Update `create_semantic_anchors` to use config values**

Pass `min_similarity` and `top_n` from config instead of hardcoded values.

**Step 3: Run full test suite**

Run: `cargo test --lib`
Expected: PASS.

**Step 4: Commit**

```bash
git add src/embed/index.rs src/tools/memory.rs
git commit -m "feat: wire config thresholds into drift hook and anchor creation"
```

---

### Task 13: Update Server Instructions

**Files:**
- Modify: `src/prompts/server_instructions.md`

**Step 1: Add `refresh_anchors` to memory tool documentation**

In the memory tool section, add `refresh_anchors` to the action list and describe its purpose.

**Step 2: Add `memory_staleness` to `project_status` output documentation**

Document the new `memory_staleness` section in the `project_status` output.

**Step 3: Run `cargo test`**

Ensure no prompt-related tests break.

**Step 4: Commit**

```bash
git add src/prompts/server_instructions.md
git commit -m "docs: update server instructions with memory staleness"
```

---

### Task 14: Final Verification

**Step 1: Run full test suite**

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

Expected: All pass, no warnings.

**Step 2: Manual smoke test**

```bash
cargo run -- start --project .
```

Then in an MCP session:
1. `memory(action="write", topic="test-staleness", content="References src/server.rs.")`
2. Verify `.codescout/memories/test-staleness.anchors.toml` exists
3. `project_status()` — should show `test-staleness` as fresh
4. Modify `src/server.rs`
5. `project_status()` — should show `test-staleness` as stale
6. `memory(action="refresh_anchors", topic="test-staleness")` — clears staleness
7. `project_status()` — fresh again
8. Clean up: `memory(action="delete", topic="test-staleness")`

**Step 3: Commit and squash if needed**

```bash
git log --oneline  # review commit history
# If clean, push. If messy, interactive rebase.
```

---

## Task Dependency Graph

```
Task 1 (TOML sidecar)
  → Task 2 (path extraction)
    → Task 3 (seed + merge)
      → Task 4 (staleness check)
        → Task 5 (wire into write path)
          → Task 6 (wire into project_status)
          → Task 7 (refresh_anchors action)
            → Task 9 (semantic anchor creation) [depends on Task 8]

Task 8 (memory_anchors table) [independent of 1-7]
  → Task 9 (semantic anchor creation)
  → Task 10 (reverse drift hook)

Task 11 (config section) [independent]
  → Task 12 (wire thresholds)

Task 13 (server instructions) [after all features complete]
Task 14 (final verification) [last]
```

**Parallelizable:** Tasks 1-7 and Task 8 can be done in parallel by separate agents. Task 11 is also independent.
