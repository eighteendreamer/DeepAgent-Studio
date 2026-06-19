//! Python symbol extraction.

use std::collections::HashMap;

use tree_sitter::Node as TsNode;

use super::{file_node_id, make_node_id, ExtractedFile, ExtractorImpl};
use crate::scanner::ScannedFile;
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind, UnresolvedRef};

/// Extractor for Python source files.
#[derive(Debug, Default)]
pub struct PythonExtractor;

#[derive(Debug, Clone)]
struct Container {
    id: String,
    qualified_name: String,
    kind: NodeKind,
}

#[derive(Debug, Clone)]
struct CallSite {
    from_node_id: String,
    reference_name: String,
    line: u32,
    file_path: String,
}

#[derive(Debug, Default)]
struct PythonState {
    file_path: String,
    file_id: String,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    calls: Vec<CallSite>,
}

impl ExtractorImpl for PythonExtractor {
    fn extract(&self, tree: &tree_sitter::Tree, source: &str, file: &ScannedFile) -> ExtractedFile {
        let mut out = ExtractedFile::file_only(file, source);
        let file_path = posix_path(file);
        let mut state = PythonState {
            file_id: file_node_id(&file_path),
            file_path,
            nodes: Vec::new(),
            edges: Vec::new(),
            calls: Vec::new(),
        };

        visit_children(tree.root_node(), source, &mut state, &mut Vec::new(), None);
        resolve_calls(&mut state);
        detect_routes(source, &mut state);

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
    state: &mut PythonState,
    containers: &mut Vec<Container>,
    current_callable: Option<&str>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_node(child, source, state, containers, current_callable);
    }
}

fn visit_node(
    node: TsNode<'_>,
    source: &str,
    state: &mut PythonState,
    containers: &mut Vec<Container>,
    current_callable: Option<&str>,
) {
    match node.kind() {
        "function_definition" => visit_function(node, source, state, containers, false),
        "class_definition" => visit_class(node, source, state, containers),
        "import_statement" | "import_from_statement" => {
            visit_import(node, source, state, containers)
        }
        "call" => {
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
            visit_children(node, source, state, containers, current_callable);
        }
        _ => visit_children(node, source, state, containers, current_callable),
    }
}

fn visit_function(
    node: TsNode<'_>,
    source: &str,
    state: &mut PythonState,
    containers: &mut Vec<Container>,
    force_method: bool,
) {
    let Some(name) = name_of(node, source) else {
        visit_children(node, source, state, containers, None);
        return;
    };
    let in_class = containers
        .last()
        .map(|c| c.kind == NodeKind::Class)
        .unwrap_or(false);
    let kind = if force_method || in_class {
        NodeKind::Method
    } else {
        NodeKind::Function
    };
    let qualified = qualify(containers, &name);
    let id = make_node_id(kind, &state.file_path, &qualified, line(node));
    state.nodes.push(build_node(
        node,
        source,
        state,
        kind,
        &name,
        &qualified,
        id.clone(),
    ));
    add_contains_edge(
        state,
        containers.last().map(|c| c.id.as_str()),
        &id,
        line(node),
    );

    containers.push(Container {
        id: id.clone(),
        qualified_name: qualified,
        kind,
    });
    visit_children(node, source, state, containers, Some(&id));
    containers.pop();
}

fn visit_class(
    node: TsNode<'_>,
    source: &str,
    state: &mut PythonState,
    containers: &mut Vec<Container>,
) {
    let Some(name) = name_of(node, source) else {
        visit_children(node, source, state, containers, None);
        return;
    };
    let qualified = qualify(containers, &name);
    let id = make_node_id(NodeKind::Class, &state.file_path, &qualified, line(node));
    state.nodes.push(build_node(
        node,
        source,
        state,
        NodeKind::Class,
        &name,
        &qualified,
        id.clone(),
    ));
    add_contains_edge(
        state,
        containers.last().map(|c| c.id.as_str()),
        &id,
        line(node),
    );
    add_extends_edges(node, source, state, &id);

    containers.push(Container {
        id,
        qualified_name: qualified,
        kind: NodeKind::Class,
    });
    visit_children(node, source, state, containers, None);
    containers.pop();
}

fn visit_import(node: TsNode<'_>, source: &str, state: &mut PythonState, containers: &[Container]) {
    let text = compact_ws(text(node, source));
    let name = text
        .strip_prefix("import ")
        .or_else(|| text.strip_prefix("from "))
        .unwrap_or(&text)
        .trim()
        .to_string();
    if name.is_empty() {
        return;
    }
    let qualified = qualify(containers, &format!("import {name}"));
    let id = make_node_id(NodeKind::Import, &state.file_path, &qualified, line(node));
    state.nodes.push(build_node(
        node,
        source,
        state,
        NodeKind::Import,
        &name,
        &qualified,
        id.clone(),
    ));
    add_contains_edge(
        state,
        containers.last().map(|c| c.id.as_str()),
        &id,
        line(node),
    );
    state.edges.push(edge(
        containers
            .last()
            .map(|c| c.id.clone())
            .unwrap_or_else(|| state.file_id.clone()),
        id,
        EdgeKind::Imports,
        Some(line(node)),
    ));
}

