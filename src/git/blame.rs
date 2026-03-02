//! Line-level git blame.

use anyhow::Result;
use std::path::Path;

use super::open_repo;

#[derive(Debug, Clone, serde::Serialize)]
pub struct BlameLine {
    pub line: usize,
    pub content: String,
    pub sha: String,
    pub author: String,
    pub timestamp: i64,
}

/// Return blame information for each line of `file` (relative to repo root).
pub fn blame_file(repo_path: &Path, file: &Path) -> Result<Vec<BlameLine>> {
    let repo = open_repo(repo_path)?;

    // `open_repo` uses `Repository::discover`, which may find a .git directory
    // above `repo_path` (e.g. project root is a subdirectory of the git repo).
    // git2 needs paths relative to the repo workdir, not the project root.
    let repo_file = match repo.workdir() {
        Some(workdir) => match repo_path.strip_prefix(workdir) {
            Ok(prefix) if !prefix.as_os_str().is_empty() => prefix.join(file),
            _ => file.to_path_buf(),
        },
        None => file.to_path_buf(),
    };

    let blame = repo.blame_file(&repo_file, None)?;

    // Read the COMMITTED version (not working dir) to avoid line-count mismatch
    let source = match committed_content(&repo, &repo_file) {
        Ok(content) => content,
        Err(_) => {
            // Fallback to disk if file is not yet in any commit (brand new file)
            std::fs::read_to_string(repo_path.join(file))?
        }
    };

    let mut result = vec![];
    for (i, line_text) in source.lines().enumerate() {
        let hunk = blame.get_line(i + 1).ok_or_else(|| {
            anyhow::anyhow!(
                "{} may have uncommitted changes. git blame only covers committed content. \
                 Use git_diff to see uncommitted changes.",
                file.display()
            )
        })?;

        let sig = hunk.orig_signature();
        result.push(BlameLine {
            line: i + 1,
            content: line_text.to_string(),
            sha: format!("{:.8}", hunk.orig_commit_id()),
            author: sig.name().unwrap_or("unknown").to_string(),
            timestamp: sig.when().seconds(),
        });
    }
    Ok(result)
}

/// Read the HEAD version of a file from the git object store.
fn committed_content(repo: &git2::Repository, file: &Path) -> Result<String> {
    let head = repo.head()?.peel_to_tree()?;
    let entry = head.get_path(file)?;
    let blob = repo.find_blob(entry.id())?;
    Ok(std::str::from_utf8(blob.content())?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Helper: init a git repo with a committed file
    fn init_repo_with_file(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, content).unwrap();

        let repo = git2::Repository::init(dir.path()).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("test.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        (dir, file)
    }

    #[test]
    fn blame_committed_file_works() {
        let (dir, _file) = init_repo_with_file("line 1\nline 2\nline 3\n");
        let result = blame_file(dir.path(), Path::new("test.txt")).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].line, 1);
        assert_eq!(result[0].content, "line 1");
    }

    #[test]
    fn blame_with_uncommitted_additions_still_works() {
        let (dir, file) = init_repo_with_file("line 1\nline 2\n");
        // Add more lines without committing
        std::fs::write(&file, "line 1\nline 2\nnew line 3\nnew line 4\n").unwrap();

        // Should still succeed — blaming the committed version (2 lines)
        let result = blame_file(dir.path(), Path::new("test.txt")).unwrap();
        assert_eq!(
            result.len(),
            2,
            "should blame the committed 2 lines, not the dirty 4"
        );
        assert_eq!(result[0].content, "line 1");
        assert_eq!(result[1].content, "line 2");
    }
}
