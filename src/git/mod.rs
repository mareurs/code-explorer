//! Git integration using `git2`.

pub mod blame;

use anyhow::Result;
use std::path::Path;

/// Open the git repository at or above `path`.
pub fn open_repo(path: &Path) -> Result<git2::Repository> {
    git2::Repository::discover(path)
        .map_err(|e| anyhow::anyhow!("No git repository found at {}: {}", path.display(), e))
}

/// Get the short commit hash of HEAD.
pub fn head_short_sha(repo: &git2::Repository) -> Result<String> {
    let head = repo.head()?;
    let commit = head.peel_to_commit()?;
    let id = commit.id();
    Ok(format!("{:.8}", id))
}

/// List the last `limit` commits for a file path.
pub fn file_log(repo: &git2::Repository, file: &Path, limit: usize) -> Result<Vec<CommitSummary>> {
    let mut revwalk = repo.revwalk()?;
    revwalk.push_head()?;
    revwalk.set_sorting(git2::Sort::TIME)?;

    let mut commits = vec![];
    for oid in revwalk.take(limit * 10) {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;

        // Check if this commit touches the file
        if commit_touches_file(repo, &commit, file)? {
            commits.push(CommitSummary {
                sha: format!("{:.8}", commit.id()),
                message: commit.summary().unwrap_or("<no message>").to_string(),
                author: commit.author().name().unwrap_or("unknown").to_string(),
                timestamp: commit.time().seconds(),
            });
            if commits.len() >= limit {
                break;
            }
        }
    }
    Ok(commits)
}

