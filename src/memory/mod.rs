//! Markdown-based persistent memory store (mirrors Serena's memory system).
//!
//! Memories are stored as `.md` files in `.code-explorer/memories/`.
//! They are organized hierarchically via path-like topics:
//! e.g. "debugging/async-patterns" → `.code-explorer/memories/debugging/async-patterns.md`

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Per-project memory store.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    memories_dir: PathBuf,
}

impl MemoryStore {
    /// Open (or create) the memory store for a project root.
    pub fn open(project_root: &Path) -> Result<Self> {
        let memories_dir = project_root.join(".code-explorer").join("memories");
        std::fs::create_dir_all(&memories_dir)?;
        Ok(Self { memories_dir })
    }

    /// Open (or create) the private memory store for a project root.
    /// Private memories are gitignored — not shared with teammates.
    /// Automatically adds `.code-explorer/private-memories/` to `.gitignore`.
    pub fn open_private(project_root: &Path) -> Result<Self> {
        let memories_dir = project_root.join(".code-explorer").join("private-memories");
        std::fs::create_dir_all(&memories_dir)?;
        Self::ensure_gitignored(project_root, ".code-explorer/private-memories/")?;
        Ok(Self { memories_dir })
    }

    fn ensure_gitignored(project_root: &Path, entry: &str) -> Result<()> {
        let gitignore_path = project_root.join(".gitignore");
        let existing = if gitignore_path.exists() {
            std::fs::read_to_string(&gitignore_path)?
        } else {
            String::new()
        };
        if existing.lines().any(|l| l.trim() == entry) {
            return Ok(());
        }
        let mut content = existing;
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(entry);
        content.push('\n');
        std::fs::write(&gitignore_path, content)?;
        Ok(())
    }

    /// Write or overwrite a memory entry.
    pub fn write(&self, topic: &str, content: &str) -> Result<()> {
        let path = self.topic_path(topic);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Read a memory entry by topic. Returns `None` if not found.
    pub fn read(&self, topic: &str) -> Result<Option<String>> {
        let path = self.topic_path(topic);
        if path.exists() {
            Ok(Some(std::fs::read_to_string(path)?))
        } else {
            Ok(None)
        }
    }

    /// List all memory topics.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut topics = vec![];
        for entry in walkdir::WalkDir::new(&self.memories_dir)
            .into_iter()
            .flatten()
        {
            if entry.file_type().is_file() {
                if let Some(ext) = entry.path().extension() {
                    if ext == "md" {
                        if let Ok(rel) = entry.path().strip_prefix(&self.memories_dir) {
                            let topic = rel.with_extension("").to_string_lossy().replace('\\', "/");
                            topics.push(topic);
                        }
                    }
                }
            }
        }
        topics.sort();
        Ok(topics)
    }

