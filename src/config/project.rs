//! Per-project configuration loaded from `.code-explorer/project.toml`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectSection,
    #[serde(default)]
    pub embeddings: EmbeddingsSection,
    #[serde(default)]
    pub ignored_paths: IgnoredPathsSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSection {
    pub name: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default = "default_encoding")]
    pub encoding: String,
    #[serde(default = "default_timeout")]
    pub tool_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingsSection {
    /// Model identifier: "local:jina-embeddings-v2-base-code",
    /// "openai:text-embedding-3-small", "ollama:nomic-embed-code"
    #[serde(default = "default_embed_model")]
    pub model: String,
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IgnoredPathsSection {
    #[serde(default = "default_ignored_patterns")]
    pub patterns: Vec<String>,
}

impl Default for EmbeddingsSection {
    fn default() -> Self {
        Self {
            model: default_embed_model(),
            chunk_size: default_chunk_size(),
            chunk_overlap: default_chunk_overlap(),
        }
    }
}

fn default_encoding() -> String {
    "utf-8".into()
}
fn default_timeout() -> u64 {
    60
}
fn default_embed_model() -> String {
    "ollama:nomic-embed-text".into()
}
fn default_chunk_size() -> usize {
    4000
}
fn default_chunk_overlap() -> usize {
    400
}
fn default_ignored_patterns() -> Vec<String> {
    vec![
        ".git".into(),
        "node_modules".into(),
        "target".into(),
        "__pycache__".into(),
        ".venv".into(),
        "dist".into(),
        "build".into(),
        ".code-explorer".into(),
    ]
}

impl ProjectConfig {
    /// Load from `.code-explorer/project.toml`, or return a sensible default
    /// derived from the directory name.
    pub fn load_or_default(root: &Path) -> Result<Self> {
        let config_path = root.join(".code-explorer").join("project.toml");
        if config_path.exists() {
            let text = std::fs::read_to_string(&config_path)?;
            Ok(toml::from_str(&text)?)
        } else {
            let name = root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unnamed")
                .to_string();
            Ok(Self::default_for(name))
        }
    }

    pub fn default_for(name: String) -> Self {
        Self {
            project: ProjectSection {
                name,
                languages: vec![],
                encoding: default_encoding(),
                tool_timeout_secs: default_timeout(),
            },
            embeddings: EmbeddingsSection::default(),
            ignored_paths: IgnoredPathsSection::default(),
        }
    }

    /// Path to the per-project data directory.
    pub fn data_dir(root: &Path) -> PathBuf {
        root.join(".code-explorer")
    }
}
