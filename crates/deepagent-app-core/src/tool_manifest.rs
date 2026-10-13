use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex};

use deepagent_core::error::Result;
use deepagent_models::ToolSchema;
use deepagent_tools::{PermissionSet, ToolRegistry};

pub(crate) type DiscoveredToolSet = Arc<Mutex<HashSet<String>>>;

pub(crate) fn live_tool_schema_source(
    registry: ToolRegistry,
    granted: PermissionSet,
    mode: deepagent_builtins::ToolSearchMode,
    discovered: DiscoveredToolSet,
    threshold_chars: usize,
) -> Arc<dyn Fn() -> Vec<ToolSchema> + Send + Sync> {
    let mode = effective_tool_search_mode(&registry, mode, threshold_chars);
    Arc::new(move || build_visible_tool_schemas(&registry, &granted, mode, &discovered))
}

#[derive(Debug, Clone)]
pub(crate) struct ToolManifest {
    pub(crate) tools: Vec<ToolSchema>,
    pub(crate) deferred_tool_names: Vec<String>,
    pub(crate) undiscovered_deferred_sources: Vec<String>,
}

pub(crate) fn prepare_tool_manifest(
    registry: &mut ToolRegistry,
    mode: deepagent_builtins::ToolSearchMode,
    discovered: DiscoveredToolSet,
    auto_threshold_chars: usize,
) -> Result<ToolManifest> {
    let mode = effective_tool_search_mode(registry, mode, auto_threshold_chars);
    let deferred_tool_names =
        register_tool_search_into(registry, mode, discovered.clone(), auto_threshold_chars)?;
    let tools =
        build_visible_tool_schemas(registry, &PermissionSet::developer(), mode, &discovered);
    let undiscovered_deferred_sources =
        undiscovered_deferred_sources(registry, &deferred_tool_names, &discovered);
    Ok(ToolManifest {
        tools,
        deferred_tool_names,
        undiscovered_deferred_sources,
    })
}

/// Resolve Auto once, before both discovery registration and schema filtering.
pub(crate) fn effective_tool_search_mode(
    registry: &ToolRegistry,
    mode: deepagent_builtins::ToolSearchMode,
    threshold_chars: usize,
) -> deepagent_builtins::ToolSearchMode {
    if should_activate_tool_search(registry, mode, threshold_chars) {
        deepagent_builtins::ToolSearchMode::Enabled
    } else {
        deepagent_builtins::ToolSearchMode::Disabled
    }
}

/// Decide whether to actually activate tool-search for this registry.
/// `Disabled` is inactive; `Enabled` is always active; `Auto` is
/// active only when the deferred-tool schema size meets `threshold_chars`.
pub(crate) fn should_activate_tool_search(
    registry: &ToolRegistry,
    mode: deepagent_builtins::ToolSearchMode,
    threshold_chars: usize,
) -> bool {
    match mode {
        deepagent_builtins::ToolSearchMode::Disabled => false,
        deepagent_builtins::ToolSearchMode::Enabled => true,
        deepagent_builtins::ToolSearchMode::Auto => {
            let total: usize = registry
                .iter_specs()
                .filter(|spec| deepagent_builtins::is_deferred_tool(spec.tool.as_ref(), mode))
                .map(|spec| {
                    spec.descriptor.name.len()
                        + spec.descriptor.description.len()
                        + spec.descriptor.parameters.to_string().len()
                })
                .sum();
            total >= threshold_chars
        }
    }
}

