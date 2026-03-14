//! Per-project configuration loaded from `.codescout/project.toml`.

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
    #[serde(default)]
    pub memory: MemorySection,
    #[serde(default)]
    pub libraries: LibrariesSection,
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
    /// Deprecated: use `.codescout/system-prompt.md` instead.
    /// This field is still read as a fallback if the file doesn't exist.
    #[serde(default)]
    pub system_prompt: Option<String>,
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
    ///   "local:AllMiniLML6V2Q"              → 384d, INT8-quantized, ~22MB, CPU-safe
    ///   "local:AllMiniLML6V2Q"              → 384d, quantized, ~22MB, lightest
    ///   "local:BGESmallENV15"               → 384d, full precision
    #[serde(default = "default_embed_model")]
    pub model: String,
    /// Ignored — kept for backwards-compatible deserialisation of existing
    /// `project.toml` files that include a `chunk_size` key.
    ///
    /// Chunk size is now derived automatically from the model's published
    /// context window — see `embed::chunk_size_for_model`. Manual tuning was
    /// error-prone (too large → truncation and degraded recall; too small →
    /// unnecessary splitting of coherent functions).
    #[serde(default, skip_serializing, rename = "chunk_size")]
    pub _chunk_size_ignored: Option<usize>,
    /// Ignored — kept for backwards-compatible deserialisation of existing
    /// `project.toml` files that include a `chunk_overlap` key.
    ///
    /// Overlap is meaningless for AST-aware chunking (clean semantic boundaries)
    /// and was removed from the public API. The plain-text fallback paths also
    /// use 0 overlap so each sub-chunk is distinct.
    #[serde(default, skip_serializing, rename = "chunk_overlap")]
    pub _chunk_overlap_ignored: Option<usize>,
    /// Enable semantic drift detection during index builds (default: true).
    ///
    /// When enabled, `index_project` compares old and new chunk embeddings to
    /// score how much each file's *meaning* changed (not just its bytes). Results
    /// are stored in the `drift_report` table and surfaced via the `check_drift` tool.
    ///
    /// Experimental — reads all old embeddings before deletion, adding memory and
    /// DB overhead proportional to the number of changed files.
    ///
    /// Opt out in `.codescout/project.toml`:
    /// ```toml
    /// [embeddings]
    /// drift_detection_enabled = false
    /// ```
    #[serde(default = "default_drift_detection_enabled")]
    pub drift_detection_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IgnoredPathsSection {
    #[serde(default = "default_ignored_patterns")]
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Enable shell command execution (default: true)
    #[serde(default = "default_true")]
    pub shell_enabled: bool,
    /// Enable file write tools: create_file, edit_file, symbol write tools (default: true)
    #[serde(default = "default_true")]
    pub file_write_enabled: bool,
    /// Enable semantic search and indexing tools (default: true)
    #[serde(default = "default_true")]
    pub indexing_enabled: bool,
    /// Enable additional GitHub tools: github_identity, github_issue, github_pr, github_file.
    /// github_repo is always available. (default: false)
    #[serde(default)]
    pub github_enabled: bool,
    /// Command substrings that bypass dangerous-command detection.
    #[serde(default)]
    pub shell_allow_always: Vec<String>,
    /// Additional regex patterns to flag as dangerous commands.
    #[serde(default)]
    pub shell_dangerous_patterns: Vec<String>,
}

impl Default for SecuritySection {
    fn default() -> Self {
        Self {
            denied_read_patterns: Vec::new(),
            extra_write_roots: Vec::new(),
            shell_command_mode: default_shell_mode(),
            shell_output_limit_bytes: default_shell_output_limit(),
            shell_enabled: true,
            file_write_enabled: true,
            indexing_enabled: true,
            github_enabled: false,
            shell_allow_always: Vec::new(),
            shell_dangerous_patterns: Vec::new(),
        }
    }
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
            indexing_enabled: self.indexing_enabled,
            github_enabled: self.github_enabled,
            library_paths: Vec::new(),
            shell_allow_always: self.shell_allow_always.clone(),
            shell_dangerous_patterns: self.shell_dangerous_patterns.clone(),
        }
    }
}