fn add_extends_edges(node: TsNode<'_>, source: &str, state: &mut PythonState, from_id: &str) {
    for superclass in descendant_kinds(node, &["argument_list"]) {
        for name in identifiers_in(superclass, source) {
            if let Some(target) = find_node_by_name(state, &name, Some(NodeKind::Class)) {
                state.edges.push(edge(
                    from_id.to_string(),
                    target.id.clone(),
                    EdgeKind::Extends,
                    Some(line(superclass)),
                ));
            }
        }
    }
}

fn build_node(
    node: TsNode<'_>,
    source: &str,
    state: &PythonState,
    kind: NodeKind,
    name: &str,
    qualified_name: &str,
    id: String,
) -> Node {
    let signature = signature_text(node, source);
    Node {
        id,
        kind,
        name: name.to_string(),
        qualified_name: qualified_name.to_string(),
        file_path: state.file_path.clone(),
        language: Language::Python,
        start_line: line(node),
        end_line: node.end_position().row as u32 + 1,
        start_column: node.start_position().column as u32,
        end_column: node.end_position().column as u32,
        signature: Some(signature.clone()),
        docstring: docstring_in_body(node, source),
        visibility: None,
        is_exported: false,
        is_async: signature.starts_with("async def "),
    }
}

fn resolve_calls(state: &mut PythonState) {
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

fn detect_routes(source: &str, state: &mut PythonState) {
    let mut pending_decorators: Vec<(String, String, u32)> = Vec::new();
    for (index, raw_line) in source.lines().enumerate() {
        let line_no = index as u32 + 1;
        let line = raw_line.trim();

        if let Some((method, path)) = parse_fastapi_decorator(line) {
            pending_decorators.push((method, path, line_no));
            continue;
        }

        if let Some(function) = python_def_name(line) {
            for (method, path, decorator_line) in pending_decorators.drain(..) {
                add_route(state, &method, &path, Some(&function), decorator_line);
            }
        } else if !line.starts_with('@') && !line.is_empty() {
            pending_decorators.clear();
        }

        if let Some((method, path, handler)) = parse_django_path(line) {
            add_route(state, &method, &path, Some(&handler), line_no);
        }
    }
}

fn add_route(state: &mut PythonState, method: &str, path: &str, handler: Option<&str>, line: u32) {
    let name = format!("{} {}", method.to_uppercase(), path);
    let qualified = match handler {
        Some(handler) => format!("route:{name} -> {handler}"),
        None => format!("route:{name}"),
    };
    let id = make_node_id(NodeKind::Route, &state.file_path, &qualified, line);
    if state.nodes.iter().any(|node| node.id == id) {
        return;
    }
    state.nodes.push(Node {
        id: id.clone(),
        kind: NodeKind::Route,
        name,
        qualified_name: qualified,
        file_path: state.file_path.clone(),
        language: Language::Python,
        start_line: line,
        end_line: line,
        start_column: 0,
        end_column: 0,
        signature: Some(format!("{} {}", method.to_uppercase(), path)),
        docstring: None,
        visibility: None,
        is_exported: false,
        is_async: false,
    });
    add_contains_edge(state, None, &id, line);
    if let Some(handler) = handler {
        let simple = handler.rsplit('.').next().unwrap_or(handler);
        if let Some(target) = find_node_by_name(state, simple, None) {
            state.edges.push(edge(
                id,
                target.id.clone(),
                EdgeKind::References,
                Some(line),
            ));
        }
    }
}

fn parse_fastapi_decorator(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix('@')?;
    let dot = rest.find('.')?;
    let after_dot = &rest[dot + 1..];
    let method_end = after_dot.find('(')?;
    let method = &after_dot[..method_end];
    if !matches!(
        method,
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
    ) {
        return None;
    }
    let (path, _) = parse_first_string_arg(&after_dot[method_end + 1..])?;
    Some((method.to_string(), path))
}

fn parse_django_path(line: &str) -> Option<(String, String, String)> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("path(")
        .or_else(|| trimmed.strip_prefix("re_path("))?;
    let (path, after_path) = parse_first_string_arg(rest)?;
    let handler = after_path
        .trim_start()
        .strip_prefix(',')?
        .split([',', ')'])
        .next()?
        .trim()
        .to_string();
    if handler.is_empty() {
        None
    } else {
        Some(("route".to_string(), path, handler))
    }
}

