use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use deepagent_core::error::Result;
use deepagent_models::ToolSchema;
use deepagent_tools::{PermissionSet, ToolRegistry};

pub(crate) type DiscoveredToolSet = Arc<Mutex<HashSet<String>>>;

#[derive(Debug, Clone)]
pub(crate) struct ToolManifest {
    pub(crate) tools: Vec<ToolSchema>,
    pub(crate) deferred_tool_names: Vec<String>,
    pub(crate) undiscovered_deferred_names: Vec<String>,
}

pub(crate) fn prepare_tool_manifest(
    registry: &mut ToolRegistry,
    mode: deepagent_builtins::ToolSearchMode,
    discovered: DiscoveredToolSet,
    auto_threshold_chars: usize,
) -> Result<ToolManifest> {
    let deferred_tool_names =
        register_tool_search_into(registry, mode, discovered.clone(), auto_threshold_chars)?;
    let tools =
        build_visible_tool_schemas(registry, &PermissionSet::developer(), mode, &discovered);
    let undiscovered_deferred_names =
        undiscovered_deferred_tools(&deferred_tool_names, &discovered);
    Ok(ToolManifest {
        tools,
        deferred_tool_names,
        undiscovered_deferred_names,
    })
}

/// Decide whether to actually activate tool-search for this registry.
/// `Disabled` is rejected upstream; `Enabled` is always active; `Auto` is
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
    registry.register(Arc::new(deepagent_builtins::ToolSearchTool::new(
        deferred, discovered,
    )))?;
    Ok(names)
}

/// Render the dynamic-section "available deferred tools" block.
pub(crate) fn deferred_tools_announcement(undiscovered: &[String]) -> Option<String> {
    if undiscovered.is_empty() {
        return None;
    }
    let mut out =
        String::with_capacity(256 + undiscovered.iter().map(|n| n.len() + 4).sum::<usize>());
    out.push_str(
        "## Lazy-loaded tools

The tools below are NOT yet loaded in this session — only their names are visible. To call one, first invoke `tool_search` to fetch its full schema:
- `select:Name1,Name2` — load these specific names.
- `slack send` — keyword search; returns the best matches by name + description.
- `+slack send` — `+`-prefixed terms are required (must appear in name or description).

Once the matching schema lands, the tool becomes callable on the next turn.

<available-deferred-tools>
",
    );
    for name in undiscovered {
        out.push_str("- ");
        out.push_str(name);
        out.push('\n');
    }
    out.push_str("</available-deferred-tools>");
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

fn undiscovered_deferred_tools(
    deferred_tool_names: &[String],
    discovered: &DiscoveredToolSet,
) -> Vec<String> {
    let set = discovered.lock().unwrap_or_else(|p| p.into_inner());
    let mut out: Vec<String> = deferred_tool_names
        .iter()
        .filter(|name| !set.contains(name.as_str()))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}
