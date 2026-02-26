use std::path::{Path, PathBuf};

pub struct DiscoveredLibrary {
    pub name: String,
    pub version: Option<String>,
    pub path: PathBuf,
    pub language: String,
}

/// Walk up from a file path to find a package manifest and extract metadata.
pub fn discover_library_root(file_path: &Path) -> Option<DiscoveredLibrary> {
    let mut dir = file_path.parent()?;

    loop {
        if let Some(result) = try_cargo_toml(dir) {
            return Some(result);
        }
        if let Some(result) = try_package_json(dir) {
            return Some(result);
        }
        if let Some(result) = try_pyproject_toml(dir) {
            return Some(result);
        }
        if let Some(result) = try_go_mod(dir) {
            return Some(result);
        }

        match dir.parent() {
            Some(parent) if parent != dir => dir = parent,
            _ => break,
        }
    }

    // Fallback: use the deepest directory name
    let fallback_dir = file_path.parent()?;
    Some(DiscoveredLibrary {
        name: fallback_dir.file_name()?.to_string_lossy().into_owned(),
        version: None,
        path: fallback_dir.to_path_buf(),
        language: "unknown".into(),
    })
}

fn try_cargo_toml(dir: &Path) -> Option<DiscoveredLibrary> {
    let manifest = dir.join("Cargo.toml");
    if !manifest.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&manifest).ok()?;

    let name = extract_toml_value(&text, "name")?;
    let version = extract_toml_value(&text, "version");

    Some(DiscoveredLibrary {
        name,
        version,
        path: dir.to_path_buf(),
        language: "rust".into(),
    })
}

fn try_package_json(dir: &Path) -> Option<DiscoveredLibrary> {
    let manifest = dir.join("package.json");
    if !manifest.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&manifest).ok()?;

    let name = extract_json_value(&text, "name")?;
    let version = extract_json_value(&text, "version");

    Some(DiscoveredLibrary {
        name,
        version,
        path: dir.to_path_buf(),
        language: "javascript".into(),
    })
}

fn try_pyproject_toml(dir: &Path) -> Option<DiscoveredLibrary> {
    let manifest = dir.join("pyproject.toml");
    if !manifest.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&manifest).ok()?;

    let name = extract_toml_value(&text, "name")?;
    let version = extract_toml_value(&text, "version");

    Some(DiscoveredLibrary {
        name,
        version,
        path: dir.to_path_buf(),
        language: "python".into(),
    })
}

fn try_go_mod(dir: &Path) -> Option<DiscoveredLibrary> {
    let manifest = dir.join("go.mod");
    if !manifest.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&manifest).ok()?;

    let name = text
        .lines()
        .find(|l| l.starts_with("module "))
        .map(|l| l.trim_start_matches("module ").trim().to_string())?;

    Some(DiscoveredLibrary {
        name,
        version: None,
        path: dir.to_path_buf(),
        language: "go".into(),
    })
}

/// Best-effort: extract `key = "value"` from TOML
fn extract_toml_value(text: &str, key: &str) -> Option<String> {
    let pattern = format!("{} = \"", key);
    text.lines()
        .find(|l| l.trim().starts_with(&pattern))
        .and_then(|l| {
            let start = l.find('"')? + 1;
            let end = l[start..].find('"')? + start;
            Some(l[start..end].to_string())
        })
}

/// Best-effort: extract `"key": "value"` from JSON
fn extract_json_value(text: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\"", key);
    text.lines().find(|l| l.contains(&pattern)).and_then(|l| {
        let after_key = l.find(&pattern)? + pattern.len();
        let rest = &l[after_key..];
        let start = rest.find('"')? + 1;
        let end = rest[start..].find('"')? + start;
        Some(rest[start..end].to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_from_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"serde\"\nversion = \"1.0.210\"\n",
        )
        .unwrap();

        let result = discover_library_root(&dir.path().join("src/de.rs")).unwrap();
        assert_eq!(result.name, "serde");
        assert_eq!(result.version, Some("1.0.210".into()));
        assert_eq!(result.path, dir.path().to_path_buf());
        assert_eq!(result.language, "rust");
    }

    #[test]
    fn discover_from_package_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{ "name": "lodash", "version": "4.17.21" }"#,
        )
        .unwrap();

        let result = discover_library_root(&dir.path().join("index.js")).unwrap();
        assert_eq!(result.name, "lodash");
        assert_eq!(result.version, Some("4.17.21".into()));
        assert_eq!(result.language, "javascript");
    }

    #[test]
    fn discover_from_pyproject_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"requests\"\nversion = \"2.31.0\"\n",
        )
        .unwrap();

        let result = discover_library_root(&dir.path().join("src/requests/api.py")).unwrap();
        assert_eq!(result.name, "requests");
        assert_eq!(result.version, Some("2.31.0".into()));
        assert_eq!(result.language, "python");
    }

    #[test]
    fn discover_from_go_mod() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("go.mod"),
            "module github.com/user/repo\n\ngo 1.21\n",
        )
        .unwrap();

        let result = discover_library_root(&dir.path().join("main.go")).unwrap();
        assert_eq!(result.name, "github.com/user/repo");
        assert_eq!(result.version, None);
        assert_eq!(result.language, "go");
    }

    #[test]
    fn discover_fallback_uses_dir_name() {
        let dir = tempfile::tempdir().unwrap();
        let result = discover_library_root(&dir.path().join("lib.rs"));
        assert!(result.is_some());
    }

    #[test]
    fn discover_walks_up_parents() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("src").join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"deep_crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let result = discover_library_root(&nested.join("mod.rs")).unwrap();
        assert_eq!(result.name, "deep_crate");
        assert_eq!(result.path, dir.path().to_path_buf());
    }

    #[test]
    fn extract_toml_value_basic() {
        let toml = "name = \"hello\"\nversion = \"1.0\"";
        assert_eq!(extract_toml_value(toml, "name"), Some("hello".into()));
        assert_eq!(extract_toml_value(toml, "version"), Some("1.0".into()));
        assert_eq!(extract_toml_value(toml, "missing"), None);
    }

    #[test]
    fn extract_json_value_basic() {
        let json = r#"{ "name": "lodash", "version": "4.17.21" }"#;
        assert_eq!(extract_json_value(json, "name"), Some("lodash".into()));
        assert_eq!(extract_json_value(json, "version"), Some("4.17.21".into()));
        assert_eq!(extract_json_value(json, "missing"), None);
    }
}
