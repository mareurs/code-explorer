//! Path security: read deny-list and write sandboxing.
//!
//! - **Reads** are allowed anywhere *except* a configurable deny-list of
//!   sensitive paths (~/.ssh, ~/.aws, etc.).
//! - **Writes** are restricted to the active project root (plus optional
//!   extra roots from configuration).

use anyhow::{bail, Result};
use regex::Regex;
use std::path::{Path, PathBuf};

/// Paths that are always denied for read access (expanded from `~`).
const DEFAULT_DENIED_PREFIXES: &[&str] = &[
    "~/.ssh",
    "~/.aws",
    "~/.gnupg",
    "~/.config/gcloud",
    "~/.config/gh",
    "~/.docker/config.json",
    "~/.netrc",
    "~/.npmrc",
    "~/.kube/config",
];

#[cfg(target_os = "linux")]
const DEFAULT_DENIED_EXACT: &[&str] = &["/etc/shadow", "/etc/gshadow"];

#[cfg(target_os = "macos")]
const DEFAULT_DENIED_EXACT: &[&str] = &["/etc/master.passwd"];

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const DEFAULT_DENIED_EXACT: &[&str] = &[];

// ---------------------------------------------------------------------------
// Public config type
// ---------------------------------------------------------------------------

/// Security configuration for path validation.
#[derive(Debug, Clone)]
pub struct PathSecurityConfig {
    /// Additional path patterns to deny reads from (beyond built-in defaults).
    pub denied_read_patterns: Vec<String>,
    /// Additional directories where writes are allowed (beyond project root).
    pub extra_write_roots: Vec<PathBuf>,
    /// Shell command mode: "unrestricted", "warn" (default), "disabled"
    pub shell_command_mode: String,
    /// Max bytes for shell command stdout/stderr (default 100KB)
    pub shell_output_limit_bytes: usize,
    /// Enable shell command execution (default: false)
    pub shell_enabled: bool,
    /// Enable file write tools (default: true)
    pub file_write_enabled: bool,
    /// Enable git tools (default: true)
    pub git_enabled: bool,
    /// Enable semantic search and indexing tools (default: true)
    pub indexing_enabled: bool,
    /// Read-only library paths (registered via LibraryRegistry).
    pub library_paths: Vec<PathBuf>,
    /// Command substrings that bypass dangerous-command detection.
    pub shell_allow_always: Vec<String>,
    /// Additional regex patterns to flag as dangerous commands.
    pub shell_dangerous_patterns: Vec<String>,
}

