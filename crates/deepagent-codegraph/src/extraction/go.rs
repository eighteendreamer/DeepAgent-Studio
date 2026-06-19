//! Go symbol extraction.

use std::collections::{BTreeSet, HashMap};

use tree_sitter::Node as TsNode;

use super::{file_node_id, make_node_id, ExtractedFile, ExtractorImpl};
use crate::scanner::ScannedFile;
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind, UnresolvedRef};

/// Extractor for Go source files.
#[derive(Debug, Default)]
pub struct GoExtractor;

#[derive(Debug, Clone)]
struct CallSite {
    from_node_id: String,
    reference_name: String,
    line: u32,
    file_path: String,
}

#[derive(Debug, Clone)]
struct MethodInfo {
    receiver_type: String,
    method_name: String,
}

#[derive(Debug, Default)]
struct GoState {
    file_path: String,
    file_id: String,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    calls: Vec<CallSite>,
    methods: Vec<MethodInfo>,
}

impl ExtractorImpl for GoExtractor {
    fn extract(&self, tree: &tree_sitter::Tree, source: &str, file: &ScannedFile) -> ExtractedFile {
        let mut out = ExtractedFile::file_only(file, source);
        let file_path = posix_path(file);
        let mut state = GoState {
            file_id: file_node_id(&file_path),
            file_path,
            nodes: Vec::new(),
            edges: Vec::new(),
            calls: Vec::new(),
            methods: Vec::new(),
        };

        visit_children(tree.root_node(), source, &mut state, None);
        add_interface_implements_edges(&mut state);
        resolve_calls(&mut state);

        out.nodes.extend(state.nodes);
        out.edges.extend(state.edges);
        out.unresolved_refs = state
            .calls
            .into_iter()
            .map(|call| UnresolvedRef {
                from_node_id: call.from_node_id,
                reference_name: call.reference_name,
                reference_kind: "call".into(),
                line: call.line,
                file_path: call.file_path,
            })
            .collect();
        out
    }
}

fn visit_children(
    node: TsNode<'_>,
    source: &str,
    state: &mut GoState,
    current_callable: Option<&str>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_node(child, source, state, current_callable);
    }
}

fn visit_node(node: TsNode<'_>, source: &str, state: &mut GoState, current_callable: Option<&str>) {
    match node.kind() {
        "function_declaration" => visit_function(node, source, state),
        "method_declaration" => visit_method(node, source, state),
        "type_declaration" => visit_type_declaration(node, source, state),
        "import_declaration" | "import_spec" => visit_import(node, source, state),
        "call_expression" => {
            if let Some(from_node_id) = current_callable {
                if let Some(name) = call_name(node, source) {
                    state.calls.push(CallSite {
                        from_node_id: from_node_id.to_string(),
                        reference_name: name,
                        line: line(node),
                        file_path: state.file_path.clone(),
                    });
                }
            }
            visit_children(node, source, state, current_callable);
        }
        _ => visit_children(node, source, state, current_callable),
    }
}

fn visit_function(node: TsNode<'_>, source: &str, state: &mut GoState) {
    let Some(name) = name_of(node, source) else {
        visit_children(node, source, state, None);
        return;
    };
    let id = make_node_id(NodeKind::Function, &state.file_path, &name, line(node));
    state.nodes.push(build_node(
        node,
        source,
        state,
        NodeKind::Function,
        &name,
        &name,
        id.clone(),
    ));
    add_contains_edge(state, &id, line(node));
    visit_children(node, source, state, Some(&id));
}

fn visit_method(node: TsNode<'_>, source: &str, state: &mut GoState) {
    let Some(name) = name_of(node, source) else {
        visit_children(node, source, state, None);
        return;
    };
    let receiver_type = receiver_type(node, source).unwrap_or_default();
    let qualified = if receiver_type.is_empty() {
        name.clone()
    } else {
        format!("{receiver_type}::{name}")
    };
    let id = make_node_id(NodeKind::Method, &state.file_path, &qualified, line(node));
    state.nodes.push(build_node(
        node,
        source,
        state,
        NodeKind::Method,
        &name,
        &qualified,
        id.clone(),
    ));
    add_contains_edge(state, &id, line(node));
    if !receiver_type.is_empty() {
        state.methods.push(MethodInfo {
            receiver_type,
            method_name: name,
        });
    }
    visit_children(node, source, state, Some(&id));
}

fn visit_type_declaration(node: TsNode<'_>, source: &str, state: &mut GoState) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "type_spec" {
            visit_node(child, source, state, None);
            continue;
        }
        let Some(name) = name_of(child, source) else {
            continue;
        };
        let body = text(child, source);
        let kind = if body.contains("interface") {
            NodeKind::Interface
        } else if body.contains("struct") {
            NodeKind::Struct
        } else {
            NodeKind::TypeAlias
        };
        let id = make_node_id(kind, &state.file_path, &name, line(child));
        let mut type_node = build_node(child, source, state, kind, &name, &name, id.clone());
        type_node.docstring = docstring_before(node, source);
        state.nodes.push(type_node);
        add_contains_edge(state, &id, line(child));
    }
}

