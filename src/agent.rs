//! Central orchestrator: manages projects, tool registry, and shared state.

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::project::ProjectConfig;
use crate::library::registry::LibraryRegistry;
use crate::memory::MemoryStore;

/// Shared agent state — cloned into each tool invocation.
/// Cached embedder: `(model_name, embedder)` — invalidated on model change.
/// `Arc<dyn Embedder>`: concrete type selected at runtime from a config string (e.g. `"openai"`, `"ollama"`); generics cannot express this.
type CachedEmbedder = Arc<tokio::sync::Mutex<Option<(String, Arc<dyn crate::embed::Embedder>)>>>;

/// State of the background index-build task spawned by `index_project`.
#[derive(Default, Clone)]
pub enum IndexingState {
    #[default]
    Idle,
    Running {
        done: usize,
        total: usize,
        eta_secs: Option<u64>,
    },
    Done {
        files_indexed: usize,
        files_deleted: usize,
        detail: String,
        total_files: usize,
        total_chunks: usize,
    },
    Failed(String),
}

/// Tracks the indexing lifecycle of a single external library.
#[derive(Debug)]
pub enum LibraryIndexState {
    Idle,
    FetchingSources { command: String },
    Indexing { done: usize, total: usize },
    Done { chunks: usize, version: String },
    Failed(String),
}

#[derive(Clone)]
pub struct Agent {
    pub inner: Arc<RwLock<AgentInner>>,
    /// Stored outside the RwLock so creation doesn't block agent reads.
    /// Mutex deduplicates concurrent creation: second caller waits and reuses.
    cached_embedder: CachedEmbedder,
    /// Tracks the background index-build task. Stored outside AgentInner
    /// so callers only need a brief std::sync lock, not an async RwLock.
    pub indexing: Arc<std::sync::Mutex<IndexingState>>,
    /// Per-session dedup for library nudge hints (e.g. "index this library").
    /// Wrapped in Arc so Agent remains Clone.
    pub nudged_libraries: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Limits concurrent embedding API calls to avoid overwhelming the embedding server.
    pub embedding_semaphore: Arc<tokio::sync::Semaphore>,
    /// Per-library indexing state (Idle / FetchingSources / Indexing / Done / Failed).
    pub library_index_states: Arc<std::sync::Mutex<HashMap<String, LibraryIndexState>>>,
}

pub struct AgentInner {
    pub active_project: Option<ActiveProject>,
    pub project_explicitly_activated: bool,
    pub home_root: Option<PathBuf>,
}

pub struct ActiveProject {
    pub root: PathBuf,
    pub config: ProjectConfig,
    pub memory: MemoryStore,
    pub private_memory: MemoryStore,
    pub library_registry: LibraryRegistry,
    /// Tracks files written by tools in this session but not yet re-indexed.
    /// Wrapped in an Arc so index_project can capture it across a tokio::spawn
    /// boundary and clear it on successful completion.
    pub dirty_files: Arc<std::sync::Mutex<std::collections::HashSet<PathBuf>>>,
}

impl Agent {
    pub async fn new(project: Option<PathBuf>) -> Result<Self> {
        let (active_project, home_root) = if let Some(root) = project {
            let config = ProjectConfig::load_or_default(&root)?;
            let memory = MemoryStore::open(&root)?;
            let private_memory = MemoryStore::open_private(&root)?;
            let registry_path = root.join(".codescout").join("libraries.json");
            let library_registry = LibraryRegistry::load(&registry_path).unwrap_or_default();
            let home = root.clone();
            (
                Some(ActiveProject {
                    root,
                    config,
                    memory,
                    private_memory,
                    library_registry,
                    dirty_files: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
                }),
                Some(home),
            )
        } else {
            (None, None)
        };

        // A project provided at startup (via --project or CWD) is treated as explicitly
        // activated — the server operator already chose the write target.
        let project_explicitly_activated = active_project.is_some();

        Ok(Self {
            inner: Arc::new(RwLock::new(AgentInner {
                active_project,
                project_explicitly_activated,
                home_root,
            })),
            cached_embedder: Arc::new(tokio::sync::Mutex::new(None)),
            indexing: Arc::new(std::sync::Mutex::new(IndexingState::Idle)),
            nudged_libraries: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            embedding_semaphore: Arc::new(tokio::sync::Semaphore::new(2)),
            library_index_states: Arc::new(std::sync::Mutex::new(HashMap::new())),
        })
    }

