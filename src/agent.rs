//! Central orchestrator: manages projects, tool registry, and shared state.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::project::ProjectConfig;
use crate::library::registry::LibraryRegistry;
use crate::memory::MemoryStore;

/// Shared agent state — cloned into each tool invocation.
#[derive(Clone)]
pub struct Agent {
    pub inner: Arc<RwLock<AgentInner>>,
}

pub struct AgentInner {
    pub active_project: Option<ActiveProject>,
}

pub struct ActiveProject {
    pub root: PathBuf,
    pub config: ProjectConfig,
    pub memory: MemoryStore,
    pub library_registry: LibraryRegistry,
}

impl Agent {
    pub async fn new(project: Option<PathBuf>) -> Result<Self> {
        let active_project = if let Some(root) = project {
            let config = ProjectConfig::load_or_default(&root)?;
            let memory = MemoryStore::open(&root)?;
            let registry_path = root.join(".code-explorer").join("libraries.json");
            let library_registry = LibraryRegistry::load(&registry_path).unwrap_or_default();
            Some(ActiveProject {
                root,
                config,
                memory,
                library_registry,
            })
        } else {
            None
        };

        Ok(Self {
            inner: Arc::new(RwLock::new(AgentInner { active_project })),
        })
    }

    /// Activate a project by path, replacing the current active project.
    pub async fn activate(&self, root: PathBuf) -> Result<()> {
        let config = ProjectConfig::load_or_default(&root)?;
        let memory = MemoryStore::open(&root)?;
        let registry_path = root.join(".code-explorer").join("libraries.json");
        let library_registry = LibraryRegistry::load(&registry_path).unwrap_or_default();
        let mut inner = self.inner.write().await;
        inner.active_project = Some(ActiveProject {
            root,
            config,
            memory,
            library_registry,
        });
        Ok(())
    }

    /// Get the active project root, or error if none is set.
    pub async fn require_project_root(&self) -> Result<PathBuf> {
        let inner = self.inner.read().await;
        inner
            .active_project
            .as_ref()
            .map(|p| p.root.clone())
            .ok_or_else(|| anyhow::anyhow!("No active project. Use activate_project first."))
    }

    /// Get the current project status for building server instructions.
    pub async fn project_status(&self) -> Option<crate::prompts::ProjectStatus> {
        let inner = self.inner.read().await;
        let project = inner.active_project.as_ref()?;
        let memories = project.memory.list().unwrap_or_default();
        let has_index = crate::embed::index::db_path(&project.root).exists();
        Some(crate::prompts::ProjectStatus {
            name: project.config.project.name.clone(),
            path: project.root.display().to_string(),
            languages: project.config.project.languages.clone(),
            memories,
            has_index,
        })
    }

    /// Get optional project root (None if no project active).
    pub async fn project_root(&self) -> Option<PathBuf> {
        let inner = self.inner.read().await;
        inner.active_project.as_ref().map(|p| p.root.clone())
    }

    /// Get the security config, or defaults if no project is active.
    pub async fn security_config(&self) -> crate::util::path_security::PathSecurityConfig {
        let inner = self.inner.read().await;
        match &inner.active_project {
            Some(p) => p.config.security.to_path_security_config(),
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
        let path = project.root.join(".code-explorer").join("libraries.json");
        project.library_registry.save(&path)
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
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        let agent = Agent::new(Some(dir.path().to_path_buf())).await.unwrap();
        let root = agent.require_project_root().await.unwrap();
        assert_eq!(root, dir.path());
    }

    #[tokio::test]
    async fn activate_sets_project() {
        let agent = Agent::new(None).await.unwrap();
        assert!(agent.require_project_root().await.is_err());

        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
        agent.activate(dir.path().to_path_buf()).await.unwrap();

        let root = agent.require_project_root().await.unwrap();
        assert_eq!(root, dir.path());
    }

    #[tokio::test]
    async fn activate_replaces_previous_project() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        std::fs::create_dir_all(dir1.path().join(".code-explorer")).unwrap();
        std::fs::create_dir_all(dir2.path().join(".code-explorer")).unwrap();

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
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
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
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
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
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
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
        std::fs::create_dir_all(dir.path().join(".code-explorer")).unwrap();
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
}
