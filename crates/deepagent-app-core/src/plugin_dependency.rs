//! Dependency verification for loaded plugins.
//!
//! Plugin dependencies are a presence guarantee: if plugin A depends on B, B
//! must be effectively enabled before A contributes runtime capabilities.
//! Verification is intentionally load-local and does not rewrite user state.

use std::collections::{BTreeMap, BTreeSet};

use crate::plugin::model::DiagnosticSeverity;
use crate::plugin_loader::{plugin_id, LoadedPlugin, PluginLoadError};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginDependencyOutcome {
    pub demoted: BTreeSet<String>,
    pub errors_by_plugin: BTreeMap<String, Vec<PluginLoadError>>,
}

impl PluginDependencyOutcome {
    pub fn errors_for(&self, plugin_id: &str) -> &[PluginLoadError] {
        self.errors_by_plugin
            .get(plugin_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

pub fn verify_plugin_dependencies(
    plugins: &[LoadedPlugin],
    is_initially_enabled: impl Fn(&LoadedPlugin) -> bool,
) -> PluginDependencyOutcome {
    let mut outcome = PluginDependencyOutcome::default();
    let known = plugins
        .iter()
        .filter(|plugin| plugin.manifest.is_some())
        .map(|plugin| plugin.id.clone())
        .collect::<BTreeSet<_>>();
    let mut enabled = plugins
        .iter()
        .filter(|plugin| is_initially_enabled(plugin))
        .map(|plugin| plugin.id.clone())
        .collect::<BTreeSet<_>>();
    let by_id = plugins
        .iter()
        .map(|plugin| (plugin.id.clone(), plugin))
        .collect::<BTreeMap<_, _>>();

    loop {
        let mut changed = false;
        for plugin in plugins {
            if !enabled.contains(&plugin.id) {
                continue;
            }
            let Some(manifest) = plugin.manifest.as_ref() else {
                continue;
            };
            for raw_dep in &manifest.dependencies {
                let Some(dep) = qualify_dependency(raw_dep, plugin) else {
                    continue;
                };
                if enabled.contains(&dep) {
                    continue;
                }

                enabled.remove(&plugin.id);
                outcome.demoted.insert(plugin.id.clone());
                outcome
                    .errors_by_plugin
                    .entry(plugin.id.clone())
                    .or_default()
                    .push(dependency_error(
                        plugin,
                        &dep,
                        if known.contains(&dep) {
                            "not-enabled"
                        } else {
                            "not-found"
                        },
                    ));
                changed = true;
                break;
            }
        }

        if let Some(cycle) = find_enabled_cycle(&by_id, &enabled) {
            let message = format!("plugin dependency cycle detected: {}", cycle.join(" -> "));
            for id in cycle.iter().collect::<BTreeSet<_>>() {
                if enabled.remove(id.as_str()) {
                    outcome.demoted.insert(id.clone());
                    let Some(plugin) = by_id.get(id.as_str()) else {
                        continue;
                    };
                    outcome
                        .errors_by_plugin
                        .entry(id.clone())
                        .or_default()
                        .push(PluginLoadError {
                            kind: "dependency-cycle".to_string(),
                            plugin: Some(id.clone()),
                            source: plugin.source_key.clone(),
                            path: Some(plugin.root.display().to_string()),
                            component: Some("dependencies".to_string()),
                            message: message.clone(),
                            // The plugin is demoted out of the enabled set, so
                            // it is unusable rather than merely degraded.
                            severity: DiagnosticSeverity::Error,
                        });
                }
            }
            changed = true;
        }

        if !changed {
            break;
        }
    }

    outcome
}

pub fn qualify_dependency(raw: &str, declaring_plugin: &LoadedPlugin) -> Option<String> {
    let dep = raw.split("@^").next().unwrap_or(raw).trim();
    if dep.is_empty() {
        return None;
    }
    if dep.contains('@') {
        Some(dep.to_string())
    } else {
        Some(plugin_id(dep, &declaring_plugin.source_key))
    }
}

pub fn find_reverse_dependents(
    target_id: &str,
    plugins: &[LoadedPlugin],
    is_enabled: impl Fn(&LoadedPlugin) -> bool,
) -> Vec<String> {
    let mut reverse = BTreeMap::<String, BTreeSet<String>>::new();
    let enabled_plugins = plugins
        .iter()
        .filter(|plugin| plugin.id != target_id && is_enabled(plugin))
        .collect::<Vec<_>>();
    for plugin in enabled_plugins {
        let Some(manifest) = plugin.manifest.as_ref() else {
            continue;
        };
        for dependency in manifest
            .dependencies
            .iter()
            .filter_map(|raw| qualify_dependency(raw, plugin))
        {
            reverse
                .entry(dependency)
                .or_default()
                .insert(plugin.id.clone());
        }
    }

    let mut dependents = BTreeSet::<String>::new();
    let mut stack = vec![target_id.to_string()];
    while let Some(id) = stack.pop() {
        let Some(next) = reverse.get(&id) else {
            continue;
        };
        for dependent in next {
            if dependents.insert(dependent.clone()) {
                stack.push(dependent.clone());
            }
        }
    }
    dependents.into_iter().collect()
}

fn dependency_error(plugin: &LoadedPlugin, dependency: &str, reason: &str) -> PluginLoadError {
    PluginLoadError {
        kind: "dependency-unsatisfied".to_string(),
        plugin: Some(plugin.id.clone()),
        source: plugin.source_key.clone(),
        path: Some(plugin.root.display().to_string()),
        component: Some("dependencies".to_string()),
        message: format!(
            "dependency {dependency} is {reason}; plugin {} will not be injected into runtime",
            plugin.id
        ),
        // Nothing from this plugin reaches the runtime, so it is unusable.
        severity: DiagnosticSeverity::Error,
    }
}

fn find_enabled_cycle(
    by_id: &BTreeMap<String, &LoadedPlugin>,
    enabled: &BTreeSet<String>,
) -> Option<Vec<String>> {
    let mut visiting = BTreeSet::<String>::new();
    let mut visited = BTreeSet::<String>::new();
    let mut stack = Vec::<String>::new();

    for id in enabled {
        if visited.contains(id) {
            continue;
        }
        if let Some(cycle) = visit(id, by_id, enabled, &mut visiting, &mut visited, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

fn visit(
    id: &str,
    by_id: &BTreeMap<String, &LoadedPlugin>,
    enabled: &BTreeSet<String>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    if let Some(start) = stack.iter().position(|item| item == id) {
        let mut cycle = stack[start..].to_vec();
        cycle.push(id.to_string());
        return Some(cycle);
    }
    if visited.contains(id) || !enabled.contains(id) {
        return None;
    }

    visiting.insert(id.to_string());
    stack.push(id.to_string());

    let plugin = by_id.get(id)?;
    if let Some(manifest) = plugin.manifest.as_ref() {
        for raw_dep in &manifest.dependencies {
            let Some(dep) = qualify_dependency(raw_dep, plugin) else {
                continue;
            };
            if !enabled.contains(&dep) {
                continue;
            }
            if let Some(cycle) = visit(&dep, by_id, enabled, visiting, visited, stack) {
                return Some(cycle);
            }
        }
    }

    stack.pop();
    visiting.remove(id);
    visited.insert(id.to_string());
    None
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::plugin_loader::{LoadedPlugin, PluginOrigin};
    use crate::plugin_manifest::{PluginManifest, PluginManifestPaths};

    use super::*;

    fn plugin(id: &str, name: &str, source_key: &str, dependencies: Vec<&str>) -> LoadedPlugin {
        LoadedPlugin {
            id: id.to_string(),
            name: name.to_string(),
            source_key: source_key.to_string(),
            origin: PluginOrigin::Personal,
            marketplace: None,
            root: PathBuf::from(format!("/plugins/{name}")),
            manifest: Some(PluginManifest {
                name: name.to_string(),
                version: None,
                description: None,
                author: None,
                homepage: None,
                repository: None,
                license: None,
                keywords: Vec::new(),
                dependencies: dependencies.into_iter().map(str::to_string).collect(),
                paths: PluginManifestPaths::default(),
                interface: Default::default(),
                runtime: Default::default(),
                manifest_path: Path::new("/plugins").join(name).join("plugin.json"),
            }),
            available: true,
            overridden_by: None,
            errors: Vec::new(),
        }
    }

    #[test]
    fn demotes_plugin_with_missing_dependency() {
        let plugins = vec![plugin("a@personal", "a", "personal", vec!["b"])];

        let outcome = verify_plugin_dependencies(&plugins, |_| true);

        assert!(outcome.demoted.contains("a@personal"));
        assert_eq!(
            outcome.errors_for("a@personal")[0].kind,
            "dependency-unsatisfied"
        );
        assert!(outcome.errors_for("a@personal")[0]
            .message
            .contains("b@personal is not-found"));
    }

    #[test]
    fn demotes_dependents_after_dependency_is_disabled() {
        let plugins = vec![
            plugin("a@personal", "a", "personal", vec!["b"]),
            plugin("b@personal", "b", "personal", Vec::new()),
        ];

        let outcome = verify_plugin_dependencies(&plugins, |plugin| plugin.id != "b@personal");

        assert!(outcome.demoted.contains("a@personal"));
        assert!(outcome.errors_for("a@personal")[0]
            .message
            .contains("b@personal is not-enabled"));
    }

    #[test]
    fn demotes_dependency_cycles() {
        let plugins = vec![
            plugin("a@personal", "a", "personal", vec!["b"]),
            plugin("b@personal", "b", "personal", vec!["a"]),
        ];

        let outcome = verify_plugin_dependencies(&plugins, |_| true);

        assert!(outcome.demoted.contains("a@personal"));
        assert!(outcome.demoted.contains("b@personal"));
        assert_eq!(outcome.errors_for("a@personal")[0].kind, "dependency-cycle");
        assert_eq!(outcome.errors_for("b@personal")[0].kind, "dependency-cycle");
    }

    #[test]
    fn finds_reverse_dependents_for_enabled_plugins() {
        let plugins = vec![
            plugin("worker@personal", "worker", "personal", vec!["helper"]),
            plugin("helper@personal", "helper", "personal", Vec::new()),
            plugin("disabled@personal", "disabled", "personal", vec!["helper"]),
        ];

        let dependents = find_reverse_dependents("helper@personal", &plugins, |plugin| {
            plugin.id != "disabled@personal"
        });

        assert_eq!(dependents, vec!["worker@personal"]);
    }

    #[test]
    fn finds_transitive_reverse_dependents_for_enabled_plugins() {
        let plugins = vec![
            plugin("app@personal", "app", "personal", vec!["worker"]),
            plugin("worker@personal", "worker", "personal", vec!["helper"]),
            plugin("helper@personal", "helper", "personal", Vec::new()),
        ];

        let dependents = find_reverse_dependents("helper@personal", &plugins, |_| true);

        assert_eq!(dependents, vec!["app@personal", "worker@personal"]);
    }
}