    /// Activate a project by path, replacing the current active project.
    pub async fn activate(&self, root: PathBuf) -> Result<()> {
        let config = ProjectConfig::load_or_default(&root)?;
        let memory = MemoryStore::open(&root)?;
        let private_memory = MemoryStore::open_private(&root)?;
        let registry_path = root.join(".codescout").join("libraries.json");
        let library_registry = LibraryRegistry::load(&registry_path).unwrap_or_default();
        let mut inner = self.inner.write().await;
        if inner.home_root.is_none() {
            inner.home_root = Some(root.clone());
        }
        inner.active_project = Some(ActiveProject {
            root,
            config,
            memory,
            private_memory,
            library_registry,
            dirty_files: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        });
        inner.project_explicitly_activated = true;
        // Clear cached embedder — new project may use a different model
        *self.cached_embedder.lock().await = None;
        Ok(())
    }

    /// Get or create a cached embedder for the given model.
    /// If the cached model matches, returns the existing embedder.
    /// The Mutex deduplicates concurrent creation — second caller waits and reuses.
    pub async fn get_or_create_embedder(
        &self,
        model: &str,
    ) -> anyhow::Result<Arc<dyn crate::embed::Embedder>> {
        let mut guard = self.cached_embedder.lock().await;
        if let Some((cached_model, embedder)) = guard.as_ref() {
            if cached_model == model {
                return Ok(Arc::clone(embedder));
            }
        }
        let embedder: Arc<dyn crate::embed::Embedder> =
            Arc::from(crate::embed::create_embedder(model).await?);
        *guard = Some((model.to_string(), Arc::clone(&embedder)));
        Ok(embedder)
    }

    /// Get the active project root, or error if none is set.
    pub async fn require_project_root(&self) -> Result<PathBuf> {
        let inner = self.inner.read().await;
        inner
            .active_project
            .as_ref()
            .map(|p| p.root.clone())
            .ok_or_else(|| {
                crate::tools::RecoverableError::with_hint(
                    "No active project. Use activate_project first.",
                    "Call activate_project(\"/path/to/project\") to set the active project.",
                )
                .into()
            })
    }

    /// Mark a file as written-but-not-yet-indexed.
    /// Called by every write tool after modifying a source file.
    pub async fn mark_file_dirty(&self, path: PathBuf) {
        let inner = self.inner.read().await;
        if let Some(p) = &inner.active_project {
            p.dirty_files
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(path);
        }
    }

    /// Number of files written in this session but not yet re-indexed.
    pub async fn dirty_file_count(&self) -> usize {
        let inner = self.inner.read().await;
        inner
            .active_project
            .as_ref()
            .map(|p| {
                p.dirty_files
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .len()
            })
            .unwrap_or(0)
    }

    /// Clone the dirty-files Arc so index_project can capture it across a spawn boundary
    /// and clear it on successful completion.
    pub async fn dirty_files_arc(
        &self,
    ) -> Option<Arc<std::sync::Mutex<std::collections::HashSet<PathBuf>>>> {
        let inner = self.inner.read().await;
        inner.active_project.as_ref().map(|p| p.dirty_files.clone())
    }

    /// Get the current project status for building server instructions.
    pub async fn project_status(&self) -> Option<crate::prompts::ProjectStatus> {
        let inner = self.inner.read().await;
        let project = inner.active_project.as_ref()?;
        let memories = project.memory.list().unwrap_or_default();
        let has_index = crate::embed::index::project_db_path(&project.root).exists();
        let github_enabled = project.config.security.github_enabled;

        // Read system prompt: file takes precedence over TOML field
        let prompt_file = project.root.join(".codescout").join("system-prompt.md");
        let system_prompt = if prompt_file.exists() {
            std::fs::read_to_string(&prompt_file).ok()
        } else {
            project.config.project.system_prompt.clone()
        };

        Some(crate::prompts::ProjectStatus {
            name: project.config.project.name.clone(),
            path: project.root.display().to_string(),
            languages: project.config.project.languages.clone(),
            memories,
            has_index,
            system_prompt,
            github_enabled,
        })
    }

    /// Get optional project root (None if no project active).
    pub async fn project_root(&self) -> Option<PathBuf> {
        let inner = self.inner.read().await;
        inner.active_project.as_ref().map(|p| p.root.clone())
    }

    pub async fn is_project_explicitly_activated(&self) -> bool {
        self.inner.read().await.project_explicitly_activated
    }

    /// Return the home project root (the first project activated in this session).
    pub async fn home_root(&self) -> Option<PathBuf> {
        self.inner.read().await.home_root.clone()
    }

