//! Heuristic architectural layer classification.
//!
//! The projector uses these layers to give the UA project map a stable,
//! human-oriented overview before richer schema projection is added.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::types::Node;

/// Broad architectural buckets used by the project map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LayerKind {
    Api,
    Service,
    Data,
    Ui,
    Utility,
    Test,
    Configuration,
    Core,
}

impl LayerKind {
    /// Stable kebab-case id used in projected JSON.
    pub fn id(self) -> &'static str {
        match self {
            LayerKind::Api => "api",
            LayerKind::Service => "service",
            LayerKind::Data => "data",
            LayerKind::Ui => "ui",
            LayerKind::Utility => "utility",
            LayerKind::Test => "test",
            LayerKind::Configuration => "configuration",
            LayerKind::Core => "core",
        }
    }

    /// Display name for UI labels.
    pub fn name(self) -> &'static str {
        match self {
            LayerKind::Api => "API",
            LayerKind::Service => "Service",
            LayerKind::Data => "Data",
            LayerKind::Ui => "UI",
            LayerKind::Utility => "Utility",
            LayerKind::Test => "Test",
            LayerKind::Configuration => "Configuration",
            LayerKind::Core => "Core",
        }
    }

    /// Short explanation used by guided projection consumers.
    pub fn description(self) -> &'static str {
        match self {
            LayerKind::Api => "Entry points, routes, handlers, and controllers.",
            LayerKind::Service => "Business logic, use cases, and domain services.",
            LayerKind::Data => "Persistence, schemas, repositories, and models.",
            LayerKind::Ui => "User interface components, pages, and views.",
            LayerKind::Utility => "Shared helpers, common libraries, and utilities.",
            LayerKind::Test => "Tests, specs, fixtures, and validation code.",
            LayerKind::Configuration => "Configuration, manifests, and build setup.",
            LayerKind::Core => "Core project files that do not match a specific layer.",
        }
    }

    /// Deterministic tour and layer order.
    pub fn ordered() -> &'static [LayerKind] {
        &[
            LayerKind::Api,
            LayerKind::Service,
            LayerKind::Data,
            LayerKind::Ui,
            LayerKind::Utility,
            LayerKind::Test,
            LayerKind::Configuration,
            LayerKind::Core,
        ]
    }
}

/// Projected architectural layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub id: String,
    pub name: String,
    pub description: String,
    pub node_ids: Vec<String>,
}

/// Classify a path into a broad architectural layer.
pub fn classify_path(path: &str) -> LayerKind {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let file_name = normalized.rsplit('/').next().unwrap_or(&normalized);
    let stem = file_name.split('.').next().unwrap_or(file_name);
    let components: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();

    if is_test_path(&components, file_name) {
        return LayerKind::Test;
    }
    if is_config_path(&components, file_name) {
        return LayerKind::Configuration;
    }
    if has_component(
        &components,
        &[
            "api",
            "apis",
            "route",
            "routes",
            "controller",
            "controllers",
            "handler",
            "handlers",
            "endpoint",
            "endpoints",
        ],
    ) || contains_token(stem, &["controller", "handler", "route", "endpoint"])
    {
        return LayerKind::Api;
    }
    if has_component(
        &components,
        &[
            "service",
            "services",
            "usecase",
            "usecases",
            "use-case",
            "use-cases",
            "domain",
            "business",
        ],
    ) || contains_token(stem, &["service", "usecase", "use-case"])
    {
        return LayerKind::Service;
    }
    if has_component(
        &components,
        &[
            "db",
            "database",
            "repo",
            "repos",
            "repository",
            "repositories",
            "model",
            "models",
            "schema",
            "schemas",
            "migration",
            "migrations",
            "prisma",
            "dao",
        ],
    ) || contains_token(
        stem,
        &["repository", "repo", "model", "schema", "migration"],
    ) {
        return LayerKind::Data;
    }
    if has_component(
        &components,
        &[
            "ui",
            "component",
            "components",
            "view",
            "views",
            "page",
            "pages",
            "screen",
            "screens",
            "frontend",
            "client",
        ],
    ) || matches!(file_name, "app.tsx" | "app.jsx" | "app.vue" | "app.svelte")
    {
        return LayerKind::Ui;
    }
    if has_component(
        &components,
        &[
            "util", "utils", "helper", "helpers", "lib", "common", "shared",
        ],
    ) || contains_token(stem, &["util", "utils", "helper", "helpers"])
    {
        return LayerKind::Utility;
    }

    LayerKind::Core
}

