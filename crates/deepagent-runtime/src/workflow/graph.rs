//! Versioned workflow graph contracts, independent of execution capabilities.
//!
//! Output declarations mirror desktop `nodeRegistry.ts` / `nodeSchemas.ts`;
//! source ports mirror `WorkflowNodeShell.tsx`. Compilation never executes nodes
//! or rewrites their configuration. Container scopes require a future contract.

use std::collections::{BTreeMap, BTreeSet};

use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

const MAX_NODES: usize = 256;
const MAX_EDGES: usize = 2048;
const MAX_DEFINITION_BYTES: usize = 1024 * 1024;
const MAX_ID_BYTES: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowDefinition {
    pub version: u32,
    pub nodes: Vec<WorkflowNodeSpec>,
    pub edges: Vec<WorkflowEdgeSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNodeSpec {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub config: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEdgeSpec {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub source_handle: Option<String>,
    #[serde(default)]
    pub target_handle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRequest {
    pub definition: WorkflowDefinition,
    #[serde(default)]
    pub inputs: Map<String, Value>,
    #[serde(default)]
    pub target_node_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledWorkflow {
    pub definition: WorkflowDefinition,
    /// Node indices in topological order, breaking readiness ties by input order.
    pub order: Vec<usize>,
    /// Lowercase SHA-256 hex of the serialized typed definition, without rewriting it.
    pub revision: String,
    /// Strict transitive ancestors, including an empty set for every root.
    pub ancestors: BTreeMap<String, BTreeSet<String>>,
}

/// Validate a version-1 DAG. A recognized kind is not a promise of execution support.
pub fn compile(definition: WorkflowDefinition) -> Result<CompiledWorkflow> {
    if definition.version != 1 {
        return Err(CoreError::invalid("workflow version must be 1"));
    }
    if definition.nodes.is_empty() || definition.nodes.len() > MAX_NODES {
        return Err(CoreError::invalid("workflow must contain 1..=256 nodes"));
    }
    if definition.edges.len() > MAX_EDGES {
        return Err(CoreError::invalid(
            "workflow must contain at most 2048 edges",
        ));
    }
    let serialized = serde_json::to_vec(&definition)?;
    if serialized.len() > MAX_DEFINITION_BYTES {
        return Err(CoreError::invalid("workflow definition exceeds 1 MiB"));
    }

    let mut indices = BTreeMap::new();
    let mut outputs = Vec::with_capacity(definition.nodes.len());
    let mut handles = Vec::with_capacity(definition.nodes.len());
    let mut has_start = false;
    for (index, node) in definition.nodes.iter().enumerate() {
        validate_id(&node.id, "node ID")?;
        if indices.insert(node.id.as_str(), index).is_some() {
            return Err(CoreError::invalid(format!(
                "duplicate node ID: {}",
                node.id
            )));
        }
        if node.kind == "start" {
            if has_start {
                return Err(CoreError::invalid(
                    "workflow permits at most one start node",
                ));
            }
            has_start = true;
        }
        outputs.push(output_names(node)?);
        handles.push(source_handles(node)?);
        validate_bindings(node)?;
    }

    let mut successors = vec![Vec::new(); definition.nodes.len()];
    let mut indegrees = vec![0usize; definition.nodes.len()];
    let mut edge_ids = BTreeSet::new();
    for edge in &definition.edges {
        validate_id(&edge.id, "edge ID")?;
        if !edge_ids.insert(&edge.id) {
            return Err(CoreError::invalid(format!(
                "duplicate edge ID: {}",
                edge.id
            )));
        }
        let source = *indices.get(edge.source.as_str()).ok_or_else(|| {
            CoreError::invalid(format!("edge {} has an unknown source node", edge.id))
        })?;
        let target = *indices.get(edge.target.as_str()).ok_or_else(|| {
            CoreError::invalid(format!("edge {} has an unknown target node", edge.id))
        })?;
        if source == target {
            return Err(CoreError::invalid(format!(
                "edge {} is a self-loop",
                edge.id
            )));
        }
        if definition.nodes[target].kind == "start" {
            return Err(CoreError::invalid("start nodes cannot have incoming edges"));
        }
        if definition.nodes[source].kind == "end" {
            return Err(CoreError::invalid("end nodes cannot have outgoing edges"));
        }
        if !matches!(edge.target_handle.as_deref(), None | Some("")) {
            return Err(CoreError::invalid(format!(
                "edge {} must use the unnamed target handle",
                edge.id
            )));
        }
        // Only the regular default port may be implicit. Even a sole named port
        // (classifier output / human timeout) must not silently select a branch.
        let source_handles = &handles[source];
        let valid_handle = match edge.source_handle.as_deref() {
            None | Some("") => source_handles.len() == 1 && source_handles.contains("default"),
            Some(handle) => source_handles.contains(handle),
        };
        if !valid_handle {
            return Err(CoreError::invalid(format!(
                "edge {} requires a valid source handle for node {}",
                edge.id, edge.source
            )));
        }
        successors[source].push(target);
        indegrees[target] += 1;
    }

    let mut ready: BTreeSet<usize> = indegrees
        .iter()
        .enumerate()
        .filter_map(|(index, &degree)| (degree == 0).then_some(index))
        .collect();
    let mut order = Vec::with_capacity(definition.nodes.len());
    let mut ancestor_sets = vec![BTreeSet::new(); definition.nodes.len()];
    while let Some(&index) = ready.iter().next() {
        ready.remove(&index);
        order.push(index);
        let mut inherited = ancestor_sets[index].clone();
        inherited.insert(definition.nodes[index].id.clone());
        for &target in &successors[index] {
            ancestor_sets[target].extend(inherited.iter().cloned());
            indegrees[target] -= 1;
            if indegrees[target] == 0 {
                ready.insert(target);
            }
        }
    }
    if order.len() != definition.nodes.len() {
        return Err(CoreError::invalid("workflow contains a cycle"));
    }

    for (index, node) in definition.nodes.iter().enumerate() {
        // Filter only node-level metadata: keys such as `description` inside an
        // HTTP body or tool arguments are still executable bindings.
        let filtered_config = node
            .config
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "description" | "label" | "codeScript"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let references = super::values::references(&Value::Object(filtered_config))?;
        for reference in references {
            let [source_id, output, ..] = reference.as_slice() else {
                return Err(CoreError::invalid(format!(
                    "node {} has an incomplete variable reference",
                    node.id
                )));
            };
            let source = *indices.get(source_id.as_str()).ok_or_else(|| {
                CoreError::invalid(format!(
                    "node {} references an unknown source node",
                    node.id
                ))
            })?;
            if !ancestor_sets[index].contains(source_id) {
                return Err(CoreError::invalid(format!(
                    "node {} references non-ancestor node {}",
                    node.id, source_id
                )));
            }
            // Nested paths are resolved at runtime; only the declared root output
            // is statically knowable (e.g. start.customer.address.city).
            if !outputs[source].contains(output) {
                return Err(CoreError::invalid(format!(
                    "node {} references an undeclared output of node {}",
                    node.id, source_id
                )));
            }
        }
    }

    let ancestors = definition
        .nodes
        .iter()
        .map(|node| node.id.clone())
        .zip(ancestor_sets)
        .collect();
    Ok(CompiledWorkflow {
        definition,
        order,
        revision: format!("{:x}", Sha256::digest(&serialized)),
        ancestors,
    })
}

/// Declared top-level outputs, with dynamic declarations validated rather than deduplicated.
pub fn output_names(node: &WorkflowNodeSpec) -> Result<BTreeSet<String>> {
    let (builtins, declaration_field) = output_contract(&node.kind)?;
    let mut names = string_set(builtins);
    if let Some(field) = declaration_field {
        for (index, declaration) in config_array(node, field)?.iter().enumerate() {
            let location = format!("node {} {field}[{index}]", node.id);
            let name = declaration_name(declaration, &location)?;
            validate_type(declaration.get("type"), &location)?;
            if !names.insert(name.to_owned()) {
                return Err(CoreError::invalid(format!(
                    "{location} has a duplicate or reserved output name"
                )));
            }
        }
    }
    if node.kind == "variable-aggregator" {
        if let Some(output_type) = node.config.get("aggregatorOutputType") {
            validate_type(Some(output_type), "aggregatorOutputType")?;
        }
    }
    Ok(names)
}

/// UI source-port IDs. `end` retains the UI's default port, but may not emit edges.
pub fn source_handles(node: &WorkflowNodeSpec) -> Result<BTreeSet<String>> {
    output_contract(&node.kind)?;
    match node.kind.as_str() {
        "if-else" => {
            let conditions = config_array(node, "conditions")?;
            let mut groups = BTreeSet::new();
            for group in conditions {
                let id = group.get("id").and_then(Value::as_str).ok_or_else(|| {
                    CoreError::invalid(format!("node {} condition group requires an ID", node.id))
                })?;
                validate_id(id, "condition group ID")?;
                if !groups.insert(id.to_owned()) {
                    return Err(CoreError::invalid(format!(
                        "node {} has duplicate condition group IDs",
                        node.id
                    )));
                }
            }
            if conditions.len() <= 1 {
                Ok(string_set(&["true", "false"]))
            } else {
                Ok(groups)
            }
        }
        "human-input" => {
            let fields = config_array(node, "humanInputFields")?;
            let mut handles: BTreeSet<_> = (0..fields.len())
                .map(|index| format!("action-{index}"))
                .collect();
            handles.insert("timeout".to_owned());
            Ok(handles)
        }
        "question-classifier" => {
            let classes = config_array(node, "classifierClasses")?;
            if classes.is_empty() {
                Ok(string_set(&["output"]))
            } else {
                Ok((0..classes.len())
                    .map(|index| format!("class-{index}"))
                    .collect())
            }
        }
        "iteration" => Ok(string_set(&["item", "done"])),
        _ => Ok(string_set(&["default"])),
    }
}

// This is a declaration registry, not an executor/capability registry. Keep the
// accepted kinds and their output contracts together so unknown kinds fail in
// both public introspection functions as well as in compile().
fn output_contract(kind: &str) -> Result<(&'static [&'static str], Option<&'static str>)> {
    Ok(match kind {
        "start" => (&[], Some("inputVariables")),
        "end" => (&[], Some("outputVariables")),
        "code" => (&[], Some("codeOutputVariables")),
        "parameter-extractor" => (
            &["__is_success", "__reason", "__usage"],
            Some("extractorParams"),
        ),
        "agent-v2" => (&["text", "usage"], Some("agentV2Outputs")),
        "human-input" => (&["action"], Some("humanInputFields")),
        "if-else" | "loop-end" => (&[], None),
        "iteration"
        | "loop"
        | "template-transform"
        | "variable-aggregator"
        | "variable-assigner" => (&["output"], None),
        "llm" | "agent" => (&["text", "usage"], None),
        "question-classifier" => (&["class_name", "class_label", "usage"], None),
        "knowledge-retrieval" => (&["documents", "content"], None),
        "http-request" => (&["body", "status_code", "headers"], None),
        "tool" => (&["text", "json"], None),
        "answer" => (&["answer"], None),
        "iteration-start" => (&["item"], None),
        "loop-start" => (&["context"], None),
        "document-extractor" => (&["text"], None),
        "list-operator" => (&["result", "first", "last"], None),
        "trigger-schedule" => (&["trigger_time", "context"], None),
        "trigger-webhook" => (&["body", "headers", "query"], None),
        "trigger-plugin" => (&["event_data"], None),
        "datasource" => (&["data", "files"], None),
        "knowledge-index" => (&["index_id", "chunk_count"], None),
        // Creative-mode canvas nodes. They share this registry, compile(), the
        // reference resolver and the event stream with professional nodes, so
        // both canvas modes execute through one chain instead of two.
        // Every creative kind that runs on a text model answers in `text`,
        // matching what the shared LLM executor emits.
        "category-picker" | "text-gen" | "script-gen" | "director" | "creative-template"
        | "storyboard-grid" | "character-face" | "character-body" | "character-style" => {
            (&["text"], None)
        }
        "image-input" | "image-gen" | "image-edit" => (&["imageUrl"], None),
        "image-compare" => (&["summary", "differences"], None),
        "video-gen" => (&["videoUrl"], None),
        "video-stitch" => (&["videoUrl"], None),
        "camera" => (&["cameraSettings"], None),
        "lens" => (&["lensSettings"], None),
        "focal-length" => (&["focalLengthConstraints"], None),
        "aperture" => (&["apertureConstraints"], None),

        "audio" => (&["text", "audioUrl"], None),
        _ => {
            return Err(CoreError::invalid(
                "unknown workflow node kind (neither professional nor creative)",
            ))
        }
    })
}

fn string_set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn config_array<'a>(node: &'a WorkflowNodeSpec, key: &str) -> Result<&'a [Value]> {
    match node.config.get(key) {
        None => Ok(&[]),
        Some(Value::Array(entries)) => Ok(entries),
        Some(_) => Err(CoreError::invalid(format!(
            "node {} {key} must be an array",
            node.id
        ))),
    }
}

fn validate_id(id: &str, location: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > MAX_ID_BYTES
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(CoreError::invalid(format!(
            "{location} must contain 1..=100 ASCII letters, digits, underscores or hyphens"
        )));
    }
    Ok(())
}

