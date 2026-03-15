use std::path::{Path, PathBuf};

/// Sentinel project ID used when no specific sub-project claims a file or
/// when the root of the workspace has no build manifest.
pub const ROOT_PROJECT_ID: &str = "root";

/// A project discovered by manifest walk during onboarding.
#[derive(Debug, Clone)]
pub struct DiscoveredProject {
    pub id: String,
    pub relative_root: PathBuf,
    pub languages: Vec<String>,
    pub manifest: Option<String>,
}

/// Walk the workspace root for build manifests and return discovered sub-projects.
pub fn discover_projects(
    workspace_root: &Path,
    max_depth: usize,
    exclude: &[String],
) -> Vec<DiscoveredProject> {
    let manifests: &[(&str, &[&str])] = &[
        ("Cargo.toml", &["rust"]),
        ("build.gradle.kts", &["kotlin", "java"]),
        ("build.gradle", &["kotlin", "java"]),
        ("go.mod", &["go"]),
        ("pom.xml", &["java"]),
        ("CMakeLists.txt", &["c", "cpp"]),
        ("mix.exs", &["elixir"]),
        ("Gemfile", &["ruby"]),
    ];
    let conditional_manifests: &[(&str, &[&str])] = &[
        ("package.json", &["typescript", "javascript"]),
        ("pyproject.toml", &["python"]),
        ("setup.py", &["python"]),
        ("requirements.txt", &["python"]),
    ];

    let mut manifest_dirs: std::collections::BTreeMap<PathBuf, (String, Vec<String>)> =
        std::collections::BTreeMap::new();

    let walker = ignore::WalkBuilder::new(workspace_root)
        .hidden(true)
        .git_ignore(true)
        .max_depth(Some(max_depth + 1))
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        let dir = entry.path().parent().unwrap_or(entry.path()).to_path_buf();

        let rel_dir = dir.strip_prefix(workspace_root).unwrap_or(&dir);

        // Skip excluded directories
        if exclude.iter().any(|ex| {
            rel_dir
                .components()
                .any(|c| c.as_os_str().to_string_lossy() == *ex)
        }) {
            continue;
        }

        // Skip node_modules explicitly (ignore crate won't skip without .gitignore)
        if rel_dir
            .components()
            .any(|c| c.as_os_str() == "node_modules")
        {
            continue;
        }

        // Check unconditional manifests
        for (manifest_name, langs) in manifests {
            if file_name == *manifest_name && !manifest_dirs.contains_key(&dir) {
                manifest_dirs.insert(
                    dir.clone(),
                    (
                        manifest_name.to_string(),
                        langs.iter().map(|s| s.to_string()).collect(),
                    ),
                );
                break;
            }
        }

        // Check conditional manifests
        for (manifest_name, langs) in conditional_manifests {
            if file_name != *manifest_name || manifest_dirs.contains_key(&dir) {
                continue;
            }
            if *manifest_name == "package.json" {
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                        let has_scripts = json
                            .get("scripts")
                            .and_then(|v| v.as_object())
                            .map(|o| !o.is_empty())
                            .unwrap_or(false);
                        let has_main = json.get("main").is_some() || json.get("module").is_some();
                        if !has_scripts && !has_main {
                            continue;
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
            if *manifest_name == "requirements.txt" && dir.join("pyproject.toml").exists() {
                continue;
            }
            manifest_dirs.insert(
                dir.clone(),
                (
                    manifest_name.to_string(),
                    langs.iter().map(|s| s.to_string()).collect(),
                ),
            );
            break;
        }
    }

    let workspace_name = workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();

    // Sort by depth so shallower (parent) projects come first
    let mut dirs: Vec<_> = manifest_dirs.into_iter().collect();
    dirs.sort_by_key(|(p, _)| p.components().count());

    let mut found: Vec<DiscoveredProject> = Vec::new();
    let mut found_roots: Vec<PathBuf> = Vec::new();

    for (dir, (manifest, languages)) in dirs {
        let rel = dir.strip_prefix(workspace_root).unwrap_or(&dir);
        let rel_path = if rel.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            rel.to_path_buf()
        };

        // Skip if dominated by a shallower project with matching language
        let dominated = found_roots.iter().any(|existing| {
            if rel_path == PathBuf::from(".") || existing == &PathBuf::from(".") {
                return false;
            }
            rel_path.starts_with(existing)
                && found.iter().any(|p| {
                    p.relative_root == *existing
                        && p.languages.iter().any(|l| languages.contains(l))
                })
        });
        if dominated {
            continue;
        }

        let id = if rel_path == PathBuf::from(".") {
            workspace_name.clone()
        } else {
            rel_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unnamed")
                .to_string()
        };

        // Deduplicate: if id already taken, use path-based name
        let final_id = if found.iter().any(|p| p.id == id) {
            rel_path.to_string_lossy().replace('/', "-")
        } else {
            id
        };

        found_roots.push(rel_path.clone());
        found.push(DiscoveredProject {
            id: final_id,
            relative_root: rel_path,
            languages,
            manifest: Some(manifest),
        });
    }

    // Ensure root project is first
    if let Some(root_idx) = found
        .iter()
        .position(|p| p.relative_root == PathBuf::from("."))
    {
        if root_idx != 0 {
            let root = found.remove(root_idx);
            found.insert(0, root);
        }
    }

    found
}

