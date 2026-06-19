//! Import path resolution.
//!
//! Resolves import nodes emitted by extractors into cross-file `imports` edges
//! from the importing file node to the imported file node.

use std::collections::{BTreeMap, BTreeSet};

use crate::extraction::file_node_id;
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind};

/// A prefix alias, e.g. TypeScript `@app/* -> src/*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportAlias {
    /// Import prefix without the trailing wildcard, e.g. `"@app/"`.
    pub prefix: String,
    /// Target path prefix without the trailing wildcard, e.g. `"src/"`.
    pub target_prefix: String,
}

impl ImportAlias {
    /// Build an alias pair. A trailing `*` is accepted and stripped on both
    /// sides so callers can pass tsconfig-style patterns directly.
    pub fn new(prefix: impl Into<String>, target_prefix: impl Into<String>) -> Self {
        Self {
            prefix: trim_star(prefix.into()),
            target_prefix: trim_star(target_prefix.into()),
        }
    }
}

/// Import resolution configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportResolverConfig {
    /// TypeScript / JavaScript path aliases.
    pub aliases: Vec<ImportAlias>,
    /// Rust crate root directories. Defaults to `["src"]`.
    pub rust_roots: Vec<String>,
    /// Go module path, e.g. `github.com/acme/app`.
    pub go_module: Option<String>,
}

impl ImportResolverConfig {
    fn rust_roots(&self) -> Vec<String> {
        if self.rust_roots.is_empty() {
            vec!["src".to_string()]
        } else {
            self.rust_roots.clone()
        }
    }
}

/// Summary of one resolution pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolveImportReport {
    /// Edges created from importer file to imported file.
    pub edges: Vec<Edge>,
    /// Import node ids that could not be mapped to a file.
    pub unresolved_import_node_ids: Vec<String>,
}

/// Import resolver over a fixed set of graph nodes.
#[derive(Debug, Clone)]
pub struct ImportResolver {
    config: ImportResolverConfig,
}

impl Default for ImportResolver {
    fn default() -> Self {
        Self::new(ImportResolverConfig::default())
    }
}

impl ImportResolver {
    /// Construct a resolver with explicit configuration.
    pub fn new(config: ImportResolverConfig) -> Self {
        Self { config }
    }

    /// Resolve every [`NodeKind::Import`] node in `nodes` into file-to-file
    /// imports edges. Existing extraction-time edges to import nodes are not
    /// inspected; this method derives the owning file from `import.file_path`.
    pub fn resolve_edges(&self, nodes: &[Node]) -> ResolveImportReport {
        let files = file_index(nodes);
        let import_nodes: Vec<&Node> = nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Import)
            .collect();

        let mut report = ResolveImportReport::default();
        let mut seen = BTreeSet::new();
        for import in import_nodes {
            let Some(target_path) = self.resolve_import(import, &files) else {
                report.unresolved_import_node_ids.push(import.id.clone());
                continue;
            };
            let source = file_node_id(&import.file_path);
            let target = file_node_id(&target_path);
            if source == target {
                continue;
            }
            let key = (source.clone(), target.clone(), import.id.clone());
            if !seen.insert(key) {
                continue;
            }
            report.edges.push(Edge {
                source,
                target,
                kind: EdgeKind::Imports,
                metadata: Some(serde_json::json!({
                    "import_node_id": import.id,
                    "import": import.name,
                })),
                line: Some(import.start_line),
                provenance: Some("import_resolver".to_string()),
            });
        }
        report
    }

    fn resolve_import(&self, import: &Node, files: &BTreeMap<String, Language>) -> Option<String> {
        let spec = clean_import_spec(&import.name, import.language);
        if spec.is_empty() {
            return None;
        }
        match import.language {
            Language::Rust => self.resolve_rust(&import.file_path, &spec, files),
            Language::TypeScript | Language::JavaScript => {
                self.resolve_js_ts(&import.file_path, &spec, files)
            }
            Language::Python => self.resolve_python(&import.file_path, &spec, files),
            Language::Go => self.resolve_go(&spec, files),
            _ => None,
        }
    }

    fn resolve_js_ts(
        &self,
        from_file: &str,
        spec: &str,
        files: &BTreeMap<String, Language>,
    ) -> Option<String> {
        if is_relative(spec) {
            return resolve_candidate(&join_import(from_file, spec), files, js_ts_suffixes());
        }
        for alias in &self.config.aliases {
            if let Some(rest) = spec.strip_prefix(&alias.prefix) {
                let mapped = format!("{}{}", alias.target_prefix, rest);
                if let Some(hit) = resolve_candidate(&mapped, files, js_ts_suffixes()) {
                    return Some(hit);
                }
            }
        }
        None
    }

    fn resolve_python(
        &self,
        from_file: &str,
        spec: &str,
        files: &BTreeMap<String, Language>,
    ) -> Option<String> {
        if is_relative(spec) {
            return resolve_candidate(&join_import(from_file, spec), files, python_suffixes());
        }
        let module = spec.split_whitespace().next().unwrap_or(spec);
        let path = module.replace('.', "/");
        resolve_candidate(&path, files, python_suffixes())
    }

    fn resolve_go(&self, spec: &str, files: &BTreeMap<String, Language>) -> Option<String> {
        let mut path = spec.to_string();
        if let Some(module) = &self.config.go_module {
            if let Some(rest) = spec.strip_prefix(module) {
                path = rest.trim_start_matches('/').to_string();
            }
        }
        resolve_candidate(&path, files, go_suffixes())
    }

    fn resolve_rust(
        &self,
        from_file: &str,
        spec: &str,
        files: &BTreeMap<String, Language>,
    ) -> Option<String> {
        let cleaned = spec.trim_start_matches("use ").trim_end_matches(';').trim();
        let first = cleaned
            .split(',')
            .next()
            .unwrap_or(cleaned)
            .trim()
            .trim_matches('{')
            .trim_matches('}');
        let parts: Vec<&str> = first.split("::").filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return None;
        }

        if parts[0] == "crate" {
            let module_parts = trim_terminal_symbol(&parts[1..]);
            for root in self.config.rust_roots() {
                if let Some(hit) = resolve_rust_module(&root, module_parts, files) {
                    return Some(hit);
                }
            }
            return None;
        }

        if parts[0] == "super" || parts[0] == "self" {
            let mut base = rust_module_dir(from_file);
            let mut start = 0;
            while start < parts.len() && parts[start] == "super" {
                base = parent_dir(&base);
                start += 1;
            }
            if start < parts.len() && parts[start] == "self" {
                start += 1;
            }
            let module_parts = trim_terminal_symbol(&parts[start..]);
            return resolve_rust_module(&base, module_parts, files);
        }

        for root in self.config.rust_roots() {
            let module_parts = trim_terminal_symbol(&parts);
            if let Some(hit) = resolve_rust_module(&root, module_parts, files) {
                return Some(hit);
            }
        }
        None
    }
}

