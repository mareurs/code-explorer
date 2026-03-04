//! File system helpers.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Walk upward from `start` looking for a directory containing `marker`.
/// Returns the directory path if found.
pub fn find_ancestor_with(start: &Path, marker: &str) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(marker).exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Auto-detect the project root by looking for `.code-explorer/`, `.git/`,
/// `Cargo.toml`, `pyproject.toml`, or `package.json` — in that priority order.
pub fn detect_project_root(from: &Path) -> Option<PathBuf> {
    let markers = [
        ".code-explorer",
        ".git",
        "Cargo.toml",
        "pyproject.toml",
        "package.json",
        "go.mod",
    ];
    for marker in markers {
        if let Some(root) = find_ancestor_with(from, marker) {
            return Some(root);
        }
    }
    None
}

/// Read a file as UTF-8, returning an error with the path on failure.
pub fn read_utf8(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))
}

/// Write UTF-8 content to a file, creating parent directories as needed.
pub fn write_utf8(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)
        .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn find_ancestor_finds_marker_in_current_dir() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(
            find_ancestor_with(dir.path(), "Cargo.toml"),
            Some(dir.path().to_path_buf())
        );
    }

    #[test]
    fn find_ancestor_walks_up_to_parent() {
        let dir = tempdir().unwrap();
        let child = dir.path().join("src").join("nested");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(
            find_ancestor_with(&child, "Cargo.toml"),
            Some(dir.path().to_path_buf())
        );
    }

    #[test]
    fn find_ancestor_returns_none_when_absent() {
        let dir = tempdir().unwrap();
        assert_eq!(
            find_ancestor_with(dir.path(), "nonexistent-xyz-marker"),
            None
        );
    }

    #[test]
    fn detect_project_root_finds_cargo_toml() {
        let dir = tempdir().unwrap();
        let deep = dir.path().join("src").join("module");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_project_root(&deep), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn detect_project_root_prefers_codescout_dir_over_git() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::create_dir(dir.path().join(".code-explorer")).unwrap();
        // .code-explorer takes priority (first in markers list)
        assert_eq!(
            detect_project_root(dir.path()),
            Some(dir.path().to_path_buf())
        );
    }

    #[test]
    fn read_write_utf8_roundtrip() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        write_utf8(&file, "hello world").unwrap();
        assert_eq!(read_utf8(&file).unwrap(), "hello world");
    }

    #[test]
    fn write_utf8_creates_intermediate_dirs() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a").join("b").join("c.txt");
        write_utf8(&file, "deep content").unwrap();
        assert_eq!(read_utf8(&file).unwrap(), "deep content");
    }

    #[test]
    fn read_utf8_missing_file_errors() {
        let dir = tempdir().unwrap();
        assert!(read_utf8(&dir.path().join("missing.txt")).is_err());
    }
}
