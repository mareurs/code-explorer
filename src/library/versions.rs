use std::path::Path;

#[derive(Debug, Clone)]
pub struct ResolvedVersion {
    pub name: String,
    pub version: String,
}

/// Detect project type and parse lockfile to get dependency versions.
pub fn resolve_dependency_versions(project_root: &Path) -> Vec<ResolvedVersion> {
    // Try Cargo.lock (Rust) — P0
    let cargo_lock = project_root.join("Cargo.lock");
    if cargo_lock.exists() {
        if let Ok(content) = std::fs::read_to_string(&cargo_lock) {
            return parse_cargo_lock(&content);
        }
    }

    // Try package-lock.json (JS/TS) — P0
    let pkg_lock = project_root.join("package-lock.json");
    if pkg_lock.exists() {
        if let Ok(content) = std::fs::read_to_string(&pkg_lock) {
            return parse_package_lock_json(&content);
        }
    }

    // TODO P1: yarn.lock, pnpm-lock.yaml, go.sum, uv.lock, poetry.lock

    Vec::new()
}

/// Parse Cargo.lock to extract package versions.
pub fn parse_cargo_lock(content: &str) -> Vec<ResolvedVersion> {
    let mut versions = Vec::new();
    let mut current_name: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("name = ") {
            current_name = line
                .strip_prefix("name = ")
                .and_then(|s| s.strip_prefix('"'))
                .and_then(|s| s.strip_suffix('"'))
                .map(|s| s.to_string());
        } else if line.starts_with("version = ") {
            if let (Some(name), Some(ver)) = (
                current_name.take(),
                line.strip_prefix("version = ")
                    .and_then(|s| s.strip_prefix('"'))
                    .and_then(|s| s.strip_suffix('"')),
            ) {
                versions.push(ResolvedVersion {
                    name,
                    version: ver.to_string(),
                });
            }
        }
    }
    versions
}

/// Parse package-lock.json (v2/v3 format) to extract package versions.
pub fn parse_package_lock_json(content: &str) -> Vec<ResolvedVersion> {
    let mut versions = Vec::new();
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) else {
        return versions;
    };

    if let Some(packages) = parsed.get("packages").and_then(|p| p.as_object()) {
        for (key, val) in packages {
            if key.is_empty() {
                continue; // Skip root package
            }
            let name = key.strip_prefix("node_modules/").unwrap_or(key).to_string();
            if let Some(version) = val.get("version").and_then(|v| v.as_str()) {
                versions.push(ResolvedVersion {
                    name,
                    version: version.to_string(),
                });
            }
        }
    }
    versions
}

/// Look up a specific library's version from resolved dependencies.
pub fn find_version(versions: &[ResolvedVersion], name: &str) -> Option<String> {
    versions
        .iter()
        .find(|v| v.name == name)
        .map(|v| v.version.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cargo_lock_extracts_versions() {
        let content = r#"
[[package]]
name = "tokio"
version = "1.38.0"

[[package]]
name = "serde"
version = "1.0.203"
"#;
        let versions = parse_cargo_lock(content);
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].name, "tokio");
        assert_eq!(versions[0].version, "1.38.0");
        assert_eq!(versions[1].name, "serde");
        assert_eq!(versions[1].version, "1.0.203");
    }

    #[test]
    fn parse_package_lock_json_extracts_versions() {
        let content = r#"{
            "packages": {
                "": { "version": "1.0.0" },
                "node_modules/lodash": { "version": "4.17.21" },
                "node_modules/@types/node": { "version": "20.11.0" }
            }
        }"#;
        let versions = parse_package_lock_json(content);
        assert_eq!(versions.len(), 2); // root package skipped
        assert!(versions
            .iter()
            .any(|v| v.name == "lodash" && v.version == "4.17.21"));
        assert!(versions
            .iter()
            .any(|v| v.name == "@types/node" && v.version == "20.11.0"));
    }

    #[test]
    fn find_version_looks_up_by_name() {
        let versions = vec![
            ResolvedVersion {
                name: "tokio".into(),
                version: "1.38.0".into(),
            },
            ResolvedVersion {
                name: "serde".into(),
                version: "1.0.203".into(),
            },
        ];
        assert_eq!(find_version(&versions, "tokio"), Some("1.38.0".to_string()));
        assert_eq!(find_version(&versions, "unknown"), None);
    }

    #[test]
    fn resolve_dependency_versions_reads_cargo_lock() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.lock"),
            r#"
[[package]]
name = "anyhow"
version = "1.0.82"
"#,
        )
        .unwrap();
        let versions = resolve_dependency_versions(dir.path());
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].name, "anyhow");
    }
}
