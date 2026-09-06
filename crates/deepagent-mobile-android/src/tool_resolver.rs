use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Resolves and caches external tool paths (adb, emulator, etc.).
///
/// Before executing any command, the backend must verify that the tool exists,
/// is executable, and optionally record its version. Results are cached for the
/// lifetime of the resolver.
#[derive(Debug, Clone)]
pub struct ToolResolver {
    cache: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, ResolvedTool>>>,
}

/// A resolved tool with validated path and optional version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedTool {
    pub name: String,
    pub path: PathBuf,
    pub version: Option<String>,
}

impl ToolResolver {
    pub fn new() -> Self {
        Self {
            cache: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Resolve a tool by name. Checks the cache first, then searches PATH.
    pub fn resolve(&self, name: &str) -> Option<ResolvedTool> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(tool) = cache.get(name) {
            return Some(tool.clone());
        }
        let path = Self::find_in_path(name)?;
        let tool = ResolvedTool {
            name: name.to_string(),
            path,
            version: None,
        };
        cache.insert(name.to_string(), tool.clone());
        Some(tool)
    }

    fn find_in_path(name: &str) -> Option<PathBuf> {
        let path_var = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
            if cfg!(windows) {
                let exe = dir.join(format!("{name}.exe"));
                if exe.is_file() {
                    return Some(exe);
                }
                let bat = dir.join(format!("{name}.bat"));
                if bat.is_file() {
                    return Some(bat);
                }
            }
        }
        None
    }

    /// Insert a resolved tool into the cache (used by probe and tests).
    pub fn insert(&self, tool: ResolvedTool) {
        let mut cache = self.cache.lock().unwrap();
        cache.insert(tool.name.clone(), tool);
    }

    /// Clear the cache.
    pub fn clear(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.clear();
    }
}

impl Default for ToolResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_cache_round_trip() {
        let resolver = ToolResolver::new();
        assert!(resolver.resolve("adb").is_none());

        resolver.insert(ResolvedTool {
            name: "adb".into(),
            path: PathBuf::from("/usr/bin/adb"),
            version: Some("1.0.41".into()),
        });

        let resolved = resolver.resolve("adb").unwrap();
        assert_eq!(resolved.name, "adb");
        assert_eq!(resolved.version.as_deref(), Some("1.0.41"));
    }

    #[test]
    fn resolver_clear() {
        let resolver = ToolResolver::new();
        resolver.insert(ResolvedTool {
            name: "adb".into(),
            path: PathBuf::from("/usr/bin/adb"),
            version: None,
        });
        resolver.clear();
        assert!(resolver.resolve("adb").is_none());
    }
}
