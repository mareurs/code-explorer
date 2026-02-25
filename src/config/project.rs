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
    #[serde(default)]
    pub security: SecuritySection,
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
    /// Model identifier — prefix determines the backend:
    ///   "ollama:<model>"                    → Ollama local daemon (default)
    ///   "openai:<model>"                    → OpenAI API (requires OPENAI_API_KEY)
    ///   "custom:<model>@<base_url>"         → Any OpenAI-compatible endpoint
    ///   "local:<EmbeddingModel variant>"    → fastembed-rs, no daemon needed,
    ///                                         CPU/WSL2-friendly. Downloads model
    ///                                         on first use to ~/.cache/huggingface/
    ///
    /// Recommended local models (rebuild with: cargo build --features local-embed):
    ///   "local:JinaEmbeddingsV2BaseCode"    → 768d, code-specific, ~300MB
    ///   "local:BGESmallENV15Q"              → 384d, quantized, ~20MB, fast CPU
    ///   "local:AllMiniLML6V2Q"              → 384d, quantized, ~22MB, lightest
    ///   "local:BGESmallENV15"               → 384d, full precision
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecuritySection {
    /// Additional path patterns to deny reads from (beyond built-in deny-list).
    #[serde(default)]
    pub denied_read_patterns: Vec<String>,
    /// Additional directories where writes are allowed (beyond project root).
    #[serde(default)]
    pub extra_write_roots: Vec<String>,
    /// Shell command mode: "unrestricted", "warn" (default), "disabled"
    #[serde(default = "default_shell_mode")]
    pub shell_command_mode: String,
    /// Max bytes for shell command stdout/stderr (default 100KB)
    #[serde(default = "default_shell_output_limit")]
    pub shell_output_limit_bytes: usize,
    /// Enable shell command execution (default: false — disabled for safety)
    #[serde(default)]
    pub shell_enabled: bool,
    /// Enable file write tools: create_text_file, replace_content, symbol write tools (default: true)
    #[serde(default = "default_true")]
    pub file_write_enabled: bool,
    /// Enable git tools: blame, log, diff (default: true)
    #[serde(default = "default_true")]
    pub git_enabled: bool,
    /// Enable semantic search and indexing tools (default: true)
    #[serde(default = "default_true")]
    pub indexing_enabled: bool,
}

fn default_shell_mode() -> String {
    "warn".into()
}

fn default_shell_output_limit() -> usize {
    100 * 1024 // 100KB
}

fn default_true() -> bool {
    true
}

impl SecuritySection {
    pub fn to_path_security_config(&self) -> crate::util::path_security::PathSecurityConfig {
        crate::util::path_security::PathSecurityConfig {
            denied_read_patterns: self.denied_read_patterns.clone(),
            extra_write_roots: self
                .extra_write_roots
                .iter()
                .map(std::path::PathBuf::from)
                .collect(),
            shell_command_mode: self.shell_command_mode.clone(),
            shell_output_limit_bytes: self.shell_output_limit_bytes,
            shell_enabled: self.shell_enabled,
            file_write_enabled: self.file_write_enabled,
            git_enabled: self.git_enabled,
            indexing_enabled: self.indexing_enabled,
        }
    }
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
    "ollama:mxbai-embed-large".into()
}
fn default_chunk_size() -> usize {
    1200
}
fn default_chunk_overlap() -> usize {
    200
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
            security: SecuritySection::default(),
        }
    }

    /// Path to the per-project data directory.
    pub fn data_dir(root: &Path) -> PathBuf {
        root.join(".code-explorer")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_is_mxbai() {
        assert_eq!(default_embed_model(), "ollama:mxbai-embed-large");
    }

    #[test]
    fn default_config_has_mxbai_model() {
        let cfg = ProjectConfig::default_for("my-project".into());
        assert_eq!(cfg.embeddings.model, "ollama:mxbai-embed-large");
    }
}