fn parse_first_string_arg(s: &str) -> Option<(String, &str)> {
    let trimmed = s.trim_start();
    let quote = trimmed.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &trimmed[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some((rest[..end].to_string(), &rest[end + quote.len_utf8()..]))
}

fn python_def_name(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("async def ")
        .or_else(|| line.strip_prefix("def "))?;
    let name: String = rest
        .chars()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn add_contains_edge(state: &mut PythonState, parent: Option<&str>, child: &str, line: u32) {
    state.edges.push(edge(
        parent.unwrap_or(&state.file_id).to_string(),
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
    node.child_by_field_name("name")
        .map(|name| text(name, source).to_string())
}

fn call_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("function")
        .map(|child| callable_text(child, source))
        .filter(|name| !name.is_empty())
}

fn callable_text(node: TsNode<'_>, source: &str) -> String {
    match node.kind() {
        "identifier" => text(node, source).to_string(),
        "attribute" => node
            .child_by_field_name("attribute")
            .map(|field| text(field, source).to_string())
            .unwrap_or_else(|| compact_ws(text(node, source))),
        _ => compact_ws(text(node, source)),
    }
}

fn descendant_kinds<'a>(node: TsNode<'a>, kinds: &[&str]) -> Vec<TsNode<'a>> {
    let mut out = Vec::new();
    collect_descendant_kinds(node, kinds, &mut out);
    out
}

fn collect_descendant_kinds<'a>(node: TsNode<'a>, kinds: &[&str], out: &mut Vec<TsNode<'a>>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            out.push(child);
        }
        collect_descendant_kinds(child, kinds, out);
    }
}

fn identifiers_in(node: TsNode<'_>, source: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_identifiers(node, source, &mut out);
    out
}

fn collect_identifiers(node: TsNode<'_>, source: &str, out: &mut Vec<String>) {
    if node.kind() == "identifier" {
        out.push(text(node, source).to_string());
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifiers(child, source, out);
    }
}

fn find_node_by_name<'a>(
    state: &'a PythonState,
    name: &str,
    kind: Option<NodeKind>,
) -> Option<&'a Node> {
    state.nodes.iter().find(|node| {
        kind.map(|k| node.kind == k).unwrap_or(true)
            && (node.name == name || node.qualified_name.ends_with(&format!("::{name}")))
    })
}

fn qualify(containers: &[Container], name: &str) -> String {
    match containers.last() {
        Some(parent) => format!("{}::{name}", parent.qualified_name),
        None => name.to_string(),
    }
}

fn signature_text(node: TsNode<'_>, source: &str) -> String {
    let raw = text(node, source);
    let before_body = raw.split(':').next().unwrap_or(raw);
    compact_ws(before_body.trim())
}

fn docstring_in_body(node: TsNode<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "block" {
            continue;
        }
        let mut block_cursor = child.walk();
        for stmt in child.children(&mut block_cursor) {
            let raw = text(stmt, source).trim();
            let doc = raw.trim_matches('"').trim_matches('\'').trim().to_string();
            if !doc.is_empty() && (raw.starts_with("\"\"\"") || raw.starts_with("'''")) {
                return Some(doc);
            }
            if !raw.is_empty() {
                break;
            }
        }
    }
    None
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
            language: Language::Python,
            size: 0,
            content_hash: "hash".to_string(),
        }
    }

    fn extract(source: &str) -> ExtractedFile {
        Extractor::new()
            .extract(&scanned("pkg/app.py"), source)
            .unwrap()
    }

    fn node<'a>(file: &'a ExtractedFile, kind: NodeKind, name: &str) -> &'a Node {
        file.nodes
            .iter()
            .find(|n| n.kind == kind && n.name == name)
            .unwrap_or_else(|| panic!("missing {kind:?} {name}"))
    }

    #[test]
    fn extracts_python_symbols_imports_extends_and_calls() {
        let source = r#"
import os
from pkg.base import Base

class Base:
    def base(self):
        pass

class Worker(Base):
    """Does work."""
    async def run(self):
        helper()
        missing()

def helper():
    return os.getcwd()
"#;
        let file = extract(source);
        let base = node(&file, NodeKind::Class, "Base");
        let worker = node(&file, NodeKind::Class, "Worker");
        let run = node(&file, NodeKind::Method, "run");
        let helper = node(&file, NodeKind::Function, "helper");

        assert_eq!(worker.language, Language::Python);
        assert_eq!(worker.docstring.as_deref(), Some("Does work."));
        assert!(run.is_async);
        assert_eq!(run.qualified_name, "Worker::run");
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Import && n.name == "os"));
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Import && n.name == "pkg.base import Base"));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::Extends && e.source == worker.id && e.target == base.id
        }));
        assert!(file
            .edges
            .iter()
            .any(|e| { e.kind == EdgeKind::Calls && e.source == run.id && e.target == helper.id }));
        assert!(file.unresolved_refs.iter().any(|r| {
            r.from_node_id == run.id && r.reference_name == "missing" && r.reference_kind == "call"
        }));
    }

    #[test]
    fn extracts_python_framework_routes() {
        let source = r#"
@app.get("/users")
def list_users():
    pass

urlpatterns = [
    path("users/new/", views.create_user, name="create_user"),
]

def create_user(request):
    pass
"#;
        let file = extract(source);
        let list_users = node(&file, NodeKind::Function, "list_users");
        let create_user = node(&file, NodeKind::Function, "create_user");
        let get_route = node(&file, NodeKind::Route, "GET /users");
        let django_route = node(&file, NodeKind::Route, "ROUTE users/new/");

        assert_eq!(get_route.signature.as_deref(), Some("GET /users"));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::References && e.source == get_route.id && e.target == list_users.id
        }));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::References
                && e.source == django_route.id
                && e.target == create_user.id
        }));
    }
}
