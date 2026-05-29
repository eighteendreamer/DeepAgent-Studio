//! The [`WorkspaceSnapshot`] data model.
//!
//! A snapshot is a point-in-time, serializable summary of a project that the
//! context pipeline (Layer 4) injects so the agent "understands the whole
//! workspace" (开发提示词.md §4 Layer 4). It is deliberately compact —
//! summaries and truncated trees, not full file contents — so it fits a token
//! budget.

use serde::{Deserialize, Serialize};

/// Detected project ecosystem(s). A repo can be polyglot, so this is a set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKind {
    /// Has a `Cargo.toml`.
    Rust,
    /// Has a `package.json`.
    Node,
    /// Has a `pyproject.toml` / `requirements.txt` / `setup.py`.
    Python,
    /// Has a `go.mod`.
    Go,
    /// Has a `pom.xml` / `build.gradle`.
    Java,
}

impl ProjectKind {
    /// Human label.
    pub const fn label(&self) -> &'static str {
        match self {
            ProjectKind::Rust => "Rust",
            ProjectKind::Node => "Node",
            ProjectKind::Python => "Python",
            ProjectKind::Go => "Go",
            ProjectKind::Java => "Java",
        }
    }
}

/// A discovered manifest file and an extracted summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestInfo {
    /// Path relative to the workspace root (forward-slashed).
    pub path: String,
    /// A short, human-readable summary (e.g. name + a few dependencies).
    pub summary: String,
}

/// A point-in-time snapshot of the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// Absolute root path (as a string for portability/serialization).
    pub root: String,
    /// Detected ecosystems, sorted and de-duplicated.
    pub kinds: Vec<ProjectKind>,
    /// A truncated, indented file tree.
    pub tree: String,
    /// Discovered manifests with summaries.
    pub manifests: Vec<ManifestInfo>,
    /// First chunk of the README, if present.
    pub readme_excerpt: Option<String>,
    /// Total files scanned (before truncation).
    pub file_count: usize,
    /// Total directories scanned.
    pub dir_count: usize,
    /// True if the tree was truncated due to limits.
    pub truncated: bool,
}

impl WorkspaceSnapshot {
    /// Render the snapshot as a compact prompt-ready block. This is what Layer 4
    /// of the context pipeline injects.
    pub fn to_context_block(&self) -> String {
        let mut out = String::new();
        out.push_str("# Workspace\n");
        if !self.kinds.is_empty() {
            let kinds: Vec<&str> = self.kinds.iter().map(|k| k.label()).collect();
            out.push_str(&format!("Project type: {}\n", kinds.join(", ")));
        }
        out.push_str(&format!(
            "Files: {} | Dirs: {}{}\n",
            self.file_count,
            self.dir_count,
            if self.truncated { " (truncated)" } else { "" }
        ));

        if let Some(readme) = &self.readme_excerpt {
            out.push_str("\n## README (excerpt)\n");
            out.push_str(readme);
            out.push('\n');
        }

        if !self.manifests.is_empty() {
            out.push_str("\n## Manifests\n");
            for m in &self.manifests {
                out.push_str(&format!("- {}: {}\n", m.path, m.summary));
            }
        }

        out.push_str("\n## File tree\n");
        out.push_str(&self.tree);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_block_includes_sections() {
        let snap = WorkspaceSnapshot {
            root: "/proj".into(),
            kinds: vec![ProjectKind::Rust],
            tree: "src/\n  main.rs".into(),
            manifests: vec![ManifestInfo {
                path: "Cargo.toml".into(),
                summary: "name = demo".into(),
            }],
            readme_excerpt: Some("A demo project.".into()),
            file_count: 2,
            dir_count: 1,
            truncated: false,
        };
        let block = snap.to_context_block();
        assert!(block.contains("Project type: Rust"));
        assert!(block.contains("README (excerpt)"));
        assert!(block.contains("Cargo.toml: name = demo"));
        assert!(block.contains("File tree"));
    }

    #[test]
    fn snapshot_roundtrips_json() {
        let snap = WorkspaceSnapshot {
            root: "/p".into(),
            kinds: vec![ProjectKind::Node, ProjectKind::Python],
            tree: "x".into(),
            manifests: vec![],
            readme_excerpt: None,
            file_count: 0,
            dir_count: 0,
            truncated: true,
        };
        let json = serde_json::to_string(&snap).unwrap();
        let back: WorkspaceSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, back);
    }
}