fn file_index(nodes: &[Node]) -> BTreeMap<String, Language> {
    nodes
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .map(|n| (normalize_path(&n.file_path), n.language))
        .collect()
}

fn resolve_candidate(
    raw_base: &str,
    files: &BTreeMap<String, Language>,
    suffixes: &[&str],
) -> Option<String> {
    let base = normalize_path(raw_base);
    let mut candidates = Vec::new();
    candidates.push(base.clone());
    for suffix in suffixes {
        candidates.push(format!("{base}{suffix}"));
    }
    for index in ["index", "__init__", "mod"] {
        for suffix in suffixes {
            candidates.push(format!("{base}/{index}{suffix}"));
        }
    }
    if let Some(name) = base.rsplit('/').next() {
        for suffix in suffixes {
            candidates.push(format!("{base}/{name}{suffix}"));
        }
    }
    candidates.into_iter().find(|c| files.contains_key(c))
}

fn resolve_rust_module(
    root: &str,
    module_parts: &[&str],
    files: &BTreeMap<String, Language>,
) -> Option<String> {
    let mut base = normalize_path(root);
    if !module_parts.is_empty() {
        if !base.is_empty() {
            base.push('/');
        }
        base.push_str(&module_parts.join("/"));
    }
    let candidates = [
        format!("{base}.rs"),
        format!("{base}/mod.rs"),
        format!("{base}/lib.rs"),
    ];
    candidates.into_iter().find(|c| files.contains_key(c))
}

fn trim_terminal_symbol<'a>(parts: &'a [&'a str]) -> &'a [&'a str] {
    if parts.len() > 1 {
        &parts[..parts.len() - 1]
    } else {
        parts
    }
}

fn clean_import_spec(name: &str, language: Language) -> String {
    let name = name.trim();
    match language {
        Language::Python => name
            .strip_prefix("import ")
            .or_else(|| name.strip_prefix("from "))
            .unwrap_or(name)
            .split(" import ")
            .next()
            .unwrap_or(name)
            .trim()
            .to_string(),
        _ => name.to_string(),
    }
}

fn join_import(from_file: &str, spec: &str) -> String {
    let parent = parent_dir(from_file);
    let mut parts: Vec<String> = parent
        .split('/')
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect();
    for part in spec.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.to_string()),
        }
    }
    parts.join("/")
}

fn rust_module_dir(from_file: &str) -> String {
    let path = normalize_path(from_file);
    if path.ends_with("/mod.rs") {
        return path.trim_end_matches("/mod.rs").to_string();
    }
    parent_dir(&path)
}