/// Resolve which project a file path belongs to using longest-prefix match.
pub fn resolve_project_for_path<'a>(
    projects: &'a [DiscoveredProject],
    workspace_root: &Path,
    file_path: &Path,
) -> Option<&'a DiscoveredProject> {
    let abs_file = if file_path.is_relative() {
        workspace_root.join(file_path)
    } else {
        file_path.to_path_buf()
    };

    projects
        .iter()
        .filter(|p| {
            let project_abs = if p.relative_root == PathBuf::from(".") {
                workspace_root.to_path_buf()
            } else {
                workspace_root.join(&p.relative_root)
            };
            abs_file.starts_with(&project_abs)
        })
        .max_by_key(|p| p.relative_root.components().count())
}

/// Given a file path and a list of discovered projects, return the project ID.
/// Falls back to `ROOT_PROJECT_ID` if no project claims the file.
pub fn resolve_project_id(
    projects: &[DiscoveredProject],
    workspace_root: &Path,
    file_path: &Path,
) -> String {
    resolve_project_for_path(projects, workspace_root, file_path)
        .map(|p| p.id.clone())
        .unwrap_or_else(|| ROOT_PROJECT_ID.to_string())
}

/// State of a project within the workspace.
pub enum ProjectState {
    /// Not yet activated — LSP not started, config not loaded.
    Dormant,
    /// Fully activated with config, memory, LSP running.
    Activated(Box<crate::agent::ActiveProject>),
}

/// A project within the workspace — discovered metadata + runtime state.
pub struct Project {
    pub discovered: DiscoveredProject,
    pub state: ProjectState,
}

impl Project {
    pub fn new_dormant(discovered: DiscoveredProject) -> Self {
        Self {
            discovered,
            state: ProjectState::Dormant,
        }
    }
}

/// The top-level workspace containing all discovered projects.
pub struct Workspace {
    pub root: PathBuf,
    pub projects: Vec<Project>,
    /// Currently focused project ID (used by require_project_root).
    pub focused: Option<String>,
}

impl Workspace {
    pub fn new(root: PathBuf, projects: Vec<Project>) -> Self {
        // Focus defaults to the root project if present, else first project
        let focused = projects
            .iter()
            .find(|p| p.discovered.relative_root == PathBuf::from("."))
            .or_else(|| projects.first())
            .map(|p| p.discovered.id.clone());
        Self {
            root,
            projects,
            focused,
        }
    }