fn visit_import(node: TsNode<'_>, source: &str, state: &mut GoState) {
    let imports = strings_in(node, source);
    for import_name in imports {
        let qualified = format!("import {import_name}");
        let id = make_node_id(NodeKind::Import, &state.file_path, &qualified, line(node));
        if state.nodes.iter().any(|n| n.id == id) {
            continue;
        }
        state.nodes.push(build_node(
            node,
            source,
            state,
            NodeKind::Import,
            &import_name,
            &qualified,
            id.clone(),
        ));
        add_contains_edge(state, &id, line(node));
        state.edges.push(edge(
            state.file_id.clone(),
            id,
            EdgeKind::Imports,
            Some(line(node)),
        ));
    }
}

fn add_interface_implements_edges(state: &mut GoState) {
    let structs: Vec<Node> = state
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Struct)
        .cloned()
        .collect();
    let interfaces: Vec<Node> = state
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Interface)
        .cloned()
        .collect();

    let mut methods_by_receiver: HashMap<String, BTreeSet<String>> = HashMap::new();
    for method in &state.methods {
        methods_by_receiver
            .entry(method.receiver_type.clone())
            .or_default()
            .insert(method.method_name.clone());
    }

    for strukt in &structs {
        let Some(methods) = methods_by_receiver.get(&strukt.name) else {
            continue;
        };
        for interface in &interfaces {
            let required = interface_methods(&interface.signature.clone().unwrap_or_default());
            if !required.is_empty() && required.iter().all(|m| methods.contains(m)) {
                state.edges.push(edge(
                    strukt.id.clone(),
                    interface.id.clone(),
                    EdgeKind::Implements,
                    Some(strukt.start_line),
                ));
            }
        }
    }
}

fn build_node(
    node: TsNode<'_>,
    source: &str,
    state: &GoState,
    kind: NodeKind,
    name: &str,
    qualified_name: &str,
    id: String,
) -> Node {
    Node {
        id,
        kind,
        name: name.to_string(),
        qualified_name: qualified_name.to_string(),
        file_path: state.file_path.clone(),
        language: Language::Go,
        start_line: line(node),
        end_line: node.end_position().row as u32 + 1,
        start_column: node.start_position().column as u32,
        end_column: node.end_position().column as u32,
        signature: Some(signature_text(node, source)),
        docstring: docstring_before(node, source),
        visibility: None,
        is_exported: name
            .chars()
            .next()
            .map(|c| c.is_ascii_uppercase())
            .unwrap_or(false),
        is_async: false,
    }
}

fn resolve_calls(state: &mut GoState) {
    let mut by_name: HashMap<String, Vec<Node>> = HashMap::new();
    for node in &state.nodes {
        if matches!(node.kind, NodeKind::Function | NodeKind::Method) {
            by_name
                .entry(node.name.clone())
                .or_default()
                .push(node.clone());
        }
    }

    let mut unresolved = Vec::new();
    for call in state.calls.drain(..) {
        let simple = call
            .reference_name
            .rsplit('.')
            .next()
            .unwrap_or(&call.reference_name);
        if let Some(target) = by_name
            .get(&call.reference_name)
            .or_else(|| by_name.get(simple))
            .and_then(|nodes| nodes.first())
        {
            state.edges.push(edge(
                call.from_node_id,
                target.id.clone(),
                EdgeKind::Calls,
                Some(call.line),
            ));
        } else {
            unresolved.push(call);
        }
    }
    state.calls = unresolved;
}

fn add_contains_edge(state: &mut GoState, child: &str, line: u32) {
    state.edges.push(edge(
        state.file_id.clone(),
        child.to_string(),
        EdgeKind::Contains,
        Some(line),
    ));
}

fn edge(source: String, target: String, kind: EdgeKind, line: Option<u32>) -> Edge {
    Edge {
        source,
        target,
        kind,
        metadata: None,
        line,
        provenance: None,
    }
}

fn name_of(node: TsNode<'_>, source: &str) -> Option<String> {
    if let Some(name) = node.child_by_field_name("name") {
        return Some(text(name, source).to_string());
    }
    let mut cursor = node.walk();
    let result = node
        .children(&mut cursor)
        .find(|child| child.kind() == "identifier" || child.kind() == "type_identifier")
        .map(|child| text(child, source).to_string());
    result
}