fn parent_dir(path: &str) -> String {
    normalize_path(path)
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

fn normalize_path(path: &str) -> String {
    let mut stack = Vec::new();
    let normalized = path.replace('\\', "/");
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    stack.join("/")
}

fn is_relative(spec: &str) -> bool {
    spec == "." || spec == ".." || spec.starts_with("./") || spec.starts_with("../")
}

fn trim_star(mut s: String) -> String {
    if s.ends_with('*') {
        s.pop();
    }
    s
}

fn js_ts_suffixes() -> &'static [&'static str] {
    &[".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"]
}

fn python_suffixes() -> &'static [&'static str] {
    &[".py", ".pyi"]
}

fn go_suffixes() -> &'static [&'static str] {
    &[".go"]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, language: Language) -> Node {
        Node {
            id: file_node_id(path),
            kind: NodeKind::File,
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            qualified_name: path.to_string(),
            file_path: path.to_string(),
            language,
            start_line: 1,
            end_line: 1,
            start_column: 0,
            end_column: 0,
            signature: None,
            docstring: None,
            visibility: None,
            is_exported: false,
            is_async: false,
        }
    }

    fn import(path: &str, name: &str, language: Language, line: u32) -> Node {
        Node {
            id: format!("import:{path}:{name}:{line}"),
            kind: NodeKind::Import,
            name: name.to_string(),
            qualified_name: format!("import {name}"),
            file_path: path.to_string(),
            language,
            start_line: line,
            end_line: line,
            start_column: 0,
            end_column: 0,
            signature: None,
            docstring: None,
            visibility: None,
            is_exported: false,
            is_async: false,
        }
    }

    #[test]
    fn resolves_relative_js_import_to_file_node() {
        let nodes = vec![
            file("src/app/main.ts", Language::TypeScript),
            file("src/app/api.ts", Language::TypeScript),
            import("src/app/main.ts", "./api", Language::TypeScript, 3),
        ];

        let report = ImportResolver::default().resolve_edges(&nodes);

        assert_eq!(report.edges.len(), 1);
        assert_eq!(report.edges[0].source, "file:src/app/main.ts");
        assert_eq!(report.edges[0].target, "file:src/app/api.ts");
        assert_eq!(report.edges[0].kind, EdgeKind::Imports);
        assert!(report.unresolved_import_node_ids.is_empty());
    }

    #[test]
    fn resolves_js_alias_import() {
        let nodes = vec![
            file("src/app/main.ts", Language::TypeScript),
            file("src/lib/api.ts", Language::TypeScript),
            import("src/app/main.ts", "@lib/api", Language::TypeScript, 1),
        ];
        let resolver = ImportResolver::new(ImportResolverConfig {
            aliases: vec![ImportAlias::new("@lib/*", "src/lib/*")],
            ..Default::default()
        });

        let report = resolver.resolve_edges(&nodes);

        assert_eq!(report.edges[0].target, "file:src/lib/api.ts");
    }

    #[test]
    fn resolves_rust_crate_and_super_imports() {
        let nodes = vec![
            file("src/lib.rs", Language::Rust),
            file("src/foo.rs", Language::Rust),
            file("src/nested/mod.rs", Language::Rust),
            import("src/lib.rs", "crate::foo::Thing", Language::Rust, 2),
            import("src/nested/mod.rs", "super::foo::Thing", Language::Rust, 3),
        ];

        let report = ImportResolver::default().resolve_edges(&nodes);
        let targets: BTreeSet<_> = report.edges.iter().map(|e| e.target.as_str()).collect();

        assert_eq!(targets, BTreeSet::from(["file:src/foo.rs"]));
        assert_eq!(report.edges.len(), 2);
    }

    #[test]
    fn resolves_python_dotted_import() {
        let nodes = vec![
            file("pkg/app.py", Language::Python),
            file("pkg/base.py", Language::Python),
            import("pkg/app.py", "pkg.base import Base", Language::Python, 1),
        ];

        let report = ImportResolver::default().resolve_edges(&nodes);

        assert_eq!(report.edges[0].target, "file:pkg/base.py");
    }

    #[test]
    fn resolves_go_module_import() {
        let nodes = vec![
            file("cmd/app/main.go", Language::Go),
            file("internal/service/service.go", Language::Go),
            import(
                "cmd/app/main.go",
                "github.com/acme/app/internal/service",
                Language::Go,
                4,
            ),
        ];
        let resolver = ImportResolver::new(ImportResolverConfig {
            go_module: Some("github.com/acme/app".into()),
            ..Default::default()
        });

        let report = resolver.resolve_edges(&nodes);

        assert_eq!(report.edges[0].target, "file:internal/service/service.go");
    }

    #[test]
    fn reports_unresolved_imports_without_error() {
        let nodes = vec![
            file("src/main.ts", Language::TypeScript),
            import("src/main.ts", "missing-package", Language::TypeScript, 1),
        ];

        let report = ImportResolver::default().resolve_edges(&nodes);

        assert!(report.edges.is_empty());
        assert_eq!(
            report.unresolved_import_node_ids,
            vec!["import:src/main.ts:missing-package:1".to_string()]
        );
    }
}