    /// Return the absolute root path of the focused project.
    pub fn focused_project_root(&self) -> anyhow::Result<PathBuf> {
        let id = self
            .focused
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("No focused project"))?;
        self.project_root_by_id(id)
    }

    /// Return the absolute root path for a project by ID.
    pub fn project_root_by_id(&self, id: &str) -> anyhow::Result<PathBuf> {
        let project = self
            .projects
            .iter()
            .find(|p| p.discovered.id == id)
            .ok_or_else(|| anyhow::anyhow!("Project '{}' not found in workspace", id))?;
        let abs = if project.discovered.relative_root == PathBuf::from(".") {
            self.root.clone()
        } else {
            self.root.join(&project.discovered.relative_root)
        };
        Ok(abs)
    }

    /// Resolve root: explicit project ID > file hint > focused project.
    pub fn resolve_root(
        &self,
        project: Option<&str>,
        file_hint: Option<&Path>,
    ) -> anyhow::Result<PathBuf> {
        match (project, file_hint) {
            (Some(id), _) => self.project_root_by_id(id),
            (None, Some(path)) => {
                // Longest-prefix match
                let discovered: Vec<_> =
                    self.projects.iter().map(|p| p.discovered.clone()).collect();
                let result = resolve_project_for_path(&discovered, &self.root, path);
                match result {
                    Some(p) => self.project_root_by_id(&p.id),
                    None => self.focused_project_root(),
                }
            }
            (None, None) => self.focused_project_root(),
        }
    }

    /// Return the focused project, if any.
    pub fn focused_active(&self) -> Option<&Project> {
        let id = self.focused.as_deref()?;
        self.projects.iter().find(|p| p.discovered.id == id)
    }

    pub fn focused_active_mut(&mut self) -> Option<&mut Project> {
        // borrow-checker: clone here to release the immutable borrow on self.focused
        // before mutably iterating self.projects.
        let id = self.focused.clone()?;
        self.projects.iter_mut().find(|p| p.discovered.id == id)
    }

    /// Switch focus to a project by ID.
    pub fn set_focused(&mut self, project_id: &str) -> anyhow::Result<()> {
        if self.projects.iter().any(|p| p.discovered.id == project_id) {
            self.focused = Some(project_id.to_string());
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Project '{}' not found in workspace",
                project_id
            ))
        }
    }

    /// Return all project IDs in the workspace.
    pub fn project_ids(&self) -> Vec<String> {
        self.projects
            .iter()
            .map(|p| p.discovered.id.clone())
            .collect()
    }

    /// Memory directory for a project. Root project -> workspace-level, sub-projects -> per-project.
    pub fn memory_dir_for_project(&self, project_id: &str) -> PathBuf {
        let is_root = self
            .projects
            .iter()
            .find(|p| p.discovered.id == project_id)
            .map(|p| p.discovered.relative_root == PathBuf::from("."))
            .unwrap_or(true);
        if is_root {
            self.root.join(".codescout").join("memories")
        } else {
            self.root
                .join(".codescout")
                .join("projects")
                .join(project_id)
                .join("memories")
        }
    }
}

/// Helper to extract `&ActiveProject` from a focused project's activated state.
impl Project {
    pub fn as_active(&self) -> Option<&crate::agent::ActiveProject> {
        match &self.state {
            ProjectState::Activated(ap) => Some(ap.as_ref()),
            ProjectState::Dormant => None,
        }
    }

    pub fn as_active_mut(&mut self) -> Option<&mut crate::agent::ActiveProject> {
        match &mut self.state {
            ProjectState::Activated(ap) => Some(ap.as_mut()),
            ProjectState::Dormant => None,
        }
    }
}

/// Normalize a relative path reference against a base directory without touching
/// the filesystem (handles `../` components manually).
fn normalize_path(base: &Path, relative: &str) -> PathBuf {
    let mut result = base.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::Normal(c) => result.push(c),
            std::path::Component::CurDir => {}
            _ => result.push(component),
        }
    }
    result
}

/// Parse Cargo.toml `[dependencies]` / `[dev-dependencies]` for `path = "..."` entries.
fn deps_from_cargo(project_root: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(project_root.join("Cargo.toml")) else {
        return vec![];
    };
    let Ok(table) = toml::from_str::<toml::Value>(&content) else {
        return vec![];
    };
    let mut paths = vec![];
    for section in &["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(deps) = table.get(section).and_then(|v| v.as_table()) {
            for dep in deps.values() {
                if let Some(p) = dep.get("path").and_then(|v| v.as_str()) {
                    paths.push(p.to_string());
                }
            }
        }
    }
    paths
}