/// Build non-empty layers for the supplied graph nodes.
///
/// File nodes are preferred for projection layers. If the caller supplies only
/// symbol nodes, the classifier still groups those symbols by their file path.
pub fn build_layers(nodes: &[Node]) -> Vec<Layer> {
    let mut grouped: BTreeMap<LayerKind, Vec<String>> = BTreeMap::new();
    for node in nodes {
        let kind = classify_path(&node.file_path);
        grouped.entry(kind).or_default().push(node.id.clone());
    }

    LayerKind::ordered()
        .iter()
        .filter_map(|kind| {
            let mut node_ids = grouped.remove(kind)?;
            node_ids.sort();
            node_ids.dedup();
            Some(Layer {
                id: kind.id().to_string(),
                name: kind.name().to_string(),
                description: kind.description().to_string(),
                node_ids,
            })
        })
        .collect()
}

fn is_test_path(components: &[&str], file_name: &str) -> bool {
    has_component(
        components,
        &[
            "test",
            "tests",
            "__tests__",
            "spec",
            "specs",
            "fixture",
            "fixtures",
        ],
    ) || file_name.contains(".test.")
        || file_name.contains(".spec.")
        || file_name.ends_with("_test.go")
        || file_name.ends_with("_test.rs")
        || file_name.starts_with("test_")
}

fn is_config_path(components: &[&str], file_name: &str) -> bool {
    has_component(
        components,
        &[
            ".github",
            ".vscode",
            "config",
            "configs",
            "configuration",
            "docker",
            "deploy",
            "deployment",
            "ci",
        ],
    ) || matches!(
        file_name,
        "cargo.toml"
            | "cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "pyproject.toml"
            | "poetry.lock"
            | "go.mod"
            | "go.sum"
            | "dockerfile"
            | "docker-compose.yml"
            | "docker-compose.yaml"
            | "makefile"
            | ".env"
            | ".env.example"
            | ".gitignore"
            | "tsconfig.json"
            | "vite.config.ts"
            | "webpack.config.js"
    ) || file_name.ends_with(".config.js")
        || file_name.ends_with(".config.ts")
        || file_name.ends_with(".config.cjs")
        || file_name.ends_with(".config.mjs")
        || file_name.ends_with(".yml")
        || file_name.ends_with(".yaml")
        || file_name.ends_with(".toml")
}

fn has_component(components: &[&str], needles: &[&str]) -> bool {
    components
        .iter()
        .any(|component| needles.contains(component))
}

fn contains_token(stem: &str, needles: &[&str]) -> bool {
    let tokens: Vec<&str> = stem
        .split(['-', '_', '.'])
        .filter(|token| !token.is_empty())
        .collect();
    needles
        .iter()
        .any(|needle| tokens.contains(needle) || stem.ends_with(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Language, Node, NodeKind};

    #[test]
    fn classifies_common_project_paths() {
        assert_eq!(classify_path("src/api/users.rs"), LayerKind::Api);
        assert_eq!(
            classify_path("src/services/payment_service.ts"),
            LayerKind::Service
        );
        assert_eq!(classify_path("src/db/user_repository.py"), LayerKind::Data);
        assert_eq!(classify_path("src/components/Button.tsx"), LayerKind::Ui);
        assert_eq!(classify_path("src/utils/path.ts"), LayerKind::Utility);
        assert_eq!(classify_path("tests/user_flow.spec.ts"), LayerKind::Test);
        assert_eq!(classify_path("Cargo.toml"), LayerKind::Configuration);
    }

    #[test]
    fn unknown_paths_fall_back_to_core() {
        assert_eq!(classify_path("src/main.rs"), LayerKind::Core);
        assert_eq!(classify_path("README.md"), LayerKind::Core);
    }

    #[test]
    fn layer_ids_are_kebab_case() {
        for kind in LayerKind::ordered() {
            let id = kind.id();
            assert!(!id.is_empty());
            assert!(id
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'));
        }
    }

    #[test]
    fn build_layers_groups_and_sorts_node_ids() {
        let nodes = vec![
            node("file:src/api/users.rs", "src/api/users.rs"),
            node("file:src/utils/path.rs", "src/utils/path.rs"),
            node("file:src/api/accounts.rs", "src/api/accounts.rs"),
        ];

        let layers = build_layers(&nodes);
        let api = layers.iter().find(|layer| layer.id == "api").unwrap();
        assert_eq!(
            api.node_ids,
            vec![
                "file:src/api/accounts.rs".to_string(),
                "file:src/api/users.rs".to_string()
            ]
        );
        assert!(layers.iter().any(|layer| layer.id == "utility"));
    }

    fn node(id: &str, file_path: &str) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::File,
            name: file_path.to_string(),
            qualified_name: file_path.to_string(),
            file_path: file_path.to_string(),
            language: Language::Rust,
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
}
