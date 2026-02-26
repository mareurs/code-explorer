//! Library registry: tracks external libraries available for cross-reference search.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A registered external library that code-explorer can search into.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LibraryEntry {
    pub name: String,
    pub version: Option<String>,
    pub path: PathBuf,
    pub language: String,
    pub discovered_via: DiscoveryMethod,
    pub indexed: bool,
}

/// How the library was discovered and registered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryMethod {
    LspFollowThrough,
    Manual,
}

/// Persistent registry of known external libraries.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibraryRegistry {
    entries: Vec<LibraryEntry>,
}

impl LibraryRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Load from a JSON file. Returns an empty registry if the file does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let data = std::fs::read_to_string(path)?;
        let registry: LibraryRegistry = serde_json::from_str(&data)?;
        Ok(registry)
    }

    /// Persist the registry to a JSON file.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)?;
        Ok(())
    }

    /// Register a library. If a library with the same name exists, update its
    /// path/language/discovered_via. The `indexed` flag is preserved when the
    /// path has not changed; otherwise it resets to `false`.
    pub fn register(
        &mut self,
        name: String,
        path: PathBuf,
        language: String,
        discovered_via: DiscoveryMethod,
    ) {
        if let Some(existing) = self.entries.iter_mut().find(|e| e.name == name) {
            let path_changed = existing.path != path;
            existing.path = path;
            existing.language = language;
            existing.discovered_via = discovered_via;
            if path_changed {
                existing.indexed = false;
            }
        } else {
            self.entries.push(LibraryEntry {
                name,
                version: None,
                path,
                language,
                indexed: false,
                discovered_via,
            });
        }
    }

    /// Look up a library by name.
    pub fn lookup(&self, name: &str) -> Option<&LibraryEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Look up a library by name (mutable).
    pub fn lookup_mut(&mut self, name: &str) -> Option<&mut LibraryEntry> {
        self.entries.iter_mut().find(|e| e.name == name)
    }

    /// Return all registered libraries.
    pub fn all(&self) -> &[LibraryEntry] {
        &self.entries
    }

    /// Resolve a relative path within a library's root directory.
    pub fn resolve_path(&self, name: &str, relative: &str) -> Result<PathBuf> {
        let entry = self
            .lookup(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown library: {}", name))?;
        Ok(entry.path.join(relative))
    }

    /// Check whether an absolute path falls inside any registered library.
    /// Returns the matching entry if found.
    pub fn is_library_path(&self, absolute_path: &Path) -> Option<&LibraryEntry> {
        self.entries
            .iter()
            .find(|e| absolute_path.starts_with(&e.path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn json_roundtrip_library_entry() {
        let entry = LibraryEntry {
            name: "serde".into(),
            version: Some("1.0.200".into()),
            path: PathBuf::from("/home/user/.cargo/registry/serde-1.0.200"),
            language: "rust".into(),
            discovered_via: DiscoveryMethod::LspFollowThrough,
            indexed: true,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let restored: LibraryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, restored);
    }

    #[test]
    fn discovery_method_serializes_as_snake_case() {
        let lsp = serde_json::to_string(&DiscoveryMethod::LspFollowThrough).unwrap();
        assert_eq!(lsp, "\"lsp_follow_through\"");

        let manual = serde_json::to_string(&DiscoveryMethod::Manual).unwrap();
        assert_eq!(manual, "\"manual\"");
    }

    #[test]
    fn register_and_lookup() {
        let mut reg = LibraryRegistry::new();
        reg.register(
            "tokio".into(),
            PathBuf::from("/libs/tokio"),
            "rust".into(),
            DiscoveryMethod::Manual,
        );

        let entry = reg.lookup("tokio").unwrap();
        assert_eq!(entry.name, "tokio");
        assert_eq!(entry.path, PathBuf::from("/libs/tokio"));
        assert_eq!(entry.language, "rust");
        assert!(!entry.indexed);
        assert_eq!(entry.discovered_via, DiscoveryMethod::Manual);

        assert!(reg.lookup("nonexistent").is_none());
    }

    #[test]
    fn register_updates_existing_entry() {
        let mut reg = LibraryRegistry::new();
        reg.register(
            "serde".into(),
            PathBuf::from("/libs/serde-1.0"),
            "rust".into(),
            DiscoveryMethod::Manual,
        );

        // Mark as indexed
        reg.lookup_mut("serde").unwrap().indexed = true;

        // Re-register with same path: indexed should be preserved
        reg.register(
            "serde".into(),
            PathBuf::from("/libs/serde-1.0"),
            "rust".into(),
            DiscoveryMethod::LspFollowThrough,
        );
        let entry = reg.lookup("serde").unwrap();
        assert!(
            entry.indexed,
            "indexed should be preserved when path unchanged"
        );
        assert_eq!(entry.discovered_via, DiscoveryMethod::LspFollowThrough);

        // Re-register with different path: indexed should reset
        reg.register(
            "serde".into(),
            PathBuf::from("/libs/serde-2.0"),
            "rust".into(),
            DiscoveryMethod::Manual,
        );
        let entry = reg.lookup("serde").unwrap();
        assert!(!entry.indexed, "indexed should reset when path changes");
        assert_eq!(entry.path, PathBuf::from("/libs/serde-2.0"));
    }

    #[test]
    fn is_library_path_matches_and_misses() {
        let mut reg = LibraryRegistry::new();
        reg.register(
            "tokio".into(),
            PathBuf::from("/libs/tokio"),
            "rust".into(),
            DiscoveryMethod::Manual,
        );
        reg.register(
            "serde".into(),
            PathBuf::from("/libs/serde"),
            "rust".into(),
            DiscoveryMethod::Manual,
        );

        // Path inside tokio
        let found = reg.is_library_path(Path::new("/libs/tokio/src/runtime.rs"));
        assert_eq!(found.unwrap().name, "tokio");

        // Path inside serde
        let found = reg.is_library_path(Path::new("/libs/serde/src/de.rs"));
        assert_eq!(found.unwrap().name, "serde");

        // Path outside any library
        assert!(reg.is_library_path(Path::new("/other/file.rs")).is_none());
    }

    #[test]
    fn resolve_path_works() {
        let mut reg = LibraryRegistry::new();
        reg.register(
            "tokio".into(),
            PathBuf::from("/libs/tokio"),
            "rust".into(),
            DiscoveryMethod::Manual,
        );

        let resolved = reg.resolve_path("tokio", "src/runtime.rs").unwrap();
        assert_eq!(resolved, PathBuf::from("/libs/tokio/src/runtime.rs"));
    }

    #[test]
    fn resolve_path_errors_for_unknown_library() {
        let reg = LibraryRegistry::new();
        let result = reg.resolve_path("nonexistent", "src/lib.rs");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown library"));
    }

    #[test]
    fn persistence_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("libraries.json");

        let mut reg = LibraryRegistry::new();
        reg.register(
            "tokio".into(),
            PathBuf::from("/libs/tokio"),
            "rust".into(),
            DiscoveryMethod::LspFollowThrough,
        );
        reg.register(
            "serde".into(),
            PathBuf::from("/libs/serde"),
            "rust".into(),
            DiscoveryMethod::Manual,
        );
        reg.lookup_mut("serde").unwrap().indexed = true;
        reg.lookup_mut("serde").unwrap().version = Some("1.0.200".into());

        reg.save(&path).unwrap();
        let loaded = LibraryRegistry::load(&path).unwrap();

        assert_eq!(loaded.all().len(), 2);
        let tokio = loaded.lookup("tokio").unwrap();
        assert_eq!(tokio.discovered_via, DiscoveryMethod::LspFollowThrough);
        assert!(!tokio.indexed);

        let serde = loaded.lookup("serde").unwrap();
        assert!(serde.indexed);
        assert_eq!(serde.version, Some("1.0.200".into()));
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let reg = LibraryRegistry::load(Path::new("/nonexistent/path/libraries.json")).unwrap();
        assert!(reg.all().is_empty());
    }

    #[test]
    fn all_returns_all_entries() {
        let mut reg = LibraryRegistry::new();
        assert!(reg.all().is_empty());

        reg.register(
            "a".into(),
            PathBuf::from("/a"),
            "rust".into(),
            DiscoveryMethod::Manual,
        );
        reg.register(
            "b".into(),
            PathBuf::from("/b"),
            "python".into(),
            DiscoveryMethod::LspFollowThrough,
        );
        assert_eq!(reg.all().len(), 2);
    }
}