fn declaration_name<'a>(declaration: &'a Value, location: &str) -> Result<&'a str> {
    let name = declaration
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::invalid(format!("{location} requires a string name")))?;
    let mut bytes = name.bytes();
    let valid = bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if !valid || matches!(name, "__proto__" | "prototype" | "constructor") {
        return Err(CoreError::invalid(format!(
            "{location} requires a non-reserved ASCII identifier"
        )));
    }
    Ok(name)
}

fn validate_type(value: Option<&Value>, location: &str) -> Result<()> {
    if !matches!(
        value.and_then(Value::as_str),
        Some("string" | "number" | "boolean" | "object" | "array" | "file")
    ) {
        return Err(CoreError::invalid(format!(
            "{location} type must be string, number, boolean, object, array or file"
        )));
    }
    Ok(())
}

fn validate_bindings(node: &WorkflowNodeSpec) -> Result<()> {
    let field = match node.kind.as_str() {
        "code" => "codeInputVariables",
        "template-transform" => "templateInputVariables",
        "answer" => "answerVariables",
        _ => return Ok(()),
    };
    let mut names = BTreeSet::new();
    for (index, binding) in config_array(node, field)?.iter().enumerate() {
        let location = format!("node {} {field}[{index}]", node.id);
        let name = declaration_name(binding, &location)?;
        if !names.insert(name) {
            return Err(CoreError::invalid(format!(
                "{location} has a duplicate binding name"
            )));
        }
        if binding.get("value").is_none() {
            return Err(CoreError::invalid(format!(
                "{location} requires a binding value"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(id: &str, kind: &str, config: Value) -> WorkflowNodeSpec {
        WorkflowNodeSpec {
            id: id.into(),
            kind: kind.into(),
            config: config.as_object().unwrap().clone(),
        }
    }

    fn edge(id: &str, source: &str, target: &str) -> WorkflowEdgeSpec {
        WorkflowEdgeSpec {
            id: id.into(),
            source: source.into(),
            target: target.into(),
            source_handle: None,
            target_handle: None,
        }
    }

    fn graph(nodes: Vec<WorkflowNodeSpec>, edges: Vec<WorkflowEdgeSpec>) -> WorkflowDefinition {
        WorkflowDefinition {
            version: 1,
            nodes,
            edges,
        }
    }

    fn valid_graph() -> WorkflowDefinition {
        graph(
            vec![
                node(
                    "start",
                    "start",
                    json!({
                        "inputVariables": [{"name": "customer", "type": "object"}]
                    }),
                ),
                node(
                    "template",
                    "template-transform",
                    json!({
                        "templateInputVariables": [
                            {"name": "name", "value": "{{#start.customer.profile.name#}}"}
                        ],
                        "templateScript": "Hello {{ name }} / {{#start.customer.profile.name#}}"
                    }),
                ),
                node(
                    "end",
                    "end",
                    json!({
                        "outputVariables": [
                            {"name": "greeting", "type": "string", "value": "{{#template.output#}}"}
                        ]
                    }),
                ),
            ],
            vec![
                edge("e1", "start", "template"),
                edge("e2", "template", "end"),
            ],
        )
    }

    fn assert_invalid(definition: WorkflowDefinition, message: &str) {
        let error = compile(definition).unwrap_err();
        assert!(matches!(error, CoreError::Invalid(_)), "{error}");
        assert!(error.to_string().contains(message), "{error}");
    }

    #[test]
    fn compiles_start_template_end_without_mutation() {
        let definition = valid_graph();
        let before = serde_json::to_vec(&definition).unwrap();
        let compiled = compile(definition.clone()).unwrap();
        assert_eq!(compiled.definition, definition);
        assert_eq!(before, serde_json::to_vec(&definition).unwrap());
        assert_eq!(before, serde_json::to_vec(&compiled.definition).unwrap());
        assert_eq!(compiled.order, vec![0, 1, 2]);
        assert_eq!(compiled.ancestors["start"], string_set(&[]));
        assert_eq!(compiled.ancestors["template"], string_set(&["start"]));
        assert_eq!(
            compiled.ancestors["end"],
            string_set(&["start", "template"])
        );
        assert_eq!(
            output_names(&definition.nodes[0]).unwrap(),
            string_set(&["customer"])
        );
        assert_eq!(
            output_names(&definition.nodes[2]).unwrap(),
            string_set(&["greeting"])
        );
    }

    #[test]
    fn typed_boundaries_reject_unknown_fields_and_default_optional_fields() {
        let mut value = serde_json::to_value(valid_graph()).unwrap();
        value["nodes"][0]["parentId"] = json!("container");
        assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
        let mut value = serde_json::to_value(valid_graph()).unwrap();
        value["edges"][0]["sourceHandle"] = json!("default");
        assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
        let mut value = serde_json::to_value(valid_graph()).unwrap();
        value["containers"] = json!([]);
        assert!(serde_json::from_value::<WorkflowDefinition>(value).is_err());
        let value = json!({"definition": valid_graph(), "unknown": true});
        assert!(serde_json::from_value::<WorkflowRequest>(value).is_err());

        let request: WorkflowRequest = serde_json::from_value(json!({
            "definition": {"version": 1, "nodes": [{"id": "a", "kind": "end"}], "edges": []}
        }))
        .unwrap();
        assert!(request.inputs.is_empty());
        assert!(request.target_node_id.is_none());
        assert!(request.definition.nodes[0].config.is_empty());
        let parsed: WorkflowEdgeSpec = serde_json::from_value(json!({
            "id": "e", "source": "a", "target": "b"
        }))
        .unwrap();
        assert_eq!(parsed, edge("e", "a", "b"));
        assert_eq!(
            serde_json::from_value::<WorkflowRequest>(serde_json::to_value(&request).unwrap())
                .unwrap(),
            request
        );
    }

    #[test]
    fn rejects_invalid_version_unknown_kind_and_duplicate_ids() {
        let mut definition = valid_graph();
        definition.version = 2;
        assert_invalid(definition, "version");
        let mut definition = valid_graph();
        definition.nodes[1].kind = "mystery-node".into();
        assert_invalid(definition, "unknown workflow node kind");
        let mut definition = valid_graph();
        definition.nodes[1].id = "start".into();
        assert_invalid(definition, "duplicate node ID");
        let mut definition = valid_graph();
        definition.edges[1].id = "e1".into();
        assert_invalid(definition, "duplicate edge ID");
    }

    #[test]
    fn creative_canvas_node_kinds_compile_through_the_same_registry() {
        // Both canvas modes must go through this registry; a creative kind that
        // is not declared here would fail compile() and force a second chain.
        for kind in [
            "category-picker",
            "text-gen",
            "image-input",
            "image-gen",
            "image-compare",
            "image-edit",
            "script-gen",
            "video-gen",
            "video-stitch",
            "camera",
            "lens",
            "focal-length",
            "aperture",
            "director",
            "creative-template",
            "character-face",
            "character-body",
            "character-style",
            "storyboard-grid",
            "audio",
        ] {
            let definition = WorkflowDefinition {
                version: 1,
                nodes: vec![WorkflowNodeSpec {
                    id: "gen-1".to_string(),
                    kind: kind.to_string(),
                    config: Map::new(),
                }],
                edges: vec![],
            };
            let compiled = compile(definition)
                .unwrap_or_else(|error| panic!("creative kind {kind} must be registered: {error}"));
            assert!(
                output_contract(kind).is_ok(),
                "creative kind {kind} has no output contract"
            );
            assert_eq!(compiled.order.len(), 1);
        }
    }

    #[test]
    fn rejects_invalid_ids_for_nodes_edges_and_condition_groups() {
        for id in [
            "".to_owned(),
            "a b".into(),
            "a.b".into(),
            "节点".into(),
            "x".repeat(101),
        ] {
            let mut definition = valid_graph();
            definition.nodes[0].id = id.clone();
            assert_invalid(definition, "node ID");
            let mut definition = valid_graph();
            definition.edges[0].id = id.clone();
            assert_invalid(definition, "edge ID");
            assert!(source_handles(&node(
                "branch",
                "if-else",
                json!({
                    "conditions": [{"id": id}, {"id": "else"}]
                })
            ))
            .is_err());
        }
        assert!(compile(graph(
            vec![node(&"x".repeat(100), "end", json!({}))],
            vec![]
        ))
        .is_ok());
    }

    #[test]
    fn rejects_cycles_self_loops_and_dangling_endpoints() {
        assert_invalid(
            graph(
                vec![node("a", "llm", json!({})), node("b", "llm", json!({}))],
                vec![edge("ab", "a", "b"), edge("ba", "b", "a")],
            ),
            "cycle",
        );
        let mut definition = valid_graph();
        definition.edges[0].target = "start".into();
        assert_invalid(definition, "self-loop");
        for missing_source in [true, false] {
            let mut definition = valid_graph();
            if missing_source {
                definition.edges[0].source = "missing".into();
            } else {
                definition.edges[0].target = "missing".into();
            }
            assert_invalid(definition, "unknown");
        }
    }

    #[test]
    fn enforces_start_and_end_boundaries_without_requiring_a_start() {
        let mut definition = valid_graph();
        definition.nodes.push(node("other", "start", json!({})));
        assert_invalid(definition, "at most one start");
        let mut definition = valid_graph();
        definition.edges.push(edge("back", "template", "start"));
        assert_invalid(definition, "start nodes cannot");
        let mut definition = valid_graph();
        definition.edges.push(edge("back", "end", "template"));
        assert_invalid(definition, "end nodes cannot");
        assert!(compile(graph(
            vec![node("trigger", "trigger-webhook", json!({}))],
            vec![]
        ))
        .is_ok());
    }

    #[test]
    fn source_handles_match_all_ui_branch_shapes() {
        let cases = [
            ("if-else", json!({}), vec!["false", "true"]),
            (
                "if-else",
                json!({"conditions": [{"id": "if"}]}),
                vec!["false", "true"],
            ),
            (
                "if-else",
                json!({"conditions": [{"id": "if"}, {"id": "else"}]}),
                vec!["else", "if"],
            ),
            ("human-input", json!({}), vec!["timeout"]),
            (
                "human-input",
                json!({"humanInputFields": [
                    {"name": "yes", "type": "boolean"}, {"name": "no", "type": "boolean"}
                ]}),
                vec!["action-0", "action-1", "timeout"],
            ),
            ("question-classifier", json!({}), vec!["output"]),
            (
                "question-classifier",
                json!({"classifierClasses": [{"name": "a"}, {"name": "b"}]}),
                vec!["class-0", "class-1"],
            ),
            ("iteration", json!({}), vec!["done", "item"]),
            ("loop", json!({}), vec!["default"]),
            ("end", json!({}), vec!["default"]),
        ];
        for (kind, config, expected) in cases {
            assert_eq!(
                source_handles(&node("n", kind, config)).unwrap(),
                string_set(&expected),
                "{kind}"
            );
        }
        assert!(source_handles(&node(
            "n",
            "if-else",
            json!({
                "conditions": [{"id": "same"}, {"id": "same"}]
            })
        ))
        .is_err());
        assert!(source_handles(&node("n", "if-else", json!({"conditions": [{}]}))).is_err());
        assert!(source_handles(&node(
            "n",
            "question-classifier",
            json!({"classifierClasses": null})
        ))
        .is_err());
    }

    #[test]
    fn named_ports_require_explicit_valid_handles_even_when_only_one_exists() {
        for (kind, config) in [
            ("if-else", json!({})),
            (
                "if-else",
                json!({"conditions": [{"id": "default"}, {"id": "else"}]}),
            ),
            (
                "human-input",
                json!({"humanInputFields": [{"name": "approved", "type": "boolean"}]}),
            ),
            ("human-input", json!({})),
            (
                "question-classifier",
                json!({"classifierClasses": [{"name": "one"}]}),
            ),
            ("question-classifier", json!({})),
            ("iteration", json!({})),
        ] {
            let definition = graph(
                vec![node("source", kind, config), node("sink", "end", json!({}))],
                vec![edge("e", "source", "sink")],
            );
            for handle in [None, Some(""), Some("unknown")] {
                let mut invalid = definition.clone();
                invalid.edges[0].source_handle = handle.map(str::to_owned);
                assert_invalid(invalid, "source handle");
            }
            for handle in source_handles(&definition.nodes[0]).unwrap() {
                let mut valid = definition.clone();
                valid.edges[0].source_handle = Some(handle);
                assert!(compile(valid).is_ok(), "{kind}");
            }
        }
    }

    #[test]
    fn regular_ports_accept_only_default_and_targets_are_unnamed() {
        for handle in [None, Some(""), Some("default")] {
            let mut definition = valid_graph();
            definition.edges[0].source_handle = handle.map(str::to_owned);
            definition.edges[0].target_handle = Some(String::new());
            assert!(compile(definition).is_ok());
        }
        let mut definition = valid_graph();
        definition.edges[0].source_handle = Some("true".into());
        assert_invalid(definition, "source handle");
        for handle in ["default", "input", " "] {
            let mut definition = valid_graph();
            definition.edges[0].target_handle = Some(handle.into());
            assert_invalid(definition, "unnamed target");
        }
    }

    #[test]
    fn rejects_missing_unconnected_self_and_future_references() {
        for (reference, message) in [
            ("{{#start.missing#}}", "undeclared output"),
            ("{{#missing.output#}}", "unknown source"),
            ("{{#template.output#}}", "non-ancestor"),
            ("{{#end.greeting#}}", "non-ancestor"),
        ] {
            let mut definition = valid_graph();
            definition.nodes[1]
                .config
                .insert("templateScript".into(), json!(reference));
            assert_invalid(definition, message);
        }
        let mut definition = valid_graph();
        definition.edges.remove(0);
        assert_invalid(definition, "non-ancestor");
    }

    #[test]
    fn scans_bindings_but_not_node_metadata_or_code_source() {
        let mut definition = valid_graph();
        for key in ["description", "label", "codeScript"] {
            definition.nodes[1]
                .config
                .insert(key.into(), json!("{{#missing.output#}}"));
        }
        assert!(compile(definition.clone()).is_ok());
        // Do not accidentally exclude nested runtime dictionary keys with metadata names.
        definition.nodes[1].config.insert(
            "toolParams".into(),
            json!({
                "description": "{{#missing.output#}}"
            }),
        );
        assert_invalid(definition, "unknown source");
        let mut definition = valid_graph();
        definition.nodes[1].config["templateInputVariables"][0]["value"] =
            json!("{{#start.absent#}}");
        assert_invalid(definition, "undeclared output");
    }

    #[test]
    fn dynamic_declarations_validate_names_types_and_reserved_collisions() {
        for (kind, field, builtins) in [
            ("start", "inputVariables", vec![]),
            ("end", "outputVariables", vec![]),
            ("code", "codeOutputVariables", vec![]),
            (
                "parameter-extractor",
                "extractorParams",
                vec!["__is_success", "__reason", "__usage"],
            ),
            ("agent-v2", "agentV2Outputs", vec!["text", "usage"]),
            ("human-input", "humanInputFields", vec!["action"]),
        ] {
            let mut spec = node("n", kind, json!({}));
            for data_type in ["string", "number", "boolean", "object", "array", "file"] {
                spec.config.insert(
                    field.into(),
                    json!([{"name": "value_1", "type": data_type}]),
                );
                let mut expected = string_set(&builtins);
                expected.insert("value_1".into());
                assert_eq!(output_names(&spec).unwrap(), expected);
            }
            for name in [
                "",
                "1value",
                "with-dash",
                "x.y",
                "变量",
                "__proto__",
                "prototype",
                "constructor",
            ] {
                spec.config
                    .insert(field.into(), json!([{"name": name, "type": "string"}]));
                assert!(output_names(&spec).is_err(), "{kind}: {name}");
            }
            for name in &builtins {
                spec.config
                    .insert(field.into(), json!([{"name": name, "type": "string"}]));
                assert!(output_names(&spec).is_err(), "{kind}: reserved {name}");
            }
            for declaration in [
                json!({"name": "value_1"}),
                json!({"name": "value_1", "type": "integer"}),
                json!(null),
            ] {
                spec.config.insert(field.into(), json!([declaration]));
                assert!(output_names(&spec).is_err());
            }
            spec.config.insert(
                field.into(),
                json!([
                    {"name": "duplicate", "type": "string"}, {"name": "duplicate", "type": "number"}
                ]),
            );
            assert!(output_names(&spec).is_err());
            spec.config.insert(field.into(), json!({}));
            assert!(output_names(&spec).is_err());
        }
        let definition = graph(
            vec![node(
                "n",
                "start",
                json!({
                    "inputVariables": [{"name": "constructor", "type": "string"}]
                }),
            )],
            vec![],
        );
        assert_invalid(definition, "non-reserved ASCII");
    }

    #[test]
    fn bindings_have_separate_namespaces_but_reject_duplicate_or_reserved_names() {
        for (kind, field) in [
            ("code", "codeInputVariables"),
            ("template-transform", "templateInputVariables"),
            ("answer", "answerVariables"),
        ] {
            for bindings in [
                json!([{"name": "constructor", "value": "x"}]),
                json!([{"name": "x", "value": "a"}, {"name": "x", "value": "b"}]),
                json!([{"name": "x"}]),
            ] {
                let mut spec = node("n", kind, json!({}));
                spec.config.insert(field.into(), bindings);
                assert!(compile(graph(vec![spec], vec![])).is_err());
            }
        }
        assert!(compile(graph(
            vec![node(
                "n",
                "code",
                json!({
                    "codeInputVariables": [{"name": "result", "value": 1}],
                    "codeOutputVariables": [{"name": "result", "type": "number"}]
                })
            )],
            vec![]
        ))
        .is_ok());
    }

    #[test]
    fn recognizes_exactly_the_professional_registry_output_contracts() {
        let cases: [(&str, &[&str]); 29] = [
            ("start", &[]),
            ("end", &[]),
            ("code", &[]),
            (
                "parameter-extractor",
                &["__is_success", "__reason", "__usage"],
            ),
            ("agent-v2", &["text", "usage"]),
            ("human-input", &["action"]),
            ("if-else", &[]),
            ("loop-end", &[]),
            ("iteration", &["output"]),
            ("loop", &["output"]),
            ("template-transform", &["output"]),
            ("variable-aggregator", &["output"]),
            ("variable-assigner", &["output"]),
            ("llm", &["text", "usage"]),
            ("agent", &["text", "usage"]),
            (
                "question-classifier",
                &["class_name", "class_label", "usage"],
            ),
            ("knowledge-retrieval", &["documents", "content"]),
            ("http-request", &["body", "status_code", "headers"]),
            ("tool", &["text", "json"]),
            ("answer", &["answer"]),
            ("iteration-start", &["item"]),
            ("loop-start", &["context"]),
            ("document-extractor", &["text"]),
            ("list-operator", &["result", "first", "last"]),
            ("trigger-schedule", &["trigger_time", "context"]),
            ("trigger-webhook", &["body", "headers", "query"]),
            ("trigger-plugin", &["event_data"]),
            ("datasource", &["data", "files"]),
            ("knowledge-index", &["index_id", "chunk_count"]),
        ];
        for (kind, expected) in cases {
            let spec = node("n", kind, json!({}));
            assert_eq!(output_names(&spec).unwrap(), string_set(expected), "{kind}");
            assert!(compile(graph(vec![spec], vec![])).is_ok(), "{kind}");
        }
        let unknown = node("n", "professional-llm", json!({}));
        assert!(output_names(&unknown).is_err());
        assert!(source_handles(&unknown).is_err());
    }

    #[test]
    fn stable_order_tracks_readiness_not_ids_or_edge_insertion_order() {
        let definition = graph(
            vec![
                node("z", "llm", json!({})),
                node("a", "llm", json!({})),
                node("root", "start", json!({})),
                node("sink", "end", json!({})),
            ],
            vec![
                edge("rz", "root", "z"),
                edge("zs", "z", "sink"),
                edge("as", "a", "sink"),
            ],
        );
        let compiled = compile(definition.clone()).unwrap();
        assert_eq!(compiled.order, vec![1, 2, 0, 3]);
        assert_eq!(compiled.ancestors["sink"], string_set(&["z", "a", "root"]));
        let mut reversed = definition;
        reversed.edges.reverse();
        let reversed = compile(reversed).unwrap();
        assert_eq!(compiled.order, reversed.order);
        assert_eq!(compiled.ancestors, reversed.ancestors);
        // Definition order is intentionally part of the revision, not normalized away.
        assert_ne!(compiled.revision, reversed.revision);
        let definition = graph(
            vec![
                node("dependent", "llm", json!({})),
                node("root", "llm", json!({})),
                node("later", "llm", json!({})),
            ],
            vec![edge("e", "root", "dependent")],
        );
        assert_eq!(compile(definition).unwrap().order, vec![1, 0, 2]);
    }

    #[test]
    fn revision_is_repeatable_sha256_of_the_typed_definition() {
        let definition = valid_graph();
        let compiled = compile(definition.clone()).unwrap();
        assert_eq!(compiled, compile(definition.clone()).unwrap());
        assert_eq!(compiled.revision.len(), 64);
        assert_eq!(
            compiled.revision,
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&definition).unwrap())
            )
        );
        let mut changed = definition;
        changed.nodes[1]
            .config
            .insert("templateScript".into(), json!("Changed {{ name }}"));
        assert_ne!(compiled.revision, compile(changed).unwrap().revision);
    }

    #[test]
    fn enforces_graph_and_serialized_size_limits_at_the_boundary() {
        assert_invalid(graph(vec![], vec![]), "1..=256");
        let nodes: Vec<_> = (0..MAX_NODES)
            .map(|index| node(&format!("n{index}"), "llm", json!({})))
            .collect();
        assert!(compile(graph(nodes.clone(), vec![])).is_ok());
        let mut oversized = nodes;
        oversized.push(node("extra", "llm", json!({})));
        assert_invalid(graph(oversized, vec![]), "1..=256");
        let mut definition = graph(
            vec![node("a", "llm", json!({})), node("b", "llm", json!({}))],
            (0..MAX_EDGES)
                .map(|index| edge(&format!("e{index}"), "a", "b"))
                .collect(),
        );
        assert!(compile(definition.clone()).is_ok());
        definition.edges.push(edge("extra", "a", "b"));
        assert_invalid(definition, "2048");
        let mut definition = graph(vec![node("n", "end", json!({"description": ""}))], vec![]);
        let overhead = serde_json::to_vec(&definition).unwrap().len();
        definition.nodes[0].config.insert(
            "description".into(),
            json!("x".repeat(MAX_DEFINITION_BYTES - overhead)),
        );
        assert_eq!(
            serde_json::to_vec(&definition).unwrap().len(),
            MAX_DEFINITION_BYTES
        );
        assert!(compile(definition.clone()).is_ok());
        definition.nodes[0].config.insert(
            "description".into(),
            json!("x".repeat(MAX_DEFINITION_BYTES - overhead + 1)),
        );
        assert_invalid(definition, "1 MiB");
    }
}
