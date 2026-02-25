//! Central orchestrator: manages projects, tool registry, and shared state.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::project::ProjectConfig;
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
}

impl Agent {
    pub async fn new(project: Option<PathBuf>) -> Result<Self> {
        let active_project = if let Some(root) = project {
            let config = ProjectConfig::load_or_default(&root)?;
            let memory = MemoryStore::open(&root)?;
            Some(ActiveProject {
                root,
                config,
                memory,
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
        let mut inner = self.inner.write().await;
        inner.active_project = Some(ActiveProject {
            root,
            config,
            memory,
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
}