/// Register the `tool_search` built-in into `registry` if `mode` activates
/// (subject to the threshold for `Auto`). Returns the names of every deferred
/// tool the snapshot captured (or empty when no tools are eligible).
pub(crate) fn register_tool_search_into(
    registry: &mut ToolRegistry,
    mode: deepagent_builtins::ToolSearchMode,
    discovered: DiscoveredToolSet,
    auto_threshold_chars: usize,
) -> Result<Vec<String>> {
    if !mode.is_active() || !should_activate_tool_search(registry, mode, auto_threshold_chars) {
        return Ok(Vec::new());
    }
    let deferred: Vec<deepagent_builtins::DeferredToolSnapshot> = registry
        .iter_specs()
        .filter(|spec| deepagent_builtins::is_deferred_tool(spec.tool.as_ref(), mode))
        .map(|spec| deepagent_builtins::DeferredToolSnapshot {
            name: spec.descriptor.name.clone(),
            description: spec.descriptor.description.clone(),
        })
        .collect();
    if deferred.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<String> = deferred.iter().map(|s| s.name.clone()).collect();
    // Child registries inherit the parent's tool objects. Rebind the reserved
    // discovery channel to this run's own set instead of retaining its parent.
    registry.replace(Arc::new(deepagent_builtins::ToolSearchTool::new(
        deferred, discovered,
    )));
    Ok(names)
}

/// Advertise sources, not the full deferred tool catalog.
pub(crate) fn deferred_tools_announcement(sources: &[String]) -> Option<String> {
    if sources.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(512 + sources.iter().map(|n| n.len() + 4).sum::<usize>());
    out.push_str(
        "## Lazy-loaded tools

Additional tools are available from the sources below. Individual tool names, descriptions and parameter schemas are kept in the host catalog until needed. Use `tool_search` to find tools by task keywords and source name; do not guess tool names or arguments.
- `select:Name1,Name2` — load exact names already known from search results or instructions.
- `slack send` — keyword search; returns the best matches by name + description.
- `+slack send` — `+`-prefixed terms are required (must appear in name or description).

Only matching tools become callable on the next model request in this task. An explicit plugin mention is optional; search whenever a source can help with the task.

<available-tool-sources>
",
    );
    for name in sources {
        out.push_str("- ");
        out.push_str(name);
        out.push('\n');
    }
    out.push_str("</available-tool-sources>");
    Some(out)
}

/// Build the per-turn `tools` array sent to the model.
pub(crate) fn build_visible_tool_schemas(
    registry: &ToolRegistry,
    granted: &PermissionSet,
    mode: deepagent_builtins::ToolSearchMode,
    discovered: &DiscoveredToolSet,
) -> Vec<ToolSchema> {
    let active = mode.is_active();
    let descriptors = registry.visible_to(granted);
    if !active {
        return descriptors
            .into_iter()
            .map(|d| ToolSchema::function(d.name, d.description, d.parameters))
            .collect();
    }
    let discovered_snapshot: HashSet<String> = discovered
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .cloned()
        .collect();
    descriptors
        .into_iter()
        .filter(|d| {
            let Some(spec) = registry.get(&d.name) else {
                return true;
            };
            if !deepagent_builtins::is_deferred_tool(spec.tool.as_ref(), mode) {
                return true;
            }
            discovered_snapshot.contains(&d.name)
        })
        .map(|d| ToolSchema::function(d.name, d.description, d.parameters))
        .collect()
}