/// Parse package.json for `"file:../..."` or `"workspace:../..."` dependency values.
fn deps_from_npm(project_root: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(project_root.join("package.json")) else {
        return vec![];
    };
    let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content) else {
        return vec![];
    };
    let mut paths = vec![];
    for section in &["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(deps) = pkg.get(section).and_then(|v| v.as_object()) {
            for val in deps.values() {
                if let Some(s) = val.as_str() {
                    if let Some(path) = s
                        .strip_prefix("file:")
                        .or_else(|| s.strip_prefix("workspace:"))
                    {
                        if path.starts_with("../") || path.starts_with("./") {
                            paths.push(path.to_string());
                        }
                    }
                }
            }
        }
    }
    paths
}

/// Parse pyproject.toml for `{path = "..."}` dependency entries.
fn deps_from_pyproject(project_root: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(project_root.join("pyproject.toml")) else {
        return vec![];
    };
    let Ok(table) = toml::from_str::<toml::Value>(&content) else {
        return vec![];
    };
    let mut paths = vec![];
    // poetry: [tool.poetry.dependencies]
    for dep in table
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_table())
        .into_iter()
        .flat_map(|t| t.values())
    {
        if let Some(p) = dep.get("path").and_then(|v| v.as_str()) {
            paths.push(p.to_string());
        }
    }
    // PEP 517 / uv: [dependencies] with path entries (less common but handle it)
    for dep in table
        .get("project")
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
    {
        if let Some(s) = dep.as_str() {
            // Editable path dep: "my-pkg @ file:../sibling" or just "../sibling"
            if let Some(rest) = s.find("@ file:").map(|i| &s[i + 7..]) {
                if rest.starts_with("../") || rest.starts_with("./") {
                    paths.push(rest.to_string());
                }
            }
        }
    }
    paths
}