fn default_drift_detection_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySection {
    /// Min drift score to trigger reverse-drift staleness flag (0.0-1.0)
    #[serde(default = "default_staleness_drift_threshold")]
    pub staleness_drift_threshold: f32,
    /// Min similarity for semantic anchor creation (0.0-1.0)
    #[serde(default = "default_semantic_anchor_min_similarity")]
    pub semantic_anchor_min_similarity: f32,
    /// Number of top chunks to consider for semantic anchoring
    #[serde(default = "default_semantic_anchor_top_n")]
    pub semantic_anchor_top_n: usize,
    /// Memory topics protected from blind overwrite during force re-onboarding.
    /// Protected topics go through a staleness-check + merge + user-approval flow.
    #[serde(default = "default_protected_topics")]
    pub protected: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrariesSection {
    #[serde(default)]
    pub auto_index: bool,
    #[serde(default)]
    pub auto_fetch_sources: bool,
    #[serde(default = "default_fetch_timeout")]
    pub fetch_timeout_secs: u64,
    #[serde(default)]
    pub version_overrides: std::collections::HashMap<String, String>,
}

fn default_fetch_timeout() -> u64 {
    300
}

impl Default for LibrariesSection {
    fn default() -> Self {
        Self {
            auto_index: false,
            auto_fetch_sources: false,
            fetch_timeout_secs: default_fetch_timeout(),
            version_overrides: std::collections::HashMap::new(),
        }
    }
}

fn default_staleness_drift_threshold() -> f32 {
    0.3
}
fn default_semantic_anchor_min_similarity() -> f32 {
    0.3
}
fn default_semantic_anchor_top_n() -> usize {
    10
}
fn default_protected_topics() -> Vec<String> {
    vec!["gotchas".to_string()]
}

impl Default for MemorySection {
    fn default() -> Self {
        Self {
            staleness_drift_threshold: default_staleness_drift_threshold(),
            semantic_anchor_min_similarity: default_semantic_anchor_min_similarity(),
            semantic_anchor_top_n: default_semantic_anchor_top_n(),
            protected: default_protected_topics(),
        }
    }
}

impl Default for EmbeddingsSection {
    fn default() -> Self {
        Self {
            model: default_embed_model(),
            _chunk_size_ignored: None,
            _chunk_overlap_ignored: None,
            drift_detection_enabled: default_drift_detection_enabled(),
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

fn default_ignored_patterns() -> Vec<String> {
    vec![
        ".git".into(),
        "node_modules".into(),
        "target".into(),
        "__pycache__".into(),
        ".venv".into(),
        "dist".into(),
        "build".into(),
        ".codescout".into(),
        ".worktrees".into(),
        ".claude".into(),
    ]
}

impl ProjectConfig {
    /// Load from `.codescout/project.toml`, or return a sensible default
    /// derived from the directory name.
    pub fn load_or_default(root: &Path) -> Result<Self> {
        let config_path = root.join(".codescout").join("project.toml");
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
                system_prompt: None,
            },
            embeddings: EmbeddingsSection::default(),
            ignored_paths: IgnoredPathsSection::default(),
            security: SecuritySection::default(),
            memory: MemorySection::default(),
            libraries: LibrariesSection::default(),
        }
    }

    /// Path to the per-project data directory.
    pub fn data_dir(root: &Path) -> PathBuf {
        root.join(".codescout")
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

    #[test]
    fn security_section_default_enables_write_git_indexing() {
        // Previously derived Default gave false for all bool fields,
        // silently disabling write tools for projects without a [security] TOML block.
        let sec = SecuritySection::default();
        assert!(
            sec.file_write_enabled,
            "file_write_enabled should default to true"
        );
        assert!(
            sec.indexing_enabled,
            "indexing_enabled should default to true"
        );
        assert!(sec.shell_enabled, "shell_enabled should default to true");
        assert!(
            !sec.github_enabled,
            "github_enabled should default to false"
        );
    }

    #[test]
    fn project_config_default_for_enables_write_tools() {
        // default_for() is used when no .codescout/project.toml exists.
        let cfg = ProjectConfig::default_for("test-project".into());
        assert!(cfg.security.file_write_enabled);
        assert!(cfg.security.indexing_enabled);
        assert!(cfg.security.shell_enabled);
        assert!(!cfg.security.github_enabled);
    }

    #[test]
    fn toml_without_security_section_enables_write_tools() {
        // When [security] is entirely absent from TOML, serde calls Default::default()
        // for SecuritySection. This must agree with the serde field-level defaults.
        let toml = "[project]\nname = \"test\"";
        let cfg: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(cfg.security.file_write_enabled);
        assert!(cfg.security.indexing_enabled);
        assert!(cfg.security.shell_enabled);
        assert!(!cfg.security.github_enabled);
    }

    #[test]
    fn system_prompt_defaults_to_none() {
        let toml = "[project]\nname = \"test\"";
        let cfg: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(cfg.project.system_prompt.is_none());
    }

    #[test]
    fn system_prompt_parses_from_toml() {
        let toml = "[project]\nname = \"test\"\nsystem_prompt = \"Use pytest for testing.\"";
        let cfg: ProjectConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.project.system_prompt.as_deref(),
            Some("Use pytest for testing.")
        );
    }

    #[test]
    fn memory_section_defaults() {
        let toml = "[project]\nname = \"test\"";
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!((config.memory.staleness_drift_threshold - 0.3).abs() < 0.01);
        assert!((config.memory.semantic_anchor_min_similarity - 0.3).abs() < 0.01);
        assert_eq!(config.memory.semantic_anchor_top_n, 10);
    }

    #[test]
    fn memory_section_override() {
        let toml = "[project]\nname = \"test\"\n[memory]\nstaleness_drift_threshold = 0.5\n";
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!((config.memory.staleness_drift_threshold - 0.5).abs() < 0.01);
    }

    #[test]
    fn memory_section_default_includes_gotchas() {
        let section = MemorySection::default();
        assert_eq!(section.protected, vec!["gotchas".to_string()]);
    }

    #[test]
    fn memory_section_serde_roundtrip_with_protected() {
        let toml_str = r#"
staleness_drift_threshold = 0.3
protected = ["gotchas", "conventions"]
"#;
        let section: MemorySection = toml::from_str(toml_str).unwrap();
        assert_eq!(
            section.protected,
            vec!["gotchas".to_string(), "conventions".to_string()]
        );

        // Round-trip
        let serialized = toml::to_string_pretty(&section).unwrap();
        let deserialized: MemorySection = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.protected, section.protected);
    }

    #[test]
    fn memory_section_missing_protected_uses_default() {
        let toml_str = r#"
staleness_drift_threshold = 0.3
"#;
        let section: MemorySection = toml::from_str(toml_str).unwrap();
        assert_eq!(section.protected, vec!["gotchas".to_string()]);
    }

    #[test]
    fn project_config_deserializes_libraries_section() {
        let toml = r#"
[project]
name = "test"

[libraries]
auto_index = true
auto_fetch_sources = true
fetch_timeout_secs = 120
"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(config.libraries.auto_index);
        assert!(config.libraries.auto_fetch_sources);
        assert_eq!(config.libraries.fetch_timeout_secs, 120);
    }

    #[test]
    fn project_config_libraries_defaults() {
        let toml = "[project]\nname = \"test\"\n";
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(!config.libraries.auto_index);
        assert!(!config.libraries.auto_fetch_sources);
        assert_eq!(config.libraries.fetch_timeout_secs, 300);
    }
}