fn undiscovered_deferred_sources(
    registry: &ToolRegistry,
    deferred_tool_names: &[String],
    discovered: &DiscoveredToolSet,
) -> Vec<String> {
    let set = discovered.lock().unwrap_or_else(|p| p.into_inner());
    deferred_tool_names
        .iter()
        .filter(|name| !set.contains(name.as_str()))
        .filter_map(|name| registry.get(name))
        .map(|spec| {
            spec.tool
                .discovery_source()
                .unwrap_or("Other deferred tools")
                .to_string()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_builtins::ToolSearchMode;
    use deepagent_tools::{RiskLevel, Tool, ToolDescriptor, ToolOutput};

    struct CatalogTool {
        name: String,
        source: String,
    }

    #[async_trait::async_trait]
    impl Tool for CatalogTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: self.name.clone(),
                description: "Inspect a managed artifact for evidence".into(),
                parameters: serde_json::json!({"type": "object", "properties": {
                    "target_path": {"type": "string"}
                }, "required": ["target_path"]}),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::read_only(),
            }
        }

        async fn invoke(&self, _: serde_json::Value) -> Result<ToolOutput> {
            Ok(ToolOutput::success(serde_json::json!({})))
        }

        fn should_defer(&self) -> bool {
            true
        }

        fn discovery_source(&self) -> Option<&str> {
            Some(&self.source)
        }
    }

    fn catalog(count: usize) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        for index in 0..count {
            registry
                .register(Arc::new(CatalogTool {
                    name: format!("mcp__plugin__rea__rea__inspect_{index}"),
                    source: "plugin__rea__rea".into(),
                }))
                .unwrap();
        }
        registry
    }

    #[test]
    fn auto_below_threshold_keeps_tools_reachable_without_search() {
        let mut registry = catalog(1);
        let manifest = prepare_tool_manifest(
            &mut registry,
            ToolSearchMode::Auto,
            Arc::default(),
            usize::MAX,
        )
        .unwrap();
        assert_eq!(manifest.tools.len(), 1);
        assert!(manifest.deferred_tool_names.is_empty());
        assert!(manifest.undiscovered_deferred_sources.is_empty());
        assert!(registry.get("tool_search").is_none());
    }

    #[tokio::test]
    async fn large_plugin_advertises_one_source_and_loads_only_matches_in_same_run() {
        let mut registry = catalog(170);
        let discovered: DiscoveredToolSet = Arc::default();
        let manifest = prepare_tool_manifest(
            &mut registry,
            ToolSearchMode::Auto,
            discovered.clone(),
            8_000,
        )
        .unwrap();
        assert_eq!(manifest.tools.len(), 1);
        assert_eq!(manifest.tools[0].function.name, "tool_search");
        assert_eq!(manifest.undiscovered_deferred_sources, ["plugin__rea__rea"]);
        let block = deferred_tools_announcement(&manifest.undiscovered_deferred_sources).unwrap();
        assert!(!block.contains("inspect_"));
        assert!(!block.contains("target_path"));
        let small_manifest = prepare_tool_manifest(
            &mut catalog(1),
            ToolSearchMode::Enabled,
            Arc::default(),
            8_000,
        )
        .unwrap();
        assert_eq!(
            block,
            deferred_tools_announcement(&small_manifest.undiscovered_deferred_sources,).unwrap()
        );

        let source = live_tool_schema_source(
            registry.clone(),
            PermissionSet::developer(),
            ToolSearchMode::Auto,
            discovered.clone(),
            8_000,
        );
        assert_eq!(source().len(), 1);
        let result = registry
            .get("tool_search")
            .unwrap()
            .tool
            .invoke(serde_json::json!({"query": "+rea managed artifact", "max_results": 1}))
            .await
            .unwrap();
        assert!(result.ok);
        let loaded = result.value["matches"][0]["name"].as_str().unwrap();
        let tools = source();
        assert_eq!(tools.len(), 2);
        let schema = tools
            .iter()
            .find(|tool| tool.function.name == loaded)
            .unwrap();
        assert_eq!(schema.function.parameters["required"][0], "target_path");
        let denied = live_tool_schema_source(
            registry.clone(),
            PermissionSet::empty(),
            ToolSearchMode::Auto,
            discovered,
            8_000,
        );
        assert!(denied().is_empty());
        // A different session never inherits this search's discoveries.
        let other = live_tool_schema_source(
            registry,
            PermissionSet::developer(),
            ToolSearchMode::Auto,
            Arc::default(),
            8_000,
        );
        assert_eq!(other().len(), 1);
    }

    #[tokio::test]
    async fn cloned_registry_rebinds_search_to_child_discoveries() {
        let mut parent = catalog(1);
        let parent_discovered: DiscoveredToolSet = Arc::default();
        register_tool_search_into(
            &mut parent,
            ToolSearchMode::Enabled,
            parent_discovered.clone(),
            8_000,
        )
        .unwrap();
        let mut child = parent.clone();
        let child_discovered: DiscoveredToolSet = Arc::default();
        register_tool_search_into(
            &mut child,
            ToolSearchMode::Enabled,
            child_discovered.clone(),
            8_000,
        )
        .unwrap();
        child
            .get("tool_search")
            .unwrap()
            .tool
            .invoke(serde_json::json!({"query": "+rea", "max_results": 1}))
            .await
            .unwrap();
        assert_eq!(child_discovered.lock().unwrap().len(), 1);
        assert!(parent_discovered.lock().unwrap().is_empty());
    }
}