fn receiver_type(node: TsNode<'_>, source: &str) -> Option<String> {
    let raw = node
        .child_by_field_name("receiver")
        .map(|r| text(r, source).to_string())
        .unwrap_or_else(|| text(node, source).to_string());
    let inside = raw
        .split_once('(')
        .and_then(|(_, rest)| rest.split_once(')').map(|(inner, _)| inner))
        .unwrap_or("");
    inside
        .split_whitespace()
        .last()
        .map(|s| s.trim_start_matches('*').to_string())
        .filter(|s| !s.is_empty())
}

fn call_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("function")
        .map(|child| callable_text(child, source))
        .filter(|name| !name.is_empty())
}

fn callable_text(node: TsNode<'_>, source: &str) -> String {
    match node.kind() {
        "identifier" => text(node, source).to_string(),
        "selector_expression" => node
            .child_by_field_name("field")
            .map(|field| text(field, source).to_string())
            .unwrap_or_else(|| compact_ws(text(node, source))),
        _ => compact_ws(text(node, source)),
    }
}

fn strings_in(node: TsNode<'_>, source: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_strings(node, source, &mut out);
    out
}

fn collect_strings(node: TsNode<'_>, source: &str, out: &mut Vec<String>) {
    if node.kind() == "interpreted_string_literal" || node.kind() == "raw_string_literal" {
        out.push(unquote(text(node, source)));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_strings(child, source, out);
    }
}

fn interface_methods(signature: &str) -> BTreeSet<String> {
    signature
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let name = trimmed.split_once('(')?.0.trim();
            if name.is_empty() || name.contains(' ') {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

fn signature_text(node: TsNode<'_>, source: &str) -> String {
    match node.kind() {
        "function_declaration" | "method_declaration" => {
            let raw = text(node, source);
            compact_ws(raw.split('{').next().unwrap_or(raw).trim())
        }
        _ => text(node, source).trim().to_string(),
    }
}

fn docstring_before(node: TsNode<'_>, source: &str) -> Option<String> {
    let prefix = &source[..node.start_byte().min(source.len())];
    let mut docs = Vec::new();
    for line in prefix.lines().rev() {
        let trimmed = line.trim();
        if let Some(doc) = trimmed.strip_prefix("//") {
            docs.push(doc.trim().to_string());
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        break;
    }
    if docs.is_empty() {
        None
    } else {
        docs.reverse();
        Some(docs.join("\n"))
    }
}

fn unquote(s: &str) -> String {
    s.trim()
        .trim_matches('"')
        .trim_matches('`')
        .trim_matches('\'')
        .to_string()
}

fn text<'a>(node: TsNode<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn compact_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn line(node: TsNode<'_>) -> u32 {
    node.start_position().row as u32 + 1
}

fn posix_path(file: &ScannedFile) -> String {
    file.relative_path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::Extractor;
    use std::path::PathBuf;

    fn scanned(rel: &str) -> ScannedFile {
        ScannedFile {
            path: PathBuf::from(format!("/abs/{rel}")),
            relative_path: PathBuf::from(rel),
            language: Language::Go,
            size: 0,
            content_hash: "hash".to_string(),
        }
    }

    fn extract(source: &str) -> ExtractedFile {
        Extractor::new()
            .extract(&scanned("cmd/app/main.go"), source)
            .unwrap()
    }

    fn node<'a>(file: &'a ExtractedFile, kind: NodeKind, name: &str) -> &'a Node {
        file.nodes
            .iter()
            .find(|n| n.kind == kind && n.name == name)
            .unwrap_or_else(|| panic!("missing {kind:?} {name}"))
    }

    #[test]
    fn extracts_go_symbols_imports_calls_and_implements() {
        let source = r#"
package main

import (
    "fmt"
    "net/http"
)

// Runner executes work.
type Runner interface {
    Run()
}

type Worker struct {
    Name string
}

func (w *Worker) Run() {
    helper()
    missing()
}

func helper() {
    fmt.Println(http.MethodGet)
}
"#;
        let file = extract(source);
        let runner = node(&file, NodeKind::Interface, "Runner");
        let worker = node(&file, NodeKind::Struct, "Worker");
        let run = node(&file, NodeKind::Method, "Run");
        let helper = node(&file, NodeKind::Function, "helper");

        assert_eq!(runner.docstring.as_deref(), Some("Runner executes work."));
        assert!(worker.is_exported);
        assert_eq!(run.qualified_name, "Worker::Run");
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Import && n.name == "fmt"));
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Import && n.name == "net/http"));
        assert!(file
            .edges
            .iter()
            .any(|e| { e.kind == EdgeKind::Calls && e.source == run.id && e.target == helper.id }));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::Implements && e.source == worker.id && e.target == runner.id
        }));
        assert!(file.unresolved_refs.iter().any(|r| {
            r.from_node_id == run.id && r.reference_name == "missing" && r.reference_kind == "call"
        }));
    }
}