    /// True when the active project is the home project (or both are None).
    pub async fn is_home(&self) -> bool {
        let inner = self.inner.read().await;
        match (&inner.active_project, &inner.home_root) {
            (Some(project), Some(home)) => project.root == *home,
            (None, None) => true,
            _ => false,
        }
    }

    /// Get the security config, or defaults if no project is active.
    /// Populates `library_paths` from the active project's library registry.
    pub async fn security_config(&self) -> crate::util::path_security::PathSecurityConfig {
        let inner = self.inner.read().await;
        match &inner.active_project {
            Some(p) => {
                let mut config = p.config.security.to_path_security_config();
                config.library_paths = p
                    .library_registry
                    .all()
                    .iter()
                    .map(|e| e.path.clone())
                    .collect();
                config
            }
            None => crate::util::path_security::PathSecurityConfig::default(),
        }
    }

    /// Run a closure with a read-lock on the active project.
    /// Returns an error if no project is active.
    pub async fn with_project<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&ActiveProject) -> Result<T>,
    {
        let inner = self.inner.read().await;
        let project = inner
            .active_project
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active project. Use activate_project first."))?;
        f(project)
    }

    /// Get a clone of the library registry, if a project is active.
    pub async fn library_registry(&self) -> Option<LibraryRegistry> {
        self.inner
            .read()
            .await
            .active_project
            .as_ref()
            .map(|p| p.library_registry.clone())
    }

    /// Persist the library registry to disk.
    pub async fn save_library_registry(&self) -> Result<()> {
        let inner = self.inner.read().await;
        let project = inner
            .active_project
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No active project"))?;
        let path = project.root.join(".codescout").join("libraries.json");
        project.library_registry.save(&path)
    }

    /// Check if we should nudge about a library. Returns true at most once per
    /// session per library, and respects the persistent `nudge_dismissed` flag.
    pub async fn should_nudge(&self, lib_name: &str) -> bool {
        // Check persistent dismissal and indexed status
        let inner = self.inner.read().await;
        if let Some(p) = &inner.active_project {
            if let Some(entry) = p.library_registry.lookup(lib_name) {
                if entry.nudge_dismissed || entry.indexed {
                    return false;
                }
            }
        }
        drop(inner);

        // Check session dedup — insert returns true if the value was NEW
        let mut nudged = self
            .nudged_libraries
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        nudged.insert(lib_name.to_string())
    }

    /// If `path` is the active project's `.codescout/project.toml`, reload the
    /// in-memory config from disk. Called by `edit_file` after every successful
    /// write so that tools like `semantic_search` see the updated model immediately
    /// without requiring a session restart.
    pub async fn reload_config_if_project_toml(&self, path: &std::path::Path) {
        let mut inner = self.inner.write().await;
        if let Some(ref mut p) = inner.active_project {
            let toml_path = p.root.join(".codescout").join("project.toml");
            if path == toml_path {
                if let Ok(fresh) = crate::config::project::ProjectConfig::load_or_default(&p.root) {
                    p.config = fresh;
                }
            }
        }
    }

    /// Update the indexing state for a named library.
    pub fn set_library_state(&self, name: &str, state: LibraryIndexState) {
        let mut states = self
            .library_index_states
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        states.insert(name.to_string(), state);
    }

    /// Spawn a background library indexing task if auto_index is enabled and library is not yet indexed.
    pub async fn maybe_auto_index_library(&self, lib_name: &str) {
        let (should_index, root, entry_path) = {
            let inner = self.inner.read().await;
            let Some(p) = &inner.active_project else {
                return;
            };
            if !p.config.libraries.auto_index {
                return;
            }
            let Some(entry) = p.library_registry.lookup(lib_name) else {
                return;
            };
            if entry.indexed {
                return;
            }
            (true, p.root.clone(), entry.path.clone())
        };
        if !should_index {
            return;
        }

        let name = lib_name.to_string();
        let source = format!("lib:{}", name);
        self.set_library_state(&name, LibraryIndexState::Indexing { done: 0, total: 0 });

        let self_clone = self.clone();
        tokio::spawn(async move {
            tracing::info!("Auto-indexing library '{}' in background...", name);
            let result =
                crate::embed::index::build_library_index(&root, &entry_path, &source, false).await;
            match result {
                Ok(()) => {
                    let mut inner = self_clone.inner.write().await;
                    if let Some(p) = inner.active_project.as_mut() {
                        if let Some(entry) = p.library_registry.lookup_mut(&name) {
                            entry.indexed = true;
                        }
                        let reg_path = p.root.join(".codescout/libraries.json");
                        let _ = p.library_registry.save(&reg_path);
                    }
                    drop(inner);
                    self_clone.set_library_state(
                        &name,
                        LibraryIndexState::Done {
                            chunks: 0,
                            version: String::new(),
                        },
                    );
                }
                Err(e) => {
                    self_clone.set_library_state(&name, LibraryIndexState::Failed(e.to_string()));
                }
            }
        });
    }

    /// Return a human-readable summary string for each tracked library.
    pub fn library_states_summary(&self) -> HashMap<String, String> {
        let states = self
            .library_index_states
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        states
            .iter()
            .map(|(k, v)| {
                let status = match v {
                    LibraryIndexState::Idle => "idle".to_string(),
                    LibraryIndexState::FetchingSources { command } => {
                        format!("fetching_sources: {}", command)
                    }
                    LibraryIndexState::Indexing { done, total } => {
                        format!("indexing: {}/{}", done, total)
                    }
                    LibraryIndexState::Done { chunks, version } => {
                        format!("done: {} chunks (v{})", chunks, version)
                    }
                    LibraryIndexState::Failed(msg) => format!("failed: {}", msg),
                };
                (k.clone(), status)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn new_without_project() {
        let agent = Agent::new(None).await.unwrap();
        assert!(agent.require_project_root().await.is_err());
        assert!(agent.project_status().await.is_none());
    }

    #[tokio::test]
    async fn new_with_valid_project() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let root = agent.require_project_root().await.unwrap();
        assert_eq!(root, dir.path());
    }

    #[tokio::test]
    async fn activate_sets_project() {
        let agent = Agent::new(None).await.unwrap();
        assert!(agent.require_project_root().await.is_err());

        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        agent.activate(dir.path().to_path_buf()).await.unwrap();

        let root = agent.require_project_root().await.unwrap();
        assert_eq!(root, dir.path());
    }

    #[tokio::test]
    async fn activate_replaces_previous_project() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        std::fs::create_dir_all(dir1.path().join(".codescout")).unwrap();
        std::fs::create_dir_all(dir2.path().join(".codescout")).unwrap();

        let agent = Agent::new(Some(dir1.path().to_path_buf())).await.unwrap();
        assert_eq!(agent.require_project_root().await.unwrap(), dir1.path());

        agent.activate(dir2.path().to_path_buf()).await.unwrap();
        assert_eq!(agent.require_project_root().await.unwrap(), dir2.path());
    }

    #[tokio::test]
    async fn require_project_root_error_message() {
        let agent = Agent::new(None).await.unwrap();
        let err = agent.require_project_root().await.unwrap_err();
        assert!(
            err.to_string().contains("No active project"),
            "error should mention no active project: {}",
            err
        );
    }

    #[tokio::test]
    async fn with_project_errors_when_none() {
        let agent = Agent::new(None).await.unwrap();
        let result = agent.with_project(|_p| Ok(42)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn with_project_runs_closure() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();

        let name = agent
            .with_project(|p| Ok(p.config.project.name.clone()))
            .await
            .unwrap();
        // Default config uses directory name
        assert!(!name.is_empty());
    }

    #[tokio::test]
    async fn project_status_returns_none_without_project() {
        let agent = Agent::new(None).await.unwrap();
        assert!(agent.project_status().await.is_none());
    }

    #[tokio::test]
    async fn project_status_returns_some_with_project() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();

        let status = agent.project_status().await;
        assert!(status.is_some());
        let status = status.unwrap();
        assert!(!status.name.is_empty());
        assert!(status.path.contains(dir.path().to_str().unwrap()));
    }

    #[tokio::test]
    async fn agent_is_clone_safe() {
        // Agent wraps Arc<RwLock<...>> so clones share state
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(None).await.unwrap();
        let agent2 = agent.clone();

        agent.activate(dir.path().to_path_buf()).await.unwrap();
        // Clone should see the activation
        let root = agent2.require_project_root().await.unwrap();
        assert_eq!(root, dir.path());
    }

    #[tokio::test]
    async fn activate_creates_empty_library_registry() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();

        let reg = agent.library_registry().await.unwrap();
        assert!(
            reg.all().is_empty(),
            "fresh project should have empty library registry"
        );
    }

    #[tokio::test]
    async fn library_registry_none_without_project() {
        let agent = Agent::new(None).await.unwrap();
        assert!(agent.library_registry().await.is_none());
    }

    #[tokio::test]
    async fn project_status_reads_system_prompt_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".codescout");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("project.toml"),
            "[project]\nname = \"test\"\n",
        )
        .unwrap();
        std::fs::write(config_dir.join("system-prompt.md"), "Always use pytest.\n").unwrap();

        let agent = Agent::new(None).await.unwrap();
        agent.activate(dir.path().to_path_buf()).await.unwrap();
        let status = agent.project_status().await.unwrap();
        assert_eq!(
            status.system_prompt.as_deref(),
            Some("Always use pytest.\n")
        );
    }

    #[tokio::test]
    async fn project_status_falls_back_to_toml_system_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".codescout");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("project.toml"),
            "[project]\nname = \"test\"\nsystem_prompt = \"From TOML\"\n",
        )
        .unwrap();

        let agent = Agent::new(None).await.unwrap();
        agent.activate(dir.path().to_path_buf()).await.unwrap();
        let status = agent.project_status().await.unwrap();
        assert_eq!(status.system_prompt.as_deref(), Some("From TOML"));
    }

    #[tokio::test]
    async fn project_status_file_takes_precedence_over_toml() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".codescout");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("project.toml"),
            "[project]\nname = \"test\"\nsystem_prompt = \"From TOML\"\n",
        )
        .unwrap();
        std::fs::write(config_dir.join("system-prompt.md"), "From file\n").unwrap();

        let agent = Agent::new(None).await.unwrap();
        agent.activate(dir.path().to_path_buf()).await.unwrap();
        let status = agent.project_status().await.unwrap();
        assert_eq!(status.system_prompt.as_deref(), Some("From file\n"));
    }

    #[tokio::test]
    async fn project_not_explicitly_activated_without_project() {
        let agent = Agent::new(None).await.unwrap();
        assert!(!agent.is_project_explicitly_activated().await);
    }

    #[tokio::test]
    async fn activate_sets_explicitly_activated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(None).await.unwrap();
        agent.activate(dir.path().to_path_buf()).await.unwrap();
        assert!(agent.is_project_explicitly_activated().await);
    }

    #[tokio::test]
    async fn new_with_project_sets_explicitly_activated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        assert!(agent.is_project_explicitly_activated().await);
    }

    #[tokio::test]
    async fn home_root_set_from_initial_project() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        assert_eq!(agent.home_root().await, Some(dir.path().to_path_buf()));
    }

    #[tokio::test]
    async fn home_root_none_without_project() {
        let agent = Agent::new(None).await.unwrap();
        assert_eq!(agent.home_root().await, None);
    }

    #[tokio::test]
    async fn home_root_set_on_first_activate() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(None).await.unwrap();
        agent.activate(dir.path().to_path_buf()).await.unwrap();
        assert_eq!(agent.home_root().await, Some(dir.path().to_path_buf()));
    }

    #[tokio::test]
    async fn home_root_not_changed_by_second_activate() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        std::fs::create_dir_all(dir1.path().join(".codescout")).unwrap();
        std::fs::create_dir_all(dir2.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir1.path().to_path_buf())).await.unwrap();
        agent.activate(dir2.path().to_path_buf()).await.unwrap();
        assert_eq!(agent.home_root().await, Some(dir1.path().to_path_buf()));
    }

    #[tokio::test]
    async fn is_home_true_when_at_home() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        assert!(agent.is_home().await);
    }

    #[tokio::test]
    async fn is_home_false_after_switching() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        std::fs::create_dir_all(dir1.path().join(".codescout")).unwrap();
        std::fs::create_dir_all(dir2.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir1.path().to_path_buf())).await.unwrap();
        agent.activate(dir2.path().to_path_buf()).await.unwrap();
        assert!(!agent.is_home().await);
    }

    #[tokio::test]
    async fn is_home_true_after_returning() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        std::fs::create_dir_all(dir1.path().join(".codescout")).unwrap();
        std::fs::create_dir_all(dir2.path().join(".codescout")).unwrap();
        let agent = Agent::new(Some(dir1.path().to_path_buf())).await.unwrap();
        agent.activate(dir2.path().to_path_buf()).await.unwrap();
        assert!(!agent.is_home().await);
        agent.activate(dir1.path().to_path_buf()).await.unwrap();
        assert!(agent.is_home().await);
    }

    #[tokio::test]
    async fn active_project_has_private_memory() {
        let dir = tempdir().unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        agent
            .with_project(|p| {
                p.private_memory.write("pref", "verbose")?;
                assert_eq!(p.private_memory.read("pref")?, Some("verbose".to_string()));
                // private is isolated from shared
                assert_eq!(p.memory.read("pref")?, None);
                Ok(())
            })
            .await
            .unwrap();
    }
}