    /// Delete a memory entry.
    pub fn delete(&self, topic: &str) -> Result<()> {
        let path = self.topic_path(topic);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn topic_path(&self, topic: &str) -> PathBuf {
        // Sanitize: replace ".." components to prevent directory traversal.
        let safe = topic.replace("..", "__");
        // Strip leading path separators to prevent absolute paths from replacing
        // the base directory when passed to PathBuf::join.
        let safe = safe.trim_start_matches('/').trim_start_matches('\\');
        // Ensure empty topics produce a path inside memories_dir (not the dir itself).
        let safe = if safe.is_empty() { "_empty" } else { safe };
        self.memories_dir.join(safe).with_extension("md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_store() -> (tempfile::TempDir, MemoryStore) {
        let dir = tempdir().unwrap();
        let store = MemoryStore::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn open_private_creates_private_memories_dir() {
        let dir = tempdir().unwrap();
        let _store = MemoryStore::open_private(dir.path()).unwrap();
        assert!(dir.path().join(".code-explorer/private-memories").is_dir());
    }

    #[test]
    fn open_private_adds_to_gitignore() {
        let dir = tempdir().unwrap();
        MemoryStore::open_private(dir.path()).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains(".code-explorer/private-memories/"));
    }

    #[test]
    fn open_private_does_not_duplicate_gitignore_entry() {
        let dir = tempdir().unwrap();
        MemoryStore::open_private(dir.path()).unwrap();
        MemoryStore::open_private(dir.path()).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        let count = content
            .lines()
            .filter(|l| l.trim() == ".code-explorer/private-memories/")
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn open_private_appends_to_existing_gitignore() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        MemoryStore::open_private(dir.path()).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert!(content.contains("target/\n"));
        assert!(content.contains(".code-explorer/private-memories/"));
    }

    #[test]
    fn write_and_read_roundtrip() {
        let (_dir, store) = make_store();
        store.write("my-topic", "hello memory").unwrap();
        assert_eq!(
            store.read("my-topic").unwrap(),
            Some("hello memory".to_string())
        );
    }

    #[test]
    fn read_missing_returns_none() {
        let (_dir, store) = make_store();
        assert_eq!(store.read("does-not-exist").unwrap(), None);
    }

    #[test]
    fn list_empty_store() {
        let (_dir, store) = make_store();
        assert_eq!(store.list().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn list_after_writes_is_sorted() {
        let (_dir, store) = make_store();
        store.write("c-topic", "c").unwrap();
        store.write("a-topic", "a").unwrap();
        store.write("b-topic", "b").unwrap();
        let list = store.list().unwrap();
        assert_eq!(list, vec!["a-topic", "b-topic", "c-topic"]);
    }

    #[test]
    fn delete_removes_entry() {
        let (_dir, store) = make_store();
        store.write("to-delete", "content").unwrap();
        store.delete("to-delete").unwrap();
        assert_eq!(store.read("to-delete").unwrap(), None);
        assert!(!store.list().unwrap().contains(&"to-delete".to_string()));
    }

    #[test]
    fn delete_nonexistent_is_ok() {
        let (_dir, store) = make_store();
        assert!(store.delete("ghost").is_ok());
    }

    #[test]
    fn nested_topic_roundtrip() {
        let (_dir, store) = make_store();
        store
            .write("debugging/async-patterns", "avoid blocking")
            .unwrap();
        assert_eq!(
            store.read("debugging/async-patterns").unwrap(),
            Some("avoid blocking".to_string())
        );
        assert!(store
            .list()
            .unwrap()
            .contains(&"debugging/async-patterns".to_string()));
    }

    #[test]
    fn overwrite_replaces_content() {
        let (_dir, store) = make_store();
        store.write("key", "v1").unwrap();
        store.write("key", "v2").unwrap();
        assert_eq!(store.read("key").unwrap(), Some("v2".to_string()));
    }

    #[test]
    fn dotdot_in_topic_is_sanitized() {
        let (_dir, store) = make_store();
        // Should not escape the memories directory
        store.write("../escape", "evil").unwrap();
        // Reading with the same (sanitized) key works
        let result = store.read("../escape").unwrap();
        assert_eq!(result, Some("evil".to_string()));
    }

    #[test]
    fn absolute_path_topic_stays_inside_memories_dir() {
        let (_dir, store) = make_store();
        // An absolute path in topic should NOT escape the memories directory.
        // PathBuf::join with an absolute path replaces the base — this tests that
        // topic_path prevents that.
        let evil_topic = "/etc/shadow";
        let resolved = store.topic_path(evil_topic);
        assert!(
            resolved.starts_with(&store.memories_dir),
            "absolute path topic escaped memories dir: {:?}",
            resolved
        );
    }

    #[test]
    fn topic_with_null_byte_is_handled() {
        let (_dir, store) = make_store();
        // Null bytes in filenames can cause truncation in C-based syscalls.
        let result = store.write("safe\0evil", "content");
        // Should either succeed safely or return an error — not panic.
        // The important thing is the file (if created) stays inside memories_dir.
        if result.is_ok() {
            let path = store.topic_path("safe\0evil");
            assert!(path.starts_with(&store.memories_dir));
        }
    }

    #[test]
    fn topic_with_backslash_traversal_stays_inside() {
        let (_dir, store) = make_store();
        // Windows-style path traversal attempt
        let resolved = store.topic_path("..\\..\\etc\\passwd");
        assert!(
            resolved.starts_with(&store.memories_dir),
            "backslash traversal escaped memories dir: {:?}",
            resolved
        );
    }

    #[test]
    fn empty_topic_does_not_panic() {
        let (_dir, store) = make_store();
        // Empty topic should not panic
        let resolved = store.topic_path("");
        assert!(resolved.starts_with(&store.memories_dir));
    }

    #[test]
    fn deeply_nested_topic_works() {
        let (_dir, store) = make_store();
        store.write("a/b/c/d/e/deep-topic", "deep content").unwrap();
        assert_eq!(
            store.read("a/b/c/d/e/deep-topic").unwrap(),
            Some("deep content".to_string())
        );
    }

    #[test]
    fn topic_with_special_chars() {
        let (_dir, store) = make_store();
        // Topics with special characters should work or fail gracefully
        for topic in &["hello world", "a&b", "test=value", "name@domain"] {
            let result = store.write(topic, "content");
            if result.is_ok() {
                assert_eq!(store.read(topic).unwrap(), Some("content".to_string()));
            }
            // Either works or returns error — no panic
        }
    }
}