/// Parse requirements.txt for `-e ../sibling` editable installs.
fn deps_from_requirements(project_root: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(project_root.join("requirements.txt")) else {
        return vec![];
    };
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let path = line.strip_prefix("-e ")?;
            if path.starts_with("../") || path.starts_with("./") {
                Some(path.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Parse settings.gradle / settings.gradle.kts for `includeBuild("../sibling")`.
fn deps_from_gradle(project_root: &Path) -> Vec<String> {
    let content = ["settings.gradle.kts", "settings.gradle"]
        .iter()
        .find_map(|f| std::fs::read_to_string(project_root.join(f)).ok())
        .unwrap_or_default();

    let mut paths = vec![];
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("includeBuild(") {
            // Extract quoted string: includeBuild("../sibling")
            let inner = rest.trim_end_matches(')').trim();
            let path = inner
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| inner.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')));
            if let Some(p) = path {
                if p.starts_with("../") || p.starts_with("./") {
                    paths.push(p.to_string());
                }
            }
        }
    }
    paths
}

/// Infer cross-project dependencies for a single project from its manifest files.
///
/// Returns a sorted, deduplicated list of project IDs that `project_root` depends on,
/// based on path references found in Cargo.toml, package.json, pyproject.toml,
/// requirements.txt, and settings.gradle(.kts).
///
/// Only same-workspace references are returned — external paths are silently ignored.
pub fn infer_depends_on(
    project_root: &Path,
    workspace_root: &Path,
    all_projects: &[DiscoveredProject],
) -> Vec<String> {
    // Build a map: canonical absolute root → project id
    let project_by_abs: std::collections::HashMap<PathBuf, &str> = all_projects
        .iter()
        .map(|p| (workspace_root.join(&p.relative_root), p.id.as_str()))
        .collect();

    let raw_paths: Vec<String> = [
        deps_from_cargo(project_root),
        deps_from_npm(project_root),
        deps_from_pyproject(project_root),
        deps_from_requirements(project_root),
        deps_from_gradle(project_root),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut deps = std::collections::BTreeSet::new();
    for raw in raw_paths {
        let abs = normalize_path(project_root, &raw);
        if let Some(&id) = project_by_abs.get(&abs) {
            // Don't add self-references
            if abs != *project_root {
                deps.insert(id.to_string());
            }
        }
    }
    deps.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn discover_single_project_repo() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

        let projects = discover_projects(dir.path(), 3, &[]);
        assert_eq!(projects.len(), 1);
        assert_eq!(
            projects[0].id,
            dir.path().file_name().unwrap().to_str().unwrap()
        );
        assert_eq!(projects[0].relative_root, std::path::Path::new("."));
        assert_eq!(projects[0].manifest, Some("Cargo.toml".to_string()));
    }

    #[test]
    fn discover_multi_project_repo() {
        let dir = tempdir().unwrap();
        // Root: Kotlin
        fs::write(dir.path().join("build.gradle.kts"), "").unwrap();
        // Sub: TypeScript
        let mcp = dir.path().join("mcp-server");
        fs::create_dir_all(&mcp).unwrap();
        fs::write(mcp.join("package.json"), r#"{"scripts":{"build":"tsc"}}"#).unwrap();
        // Sub: Python
        let py = dir.path().join("python-services");
        fs::create_dir_all(&py).unwrap();
        fs::write(py.join("requirements.txt"), "flask\n").unwrap();

        let projects = discover_projects(dir.path(), 3, &[]);
        assert_eq!(projects.len(), 3);

        // Root project first
        assert_eq!(projects[0].relative_root, std::path::Path::new("."));
        assert_eq!(projects[0].manifest, Some("build.gradle.kts".to_string()));

        // Sub-projects sorted by id
        let ids: Vec<&str> = projects.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"mcp-server"));
        assert!(ids.contains(&"python-services"));
    }

    #[test]
    fn skips_node_modules_manifests() {
        let dir = tempdir().unwrap();
        // Root has a real project (non-empty scripts)
        fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"test":"jest"}}"#,
        )
        .unwrap();
        let nm = dir.path().join("node_modules").join("dep");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("package.json"), r#"{"name":"dep"}"#).unwrap();

        let projects = discover_projects(dir.path(), 3, &[]);
        assert_eq!(projects.len(), 1); // only root, not node_modules/dep
    }

    #[test]
    fn respects_exclude_list() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("build.gradle.kts"), "").unwrap();
        let tools = dir.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        fs::write(tools.join("requirements.txt"), "click\n").unwrap();

        let projects = discover_projects(dir.path(), 3, &["tools".to_string()]);
        assert_eq!(projects.len(), 1); // tools excluded
    }

    #[test]
    fn max_depth_limits_discovery() {
        let dir = tempdir().unwrap();
        // Manifest at depth 4 — should be skipped with max_depth=3
        let deep = dir
            .path()
            .join("a")
            .join("b")
            .join("c")
            .join("deep-service");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("Cargo.toml"), "[package]\nname = \"deep\"").unwrap();

        let projects = discover_projects(dir.path(), 3, &[]);
        assert!(
            projects.is_empty(),
            "manifest at depth 4 should be skipped with max_depth=3"
        );

        // But discoverable with max_depth=5
        let projects = discover_projects(dir.path(), 5, &[]);
        assert_eq!(projects.len(), 1);
    }

    #[test]
    fn id_collision_is_deduplicated() {
        let dir = tempdir().unwrap();
        // Two subdirectories named "api" at different paths
        let svc_api = dir.path().join("services").join("api");
        fs::create_dir_all(&svc_api).unwrap();
        fs::write(svc_api.join("Cargo.toml"), "[package]\nname = \"svc-api\"").unwrap();

        let tools_api = dir.path().join("tools").join("api");
        fs::create_dir_all(&tools_api).unwrap();
        fs::write(
            tools_api.join("Cargo.toml"),
            "[package]\nname = \"tools-api\"",
        )
        .unwrap();

        let projects = discover_projects(dir.path(), 3, &[]);
        let ids: Vec<&str> = projects.iter().map(|p| p.id.as_str()).collect();
        // IDs must be unique — one gets the plain name, other gets path-based name
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "IDs must be unique: got {:?}", ids);
    }

    #[test]
    fn resolve_project_from_path_uses_longest_prefix() {
        let dir = tempdir().unwrap();
        let projects = vec![
            DiscoveredProject {
                id: ROOT_PROJECT_ID.into(),
                relative_root: ".".into(),
                languages: vec!["kotlin".into()],
                manifest: Some("build.gradle.kts".into()),
            },
            DiscoveredProject {
                id: "mcp-server".into(),
                relative_root: "mcp-server".into(),
                languages: vec!["typescript".into()],
                manifest: Some("package.json".into()),
            },
        ];

        // File inside mcp-server → resolves to mcp-server
        let result = resolve_project_for_path(
            &projects,
            dir.path(),
            &dir.path().join("mcp-server/src/index.ts"),
        );
        assert_eq!(result.unwrap().id, "mcp-server");

        // File at root → resolves to root
        let result = resolve_project_for_path(
            &projects,
            dir.path(),
            &dir.path().join("src/main/kotlin/App.kt"),
        );
        assert_eq!(result.unwrap().id, ROOT_PROJECT_ID);
    }

    // ── infer_depends_on tests ──────────────────────────────────────────────

    fn make_project(id: &str, relative_root: &str) -> DiscoveredProject {
        DiscoveredProject {
            id: id.to_string(),
            relative_root: PathBuf::from(relative_root),
            languages: vec![],
            manifest: None,
        }
    }

    #[test]
    fn infer_depends_on_cargo_path_dep() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let api = ws.join("api");
        let shared = ws.join("shared");
        fs::create_dir_all(&api).unwrap();
        fs::create_dir_all(&shared).unwrap();
        fs::write(
            api.join("Cargo.toml"),
            "[package]\nname = \"api\"\n\n[dependencies]\nshared = { path = \"../shared\" }\n",
        )
        .unwrap();
        let projects = vec![make_project("api", "api"), make_project("shared", "shared")];
        let deps = infer_depends_on(&api, ws, &projects);
        assert_eq!(deps, vec!["shared"]);
    }

    #[test]
    fn infer_depends_on_npm_workspace_protocol() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let web = ws.join("web");
        let ui = ws.join("ui");
        fs::create_dir_all(&web).unwrap();
        fs::create_dir_all(&ui).unwrap();
        fs::write(
            web.join("package.json"),
            r#"{"name":"web","scripts":{"build":"tsc"},"dependencies":{"@app/ui":"workspace:../ui"}}"#,
        )
        .unwrap();
        let projects = vec![make_project("web", "web"), make_project("ui", "ui")];
        let deps = infer_depends_on(&web, ws, &projects);
        assert_eq!(deps, vec!["ui"]);
    }

    #[test]
    fn infer_depends_on_requirements_txt_editable() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let svc = ws.join("svc");
        let lib = ws.join("lib");
        fs::create_dir_all(&svc).unwrap();
        fs::create_dir_all(&lib).unwrap();
        fs::write(svc.join("requirements.txt"), "-e ../lib\nrequests==2.31\n").unwrap();
        let projects = vec![make_project("svc", "svc"), make_project("lib", "lib")];
        let deps = infer_depends_on(&svc, ws, &projects);
        assert_eq!(deps, vec!["lib"]);
    }

    #[test]
    fn infer_depends_on_gradle_include_build() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let app = ws.join("app");
        let core = ws.join("core");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&core).unwrap();
        fs::write(
            app.join("settings.gradle.kts"),
            "includeBuild(\"../core\")\n",
        )
        .unwrap();
        let projects = vec![make_project("app", "app"), make_project("core", "core")];
        let deps = infer_depends_on(&app, ws, &projects);
        assert_eq!(deps, vec!["core"]);
    }

    #[test]
    fn infer_depends_on_no_manifests_returns_empty() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let a = ws.join("a");
        fs::create_dir_all(&a).unwrap();
        let projects = vec![make_project("a", "a"), make_project("b", "b")];
        let deps = infer_depends_on(&a, ws, &projects);
        assert!(deps.is_empty());
    }

    #[test]
    fn infer_depends_on_ignores_external_paths() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let api = ws.join("api");
        fs::create_dir_all(&api).unwrap();
        // References a path outside the workspace
        fs::write(
            api.join("Cargo.toml"),
            "[package]\nname = \"api\"\n\n[dependencies]\nexternal = { path = \"../../outside\" }\n",
        )
        .unwrap();
        let projects = vec![make_project("api", "api")];
        let deps = infer_depends_on(&api, ws, &projects);
        assert!(deps.is_empty());
    }

    #[test]
    fn package_json_without_scripts_or_main_is_skipped() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("data");
        fs::create_dir_all(&sub).unwrap();
        // package.json with no scripts/main/module — not a real project
        fs::write(
            sub.join("package.json"),
            r#"{"name":"data","version":"1.0"}"#,
        )
        .unwrap();

        let projects = discover_projects(dir.path(), 3, &[]);
        assert!(projects.is_empty());
    }
}