fn commit_touches_file(
    repo: &git2::Repository,
    commit: &git2::Commit<'_>,
    file: &Path,
) -> Result<bool> {
    let tree = commit.tree()?;
    let _file_str = file.to_string_lossy();

    if commit.parent_count() == 0 {
        // Initial commit — check if file exists in tree
        return Ok(tree.get_path(file).is_ok());
    }

    let parent = commit.parent(0)?;
    let parent_tree = parent.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&parent_tree), Some(&tree), None)?;

    for delta in diff.deltas() {
        let touches = delta.new_file().path().map(|p| p == file).unwrap_or(false)
            || delta.old_file().path().map(|p| p == file).unwrap_or(false);
        if touches {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CommitSummary {
    pub sha: String,
    pub message: String,
    pub author: String,
    /// Unix timestamp
    pub timestamp: i64,
}

/// Diff the working directory (or a specific file) against HEAD or a given commit.
pub fn diff_workdir(
    repo: &git2::Repository,
    file: Option<&Path>,
    commit_sha: Option<&str>,
) -> Result<String> {
    let tree = if let Some(rev) = commit_sha {
        let obj = repo
            .revparse_single(rev)
            .map_err(|e| anyhow::anyhow!("Invalid commit ref '{}': {}", rev, e))?;
        let commit = obj
            .peel_to_commit()
            .map_err(|e| anyhow::anyhow!("'{}' is not a commit: {}", rev, e))?;
        Some(commit.tree()?)
    } else {
        // HEAD tree (may not exist for empty repos)
        repo.head().ok().and_then(|h| h.peel_to_tree().ok())
    };

    let mut opts = git2::DiffOptions::new();
    if let Some(f) = file {
        opts.pathspec(f.to_string_lossy().as_ref());
    }

    let diff = repo.diff_tree_to_workdir_with_index(tree.as_ref(), Some(&mut opts))?;

    let mut output = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let prefix = match line.origin() {
            '+' => "+",
            '-' => "-",
            ' ' => " ",
            _ => "",
        };
        output.push_str(prefix);
        output.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;

    Ok(output)
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffStatus {
    Added,
    Modified,
    Deleted,
    Renamed { old_path: String },
}

#[derive(Debug, Clone)]
pub struct DiffEntry {
    pub path: String,
    pub status: DiffStatus,
}

/// Diff two commits by SHA, returning a list of changed files.
/// Returns `Err` if either SHA is not found (e.g. after a rebase).
pub fn diff_tree_to_tree(
    repo: &git2::Repository,
    from_sha: &str,
    to_sha: &str,
) -> Result<Vec<DiffEntry>> {
    let from_obj = repo.revparse_single(from_sha)?;
    let to_obj = repo.revparse_single(to_sha)?;
    let from_tree = from_obj.peel_to_commit()?.tree()?;
    let to_tree = to_obj.peel_to_commit()?.tree()?;

    let mut opts = git2::DiffOptions::new();
    let diff = repo.diff_tree_to_tree(Some(&from_tree), Some(&to_tree), Some(&mut opts))?;

    // Enable rename detection
    let mut find_opts = git2::DiffFindOptions::new();
    find_opts.renames(true);
    let mut diff = diff;
    diff.find_similar(Some(&mut find_opts))?;

    let mut entries = Vec::new();
    for delta in diff.deltas() {
        let status = match delta.status() {
            git2::Delta::Added => DiffStatus::Added,
            git2::Delta::Modified => DiffStatus::Modified,
            git2::Delta::Deleted => DiffStatus::Deleted,
            git2::Delta::Renamed => {
                let old = delta
                    .old_file()
                    .path()
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                DiffStatus::Renamed { old_path: old }
            }
            _ => continue, // Ignore typechange, copied, etc.
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        entries.push(DiffEntry { path, status });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn init_repo(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@test.com").unwrap();
        repo
    }

    fn commit_file(repo: &git2::Repository, path: &str, content: &str, msg: &str) -> git2::Oid {
        let root = repo.workdir().unwrap();
        let file_path = root.join(path);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&file_path, content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(path)).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap()
    }

    #[test]
    fn diff_tree_detects_added_file() {
        let dir = tempdir().unwrap();
        let repo = init_repo(dir.path());
        let c1 = commit_file(&repo, "a.rs", "fn a() {}", "init");
        let c2 = commit_file(&repo, "b.rs", "fn b() {}", "add b");
        let entries = diff_tree_to_tree(&repo, &c1.to_string(), &c2.to_string()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "b.rs");
        assert!(matches!(entries[0].status, DiffStatus::Added));
    }

    #[test]
    fn diff_tree_detects_modified_file() {
        let dir = tempdir().unwrap();
        let repo = init_repo(dir.path());
        let c1 = commit_file(&repo, "a.rs", "fn a() {}", "init");
        let c2 = commit_file(&repo, "a.rs", "fn a() { 1 }", "modify a");
        let entries = diff_tree_to_tree(&repo, &c1.to_string(), &c2.to_string()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "a.rs");
        assert!(matches!(entries[0].status, DiffStatus::Modified));
    }

    #[test]
    fn diff_tree_detects_deleted_file() {
        let dir = tempdir().unwrap();
        let repo = init_repo(dir.path());
        let c1 = commit_file(&repo, "a.rs", "fn a() {}", "init");
        // Delete a.rs and commit
        std::fs::remove_file(dir.path().join("a.rs")).unwrap();
        let mut index = repo.index().unwrap();
        index.remove_path(Path::new("a.rs")).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = repo.signature().unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let c2 = repo
            .commit(Some("HEAD"), &sig, &sig, "del a", &tree, &[&parent])
            .unwrap();
        let entries = diff_tree_to_tree(&repo, &c1.to_string(), &c2.to_string()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "a.rs");
        assert!(matches!(entries[0].status, DiffStatus::Deleted));
    }

    #[test]
    fn diff_tree_returns_empty_for_same_commit() {
        let dir = tempdir().unwrap();
        let repo = init_repo(dir.path());
        let c1 = commit_file(&repo, "a.rs", "fn a() {}", "init");
        let entries = diff_tree_to_tree(&repo, &c1.to_string(), &c1.to_string()).unwrap();
        assert!(entries.is_empty());
    }
}
