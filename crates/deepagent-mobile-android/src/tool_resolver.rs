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

    /// Resolve a tool by name. Checks the cache first, then searches PATH,
    /// ANDROID_HOME, ANDROID_SDK_ROOT, and well-known SDK installation paths.
    pub fn resolve(&self, name: &str) -> Option<ResolvedTool> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(tool) = cache.get(name) {
            return Some(tool.clone());
        }
        let path = Self::find_tool(name)?;
        let version = None;
        let tool = ResolvedTool {
            name: name.to_string(),
            path,
            version,
        };
        cache.insert(name.to_string(), tool.clone());
        Some(tool)
    }

    fn find_tool(name: &str) -> Option<PathBuf> {
        if let Some(p) = Self::find_in_path(name) {
            return Some(p);
        }
        let subdir = Self::sdk_subdir(name);
        if let Some(env) = std::env::var_os("ANDROID_HOME") {
            let candidate = PathBuf::from(&env).join(subdir).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
            if cfg!(windows) {
                let exe = candidate.with_extension("exe");
                if exe.is_file() {
                    return Some(exe);
                }
            }
        }
        if let Some(env) = std::env::var_os("ANDROID_SDK_ROOT") {
            let candidate = PathBuf::from(&env).join(subdir).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
            if cfg!(windows) {
                let exe = candidate.with_extension("exe");
                if exe.is_file() {
                    return Some(exe);
                }
            }
        }
        for dir in Self::well_known_sdk_dirs() {
            let candidate = dir.join(subdir).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
            if cfg!(windows) {
                let exe = candidate.with_extension("exe");
                if exe.is_file() {
                    return Some(exe);
                }
            }
        }
        None
    }

    fn sdk_subdir(tool_name: &str) -> &'static str {
        match tool_name {
            "emulator" => "emulator",
            _ => "platform-tools",
        }
    }

    fn well_known_sdk_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if cfg!(windows) {
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                dirs.push(PathBuf::from(local).join("Android").join("Sdk"));
            }
            if let Some(home) = std::env::var_os("USERPROFILE") {
                let home = PathBuf::from(&home);
                dirs.push(home.join("Android").join("Sdk"));
                dirs.push(home.clone());
            }
        } else if cfg!(target_os = "macos") {
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(
                    PathBuf::from(&home)
                        .join("Library")
                        .join("Android")
                        .join("sdk"),
                );
                dirs.push(PathBuf::from(&home));
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(&home);
            dirs.push(home.join("Android").join("Sdk"));
            if !dirs.iter().any(|d| d == &home) {
                dirs.push(home);
            }
        }
        dirs
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
        let fake_name = "adb-test-nonexistent-tool";
        assert!(resolver.resolve(fake_name).is_none());

        resolver.insert(ResolvedTool {
            name: fake_name.into(),
            path: PathBuf::from("/usr/bin/adb"),
            version: Some("1.0.41".into()),
        });

        let resolved = resolver.resolve(fake_name).unwrap();
        assert_eq!(resolved.name, fake_name);
        assert_eq!(resolved.version.as_deref(), Some("1.0.41"));
    }

    #[test]
    fn resolver_clear() {
        let resolver = ToolResolver::new();
        let name = "nonexistent-tool-xyz-clear-test";
        resolver.insert(ResolvedTool {
            name: name.into(),
            path: PathBuf::from("/usr/bin/fake"),
            version: None,
        });
        resolver.clear();
        assert!(resolver.resolve(name).is_none());
    }

    #[test]
    fn sdk_subdir_maps_emulator_and_adb() {
        assert_eq!(ToolResolver::sdk_subdir("emulator"), "emulator");
        assert_eq!(ToolResolver::sdk_subdir("adb"), "platform-tools");
        assert_eq!(ToolResolver::sdk_subdir("fastboot"), "platform-tools");
    }

    #[test]
    fn well_known_dirs_are_existing_paths() {
        let dirs = ToolResolver::well_known_sdk_dirs();
        assert!(!dirs.is_empty(), "should have at least one well-known dir");
        for dir in &dirs {
            let is_sdk = dir.ends_with("Android/Sdk") || dir.ends_with("Android/sdk");
            let is_home = std::env::var_os("USERPROFILE")
                .map(|h| dir == &PathBuf::from(h))
                .unwrap_or(false)
                || std::env::var_os("HOME")
                    .map(|h| dir == &PathBuf::from(h))
                    .unwrap_or(false);
            assert!(
                is_sdk || is_home,
                "dir should be an SDK root or home: {:?}",
                dir
            );
        }
    }
}