impl Default for PathSecurityConfig {
    fn default() -> Self {
        Self {
            denied_read_patterns: Vec::new(),
            extra_write_roots: Vec::new(),
            shell_command_mode: "warn".into(),
            shell_output_limit_bytes: 100 * 1024,
            shell_enabled: false,
            file_write_enabled: true,
            git_enabled: true,
            indexing_enabled: true,
            library_paths: Vec::new(),
            shell_allow_always: Vec::new(),
            shell_dangerous_patterns: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// Expand a leading `~` to `$HOME`.
fn expand_home(pattern: &str) -> Option<PathBuf> {
    if let Some(rest) = pattern.strip_prefix("~/") {
        home_dir().map(|h| h.join(rest))
    } else if pattern == "~" {
        home_dir()
    } else {
        Some(PathBuf::from(pattern))
    }
}

/// Build the full list of denied read paths (defaults + user-configured).
fn denied_read_paths(config: &PathSecurityConfig) -> Vec<PathBuf> {
    let mut denied = Vec::new();
    for p in DEFAULT_DENIED_PREFIXES
        .iter()
        .chain(DEFAULT_DENIED_EXACT.iter())
    {
        if let Some(expanded) = expand_home(p) {
            denied.push(expanded);
        }
    }
    // Windows-specific system paths
    #[cfg(windows)]
    {
        if let Ok(sysroot) = std::env::var("SYSTEMROOT") {
            denied.push(PathBuf::from(&sysroot).join("System32").join("config"));
        }
    }
    for p in &config.denied_read_patterns {
        if let Some(expanded) = expand_home(p) {
            denied.push(expanded);
        }
    }
    denied
}

/// Check if `resolved` falls under any denied path.
fn is_denied(resolved: &Path, denied: &[PathBuf]) -> bool {
    denied
        .iter()
        .any(|d| resolved.starts_with(d) || resolved == d.as_path())
}

/// Best-effort canonicalization: use `fs::canonicalize` when the path exists,
/// otherwise return the path as-is (e.g. for `CreateFile` targets that
/// don't exist yet).
fn best_effort_canonicalize(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Canonicalize a write target: the parent must exist (or be canonicalized
/// best-effort), then append the file name.
fn canonicalize_write_target(path: &Path) -> PathBuf {
    if let Some(parent) = path.parent() {
        let canon_parent = best_effort_canonicalize(parent);
        if let Some(name) = path.file_name() {
            return canon_parent.join(name);
        }
    }
    best_effort_canonicalize(path)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Validate a path for **read** access.
///
/// - Relative paths are resolved against `project_root` (if available).
/// - Absolute paths are used as-is.
/// - The resolved path is checked against the deny-list.
pub fn validate_read_path(
    raw: &str,
    project_root: Option<&Path>,
    config: &PathSecurityConfig,
) -> Result<PathBuf> {
    if raw.is_empty() {
        bail!("path must not be empty");
    }
    if raw.contains('\0') {
        bail!("path contains null byte");
    }

    let path = Path::new(raw);
    let resolved = if path.is_absolute() {
        PathBuf::from(raw)
    } else if let Some(root) = project_root {
        root.join(raw)
    } else {
        bail!("relative path '{}' requires an active project", raw);
    };

    // Canonicalize to resolve symlinks and `..` components.
    let resolved = best_effort_canonicalize(&resolved);

    let denied = denied_read_paths(config);
    if is_denied(&resolved, &denied) {
        bail!("access denied: '{}' is in a protected location", raw);
    }

    Ok(resolved)
}

/// Validate a path for **write** access.
///
/// - Relative paths are resolved against `project_root`.
/// - The resolved path must be under `project_root` or one of the
///   configured `extra_write_roots`.
/// - The deny-list is also checked (writes to `~/.ssh/` are always blocked).
pub fn validate_write_path(
    raw: &str,
    project_root: &Path,
    config: &PathSecurityConfig,
) -> Result<PathBuf> {
    if raw.is_empty() {
        bail!("path must not be empty");
    }
    if raw.contains('\0') {
        bail!("path contains null byte");
    }

    let path = Path::new(raw);
    let resolved = if path.is_absolute() {
        PathBuf::from(raw)
    } else {
        project_root.join(raw)
    };

    // For write targets the file may not exist yet, canonicalize via parent.
    let resolved = canonicalize_write_target(&resolved);
    let project_root = best_effort_canonicalize(project_root);

    // Check deny-list first (blocks writes to ~/.ssh even if somehow under
    // an extra_write_root).
    let denied = denied_read_paths(config);
    if is_denied(&resolved, &denied) {
        bail!("write denied: '{}' is in a protected location", raw);
    }

    // Check that the path is under an allowed root.
    let mut allowed = vec![project_root];
    for extra in &config.extra_write_roots {
        allowed.push(best_effort_canonicalize(extra));
    }

    let under_allowed_root = allowed.iter().any(|root| resolved.starts_with(root));
    if !under_allowed_root {
        bail!("write denied: '{}' is outside the project root", raw);
    }

    Ok(resolved)
}

/// List the root paths of all linked git worktrees for `project_root`.
///
/// Reads `.git/worktrees/<name>/gitdir` files, which contain absolute paths
/// like `/path/to/worktree/.git`. Returns the parent (the worktree root).
/// Returns an empty vec if no worktrees exist (the common case).
pub fn list_git_worktrees(project_root: &Path) -> Vec<PathBuf> {
    let worktrees_dir = project_root.join(".git").join("worktrees");
    if !worktrees_dir.is_dir() {
        return vec![];
    }
    let entries = match std::fs::read_dir(&worktrees_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let gitdir_file = entry.path().join("gitdir");
        if let Ok(content) = std::fs::read_to_string(&gitdir_file) {
            let worktree_git = PathBuf::from(content.trim());
            if let Some(worktree_root) = worktree_git.parent() {
                paths.push(worktree_root.to_path_buf());
            }
        }
    }
    paths
}

/// Returns an advisory hint string if git linked worktrees exist under
/// `project_root`. Intended to be included in write-tool responses so an
/// agent knows it may have written to the main repo instead of a worktree.
///
/// Returns `None` if no worktrees exist (zero-overhead fast path).
pub fn worktree_hint(project_root: &Path) -> Option<String> {
    let worktrees = list_git_worktrees(project_root);
    if worktrees.is_empty() {
        return None;
    }
    let wt_list = worktrees
        .iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "This repo has linked worktrees:\n{wt_list}\n\
         If you meant to edit a worktree, call activate_project first.",
    ))
}

// ---------------------------------------------------------------------------
// Tool access controls
// ---------------------------------------------------------------------------

/// Check if a tool is allowed by the current security configuration.
/// Returns Ok(()) if allowed, or an error message explaining how to enable it.
pub fn check_tool_access(tool_name: &str, config: &PathSecurityConfig) -> Result<()> {
    match tool_name {
        "run_command" => {
            if !config.shell_enabled {
                bail!(
                    "Shell commands are disabled. Set security.shell_enabled = true in .code-explorer/project.toml to enable."
                );
            }
        }
        "create_file" | "edit_file" | "replace_symbol" | "insert_code" | "rename_symbol"
        | "remove_symbol" => {
            if !config.file_write_enabled {
                bail!(
                    "File write tools are disabled. Set security.file_write_enabled = true in .code-explorer/project.toml to enable."
                );
            }
        }
        "git_blame" => {
            if !config.git_enabled {
                bail!(
                    "Git tools are disabled. Set security.git_enabled = true in .code-explorer/project.toml to enable."
                );
            }
        }
        "semantic_search" | "index_project" | "index_status" => {
            if !config.indexing_enabled {
                bail!(
                    "Indexing tools are disabled. Set security.indexing_enabled = true in .code-explorer/project.toml to enable."
                );
            }
        }
        _ => {} // All other tools are always allowed
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dangerous command detection
// ---------------------------------------------------------------------------

/// Default patterns that indicate a dangerous/destructive command.
/// Each entry is (regex_pattern, human-readable description).
const DEFAULT_DANGEROUS_PATTERNS: &[(&str, &str)] = &[
    (
        r"rm\s+(-[a-zA-Z]*f|-[a-zA-Z]*r|--force|--recursive)",
        "rm with --force or --recursive",
    ),
    (r"git\s+push\s+.*--force", "git push --force"),
    (r"git\s+reset\s+--hard", "git reset --hard"),
    (r"git\s+branch\s+-D\b", "git branch -D (force delete)"),
    (
        r"git\s+checkout\s+--\s+\.",
        "git checkout -- . (discard all changes)",
    ),
    (
        r"git\s+clean\s+-[a-zA-Z]*f",
        "git clean -f (remove untracked files)",
    ),
    (r"(?i)DROP\s+(TABLE|DATABASE)", "SQL DROP TABLE/DATABASE"),
    (r"chmod\s+777", "chmod 777 (world-writable)"),
    (r"kill\s+-9", "kill -9 (SIGKILL)"),
    (r"\bmkfs\b", "mkfs (format filesystem)"),
    (r"\bdd\s+if=", "dd (raw disk write)"),
];

/// Check if a command matches a dangerous pattern.
///
/// Returns the matched pattern description if dangerous, `None` if safe.
/// Respects `shell_allow_always` overrides from config.
pub fn is_dangerous_command(command: &str, config: &PathSecurityConfig) -> Option<String> {
    // 1. Check allow-list first — if command contains any allow string, it's safe.
    for allow in &config.shell_allow_always {
        if command.contains(allow.as_str()) {
            return None;
        }
    }

    // 2. Check built-in dangerous patterns.
    for (pattern, description) in DEFAULT_DANGEROUS_PATTERNS {
        if let Ok(re) = Regex::new(pattern) {
            if re.is_match(command) {
                return Some(description.to_string());
            }
        }
    }

    // 3. Check user-configured dangerous patterns.
    for pattern in &config.shell_dangerous_patterns {
        if let Ok(re) = Regex::new(pattern) {
            if re.is_match(command) {
                return Some(format!("matches custom pattern: {}", pattern));
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn default_config() -> PathSecurityConfig {
        PathSecurityConfig::default()
    }

    // ── Read validation ──────────────────────────────────────────────────

    #[test]
    fn read_empty_path_rejected() {
        let result = validate_read_path("", None, &default_config());
        assert!(result.is_err());
    }

    #[test]
    fn read_null_byte_rejected() {
        let result = validate_read_path("hello\0world", None, &default_config());
        assert!(result.is_err());
    }

    #[test]
    fn read_relative_without_project_errors() {
        let result = validate_read_path("src/main.rs", None, &default_config());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires an active project"));
    }

    #[test]
    fn read_relative_with_project_resolves() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("hello.txt");
        std::fs::write(&file, "hi").unwrap();

        let result = validate_read_path("hello.txt", Some(dir.path()), &default_config());
        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("hello.txt"));
    }

    #[test]
    fn read_absolute_outside_project_allowed() {
        // An absolute path to a non-sensitive location should work
        let dir = tempdir().unwrap();
        let file = dir.path().join("readable.txt");
        std::fs::write(&file, "data").unwrap();

        let result = validate_read_path(file.to_str().unwrap(), None, &default_config());
        assert!(result.is_ok());
    }

    #[test]
    fn read_ssh_key_denied() {
        if let Some(home) = home_dir() {
            let ssh_path = home.join(".ssh/id_rsa");
            let result = validate_read_path(ssh_path.to_str().unwrap(), None, &default_config());
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("protected location"));
        }
    }

    #[test]
    fn read_aws_credentials_denied() {
        if let Some(home) = home_dir() {
            let aws_path = home.join(".aws/credentials");
            let result = validate_read_path(aws_path.to_str().unwrap(), None, &default_config());
            assert!(result.is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn read_etc_shadow_denied() {
        let result = validate_read_path("/etc/shadow", None, &default_config());
        assert!(result.is_err());
    }

    #[test]
    fn read_custom_denied_pattern() {
        let secret_dir = std::env::temp_dir().join("code_explorer_secret_test");
        let secret_str = secret_dir.to_str().unwrap().to_string();
        let config = PathSecurityConfig {
            denied_read_patterns: vec![secret_str.clone()],
            extra_write_roots: vec![],
            ..Default::default()
        };
        // Create the directory so canonicalize works
        if secret_dir.exists() || std::fs::create_dir_all(&secret_dir).is_ok() {
            let test_path = format!("{}/data.txt", secret_str);
            let result = validate_read_path(&test_path, None, &config);
            assert!(result.is_err());
            let _ = std::fs::remove_dir(&secret_dir);
        }
    }

    // ── Write validation ─────────────────────────────────────────────────

    #[test]
    fn write_empty_path_rejected() {
        let dir = tempdir().unwrap();
        let result = validate_write_path("", dir.path(), &default_config());
        assert!(result.is_err());
    }

    #[test]
    fn write_null_byte_rejected() {
        let dir = tempdir().unwrap();
        let result = validate_write_path("file\0evil", dir.path(), &default_config());
        assert!(result.is_err());
    }

    #[test]
    fn write_within_project_allowed() {
        let dir = tempdir().unwrap();
        // Create the target directory so canonicalize resolves properly
        std::fs::create_dir_all(dir.path().join("src")).unwrap();

        let result = validate_write_path("src/new.rs", dir.path(), &default_config());
        assert!(result.is_ok());
        assert!(result
            .unwrap()
            .starts_with(dir.path().canonicalize().unwrap()));
    }

    #[test]
    fn write_outside_project_rejected() {
        let project = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let target = outside.path().join("evil.rs");

        let result =
            validate_write_path(target.to_str().unwrap(), project.path(), &default_config());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("outside the project root"));
    }

    #[test]
    fn write_traversal_outside_project_rejected() {
        let project = tempdir().unwrap();
        std::fs::create_dir_all(project.path().join("src")).unwrap();

        let result = validate_write_path("../../../tmp/evil.rs", project.path(), &default_config());
        assert!(result.is_err());
    }

    #[test]
    fn write_extra_root_allowed() {
        let project = tempdir().unwrap();
        let extra = tempdir().unwrap();
        std::fs::create_dir_all(extra.path().join("sub")).unwrap();

        let config = PathSecurityConfig {
            denied_read_patterns: vec![],
            extra_write_roots: vec![extra.path().to_path_buf()],
            ..Default::default()
        };

        let target = extra.path().join("sub/file.rs");
        let result = validate_write_path(target.to_str().unwrap(), project.path(), &config);
        assert!(result.is_ok());
    }

    #[test]
    fn write_to_ssh_denied_even_if_under_project() {
        // If somehow ~/.ssh were under the project root, it should still be denied
        if let Some(home) = home_dir() {
            let ssh_path = home.join(".ssh/authorized_keys");
            let result = validate_write_path(
                ssh_path.to_str().unwrap(),
                &home, // pretend home is the project root
                &default_config(),
            );
            assert!(result.is_err());
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("protected location"));
        }
    }

    // ── Symlink resolution ───────────────────────────────────────────────

    #[test]
    fn symlink_to_denied_path_is_caught_on_read() {
        if let Some(home) = home_dir() {
            let ssh_dir = home.join(".ssh");
            if !ssh_dir.exists() {
                return; // skip if no .ssh directory
            }

            let dir = tempdir().unwrap();
            let link = dir.path().join("sneaky_link");
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&ssh_dir, &link).unwrap();
                // Find an actual file inside ~/.ssh to test against.
                // If none exists, test the directory symlink itself.
                let target = std::fs::read_dir(&ssh_dir).ok().and_then(|mut entries| {
                    entries.find_map(|e| {
                        let e = e.ok()?;
                        e.file_type().ok()?.is_file().then(|| e.file_name())
                    })
                });
                let test_path = match &target {
                    Some(file) => link.join(file),
                    None => link.clone(), // test directory itself
                };
                let result = validate_read_path(
                    test_path.to_str().unwrap(),
                    Some(dir.path()),
                    &default_config(),
                );
                // After canonicalization the symlink resolves to ~/.ssh/...
                assert!(
                    result.is_err(),
                    "symlink to ~/.ssh should be denied, path: {:?}",
                    test_path
                );
            }
        }
    }

    #[test]
    fn symlink_write_escape_caught() {
        let project = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let escape_target = outside.path().join("escaped.txt");
        std::fs::write(&escape_target, "").unwrap();

        // Create symlink inside project pointing outside
        let link = project.path().join("sneaky");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), &link).unwrap();
            let result =
                validate_write_path("sneaky/escaped.txt", project.path(), &default_config());
            // After canonicalization, this resolves outside the project
            assert!(result.is_err());
        }
    }

    // ── Tool access controls ─────────────────────────────────────────────

    #[test]
    fn shell_disabled_by_default() {
        let config = PathSecurityConfig::default();
        assert!(!config.shell_enabled);
        assert!(check_tool_access("run_command", &config).is_err());
    }

    #[test]
    fn shell_enabled_when_configured() {
        let mut config = PathSecurityConfig::default();
        config.shell_enabled = true;
        assert!(check_tool_access("run_command", &config).is_ok());
    }

    #[test]
    fn file_write_enabled_by_default() {
        let config = PathSecurityConfig::default();
        assert!(config.file_write_enabled);
        assert!(check_tool_access("create_file", &config).is_ok());
        assert!(check_tool_access("replace_symbol", &config).is_ok());
    }

    #[test]
    fn file_write_disabled_blocks_all_write_tools() {
        let mut config = PathSecurityConfig::default();
        config.file_write_enabled = false;
        for tool in &[
            "create_file",
            "edit_file",
            "remove_symbol",
            "replace_symbol",
            "insert_code",
            "rename_symbol",
        ] {
            assert!(
                check_tool_access(tool, &config).is_err(),
                "{} should be blocked",
                tool
            );
        }
    }

    #[test]
    fn git_disabled_blocks_git_tools() {
        let mut config = PathSecurityConfig::default();
        config.git_enabled = false;
        for tool in &["git_blame"] {
            assert!(
                check_tool_access(tool, &config).is_err(),
                "{} should be blocked",
                tool
            );
        }
    }

    #[test]
    fn indexing_disabled_blocks_search_tools() {
        let mut config = PathSecurityConfig::default();
        config.indexing_enabled = false;
        for tool in &["semantic_search", "index_project", "index_status"] {
            assert!(
                check_tool_access(tool, &config).is_err(),
                "{} should be blocked",
                tool
            );
        }
    }

    #[test]
    fn read_tools_always_allowed() {
        let mut config = PathSecurityConfig::default();
        config.shell_enabled = false;
        config.file_write_enabled = false;
        config.git_enabled = false;
        config.indexing_enabled = false;
        // Read tools should always work
        for tool in &[
            "read_file",
            "list_dir",
            "search_pattern",
            "find_file",
            "find_symbol",
            "list_symbols",
            "list_functions",
            "onboarding",
            "activate_project",
        ] {
            assert!(
                check_tool_access(tool, &config).is_ok(),
                "{} should always be allowed",
                tool
            );
        }
    }

    #[test]
    fn home_dir_returns_some_on_all_platforms() {
        // home_dir() must return Some on every platform we support.
        // On Linux/macOS it reads $HOME, on Windows $USERPROFILE.
        let home = home_dir();
        assert!(
            home.is_some(),
            "home_dir() returned None — deny-list will be empty (security bug)"
        );
    }

    #[test]
    fn check_tool_access_error_message_includes_config_hint() {
        let config = PathSecurityConfig::default();
        let err = check_tool_access("run_command", &config).unwrap_err();
        assert!(
            err.to_string().contains("shell_enabled"),
            "error should mention config key"
        );
        assert!(
            err.to_string().contains("project.toml"),
            "error should mention config file"
        );
    }

    #[test]
    fn library_paths_default_is_empty() {
        let config = PathSecurityConfig::default();
        assert!(config.library_paths.is_empty());
    }

    #[test]
    fn list_git_worktrees_empty_when_no_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = list_git_worktrees(dir.path());
        assert!(result.is_empty());
    }

    #[test]
    fn list_git_worktrees_finds_linked_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let wt_root = tempfile::tempdir().unwrap();
        let wt_entry = dir.path().join(".git").join("worktrees").join("feat");
        std::fs::create_dir_all(&wt_entry).unwrap();
        let gitdir_content = format!("{}/.git\n", wt_root.path().display());
        std::fs::write(wt_entry.join("gitdir"), &gitdir_content).unwrap();

        let result = list_git_worktrees(dir.path());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], wt_root.path());
    }

    #[test]
    fn worktree_hint_none_when_no_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let hint = worktree_hint(dir.path());
        assert!(hint.is_none());
    }

    #[test]
    fn worktree_hint_some_when_worktrees_exist() {
        let dir = tempfile::tempdir().unwrap();
        let wt_root = tempfile::tempdir().unwrap();
        let wt_entry = dir.path().join(".git").join("worktrees").join("feat");
        std::fs::create_dir_all(&wt_entry).unwrap();
        let gitdir_content = format!("{}/.git\n", wt_root.path().display());
        std::fs::write(wt_entry.join("gitdir"), &gitdir_content).unwrap();

        let hint = worktree_hint(dir.path());
        assert!(hint.is_some(), "should return hint when worktrees exist");
        let msg = hint.unwrap();
        assert!(
            msg.contains(wt_root.path().to_str().unwrap()),
            "hint should contain the worktree path"
        );
        assert!(
            msg.contains("activate_project"),
            "hint should mention activate_project"
        );
    }

    // ── Dangerous command detection ──────────────────────────────────────

    #[test]
    fn dangerous_command_detected() {
        let config = PathSecurityConfig::default();
        assert!(is_dangerous_command("rm -rf /tmp/foo", &config).is_some());
        assert!(is_dangerous_command("git push --force origin main", &config).is_some());
        assert!(is_dangerous_command("git reset --hard", &config).is_some());
        assert!(is_dangerous_command("git branch -D feature", &config).is_some());
        assert!(is_dangerous_command("git clean -fd", &config).is_some());
        assert!(is_dangerous_command("chmod 777 script.sh", &config).is_some());
        assert!(is_dangerous_command("kill -9 1234", &config).is_some());
    }

    #[test]
    fn safe_command_not_flagged() {
        let config = PathSecurityConfig::default();
        assert!(is_dangerous_command("cargo test", &config).is_none());
        assert!(is_dangerous_command("git status", &config).is_none());
        assert!(is_dangerous_command("git push origin main", &config).is_none());
        assert!(is_dangerous_command("rm temp.txt", &config).is_none());
        assert!(is_dangerous_command("npm run build", &config).is_none());
    }

    #[test]
    fn allow_always_bypasses_detection() {
        let mut config = PathSecurityConfig::default();
        config.shell_allow_always = vec!["git push --force".to_string()];
        assert!(is_dangerous_command("git push --force origin main", &config).is_none());
        // Other dangerous commands still detected
        assert!(is_dangerous_command("rm -rf /tmp", &config).is_some());
    }

    #[test]
    fn custom_dangerous_patterns() {
        let mut config = PathSecurityConfig::default();
        config.shell_dangerous_patterns = vec!["kubectl delete".to_string()];
        assert!(is_dangerous_command("kubectl delete pod nginx", &config).is_some());
    }
}
