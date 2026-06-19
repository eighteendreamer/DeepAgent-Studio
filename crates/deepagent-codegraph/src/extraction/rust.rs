//! Rust symbol extraction.
//!
//! This extractor intentionally starts with a small, dependable AST traversal:
//! it records the common Rust item nodes, container relationships, same-file
//! calls that can be matched by name, and parks the remaining calls for the
//! later cross-file resolver.

use std::collections::HashMap;

use tree_sitter::Node as TsNode;

use super::{file_node_id, make_node_id, ExtractedFile, ExtractorImpl};
use crate::scanner::ScannedFile;
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind, UnresolvedRef};

/// Extractor for Rust source files.
#[derive(Debug, Default)]
pub struct RustExtractor;

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
struct RustState {
    file_path: String,
    file_id: String,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    calls: Vec<CallSite>,
}

impl ExtractorImpl for RustExtractor {
    fn extract(&self, tree: &tree_sitter::Tree, source: &str, file: &ScannedFile) -> ExtractedFile {
        let mut out = ExtractedFile::file_only(file, source);
        let file_path = posix_path(file);
        let mut state = RustState {
            file_id: file_node_id(&file_path),
            file_path,
            nodes: Vec::new(),
            edges: Vec::new(),
            calls: Vec::new(),
        };

        let root = tree.root_node();
        visit_children(root, source, &mut state, &mut Vec::new(), None);

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
    state: &mut RustState,
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
    state: &mut RustState,
    containers: &mut Vec<Container>,
    current_callable: Option<&str>,
) {
    match node.kind() {
        "function_item" => visit_function(node, source, state, containers),
        "struct_item" => visit_item(node, source, state, containers, NodeKind::Struct),
        "enum_item" => visit_item(node, source, state, containers, NodeKind::Enum),
        "trait_item" => visit_item(node, source, state, containers, NodeKind::Trait),
        "mod_item" => visit_item(node, source, state, containers, NodeKind::Module),
        "const_item" => visit_item(node, source, state, containers, NodeKind::Constant),
        "static_item" => visit_item(node, source, state, containers, NodeKind::Constant),
        "type_item" => visit_item(node, source, state, containers, NodeKind::TypeAlias),
        "impl_item" => visit_impl(node, source, state, containers),
        "use_declaration" => visit_use(node, source, state, containers),
        "field_declaration" => visit_field(node, source, state, containers),
        "enum_variant" => visit_enum_variant(node, source, state, containers),
        "call_expression" | "macro_invocation" => {
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
    state: &mut RustState,
    containers: &mut Vec<Container>,
) {
    let Some(name) = name_of(node, source) else {
        visit_children(node, source, state, containers, None);
        return;
    };
    let in_impl = containers
        .iter()
        .rev()
        .any(|c| c.kind == NodeKind::Namespace);
    let in_trait = containers.iter().rev().any(|c| c.kind == NodeKind::Trait);
    let kind = if in_impl || in_trait {
        NodeKind::Method
    } else {
        NodeKind::Function
    };

    let qualified = qualify(containers, &name);
    let id = make_node_id(kind, &state.file_path, &qualified, line(node));
    let function = build_node(node, source, state, kind, &name, &qualified, id.clone());
    state.nodes.push(function);
    add_contains_edge(
        state,
        containers.last().map(|c| c.id.as_str()),
        &id,
        line(node),
    );

    let container = Container {
        id: id.clone(),
        qualified_name: qualified,
        kind,
    };
    containers.push(container);
    visit_children(node, source, state, containers, Some(&id));
    containers.pop();
}

fn visit_item(
    node: TsNode<'_>,
    source: &str,
    state: &mut RustState,
    containers: &mut Vec<Container>,
    kind: NodeKind,
) {
    let Some(name) = name_of(node, source) else {
        visit_children(node, source, state, containers, None);
        return;
    };
    let qualified = qualify(containers, &name);
    let id = make_node_id(kind, &state.file_path, &qualified, line(node));
    let item = build_node(node, source, state, kind, &name, &qualified, id.clone());
    state.nodes.push(item);
    add_contains_edge(
        state,
        containers.last().map(|c| c.id.as_str()),
        &id,
        line(node),
    );

    let container = Container {
        id,
        qualified_name: qualified,
        kind,
    };
    containers.push(container);
    visit_children(node, source, state, containers, None);
    containers.pop();
}

fn visit_impl(
    node: TsNode<'_>,
    source: &str,
    state: &mut RustState,
    containers: &mut Vec<Container>,
) {
    let header = signature_text(node, source);
    let name = impl_name(&header);
    let qualified = qualify(containers, &name);
    let id = make_node_id(
        NodeKind::Namespace,
        &state.file_path,
        &qualified,
        line(node),
    );
    let impl_node = build_node(
        node,
        source,
        state,
        NodeKind::Namespace,
        &name,
        &qualified,
        id.clone(),
    );
    state.nodes.push(impl_node);
    add_contains_edge(
        state,
        containers.last().map(|c| c.id.as_str()),
        &id,
        line(node),
    );

    if let Some(trait_name) = trait_name_from_impl_header(&header) {
        if let Some(target) = find_node_by_name(state, &trait_name, Some(NodeKind::Trait)) {
            state.edges.push(edge(
                id.clone(),
                target.id.clone(),
                EdgeKind::Implements,
                Some(line(node)),
            ));
        }
    }

    containers.push(Container {
        id,
        qualified_name: qualified,
        kind: NodeKind::Namespace,
    });
    visit_children(node, source, state, containers, None);
    containers.pop();
}

fn visit_use(node: TsNode<'_>, source: &str, state: &mut RustState, containers: &[Container]) {
    let text = compact_ws(text(node, source).trim_end_matches(';'));
    let name = text
        .strip_prefix("use ")
        .unwrap_or(&text)
        .trim()
        .to_string();
    if name.is_empty() {
        return;
    }
    let qualified = qualify(containers, &format!("use {name}"));
    let id = make_node_id(NodeKind::Import, &state.file_path, &qualified, line(node));
    let import = build_node(
        node,
        source,
        state,
        NodeKind::Import,
        &name,
        &qualified,
        id.clone(),
    );
    state.nodes.push(import);
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

fn visit_field(node: TsNode<'_>, source: &str, state: &mut RustState, containers: &[Container]) {
    let parent_is_struct = containers
        .last()
        .map(|c| c.kind == NodeKind::Struct)
        .unwrap_or(false);
    if !parent_is_struct {
        return;
    }
    let Some(name) = name_of(node, source) else {
        return;
    };
    let qualified = qualify(containers, &name);
    let id = make_node_id(NodeKind::Field, &state.file_path, &qualified, line(node));
    let field = build_node(
        node,
        source,
        state,
        NodeKind::Field,
        &name,
        &qualified,
        id.clone(),
    );
    state.nodes.push(field);
    add_contains_edge(
        state,
        containers.last().map(|c| c.id.as_str()),
        &id,
        line(node),
    );
}

fn visit_enum_variant(
    node: TsNode<'_>,
    source: &str,
    state: &mut RustState,
    containers: &[Container],
) {
    let parent_is_enum = containers
        .last()
        .map(|c| c.kind == NodeKind::Enum)
        .unwrap_or(false);
    if !parent_is_enum {
        return;
    }
    let Some(name) = name_of(node, source) else {
        return;
    };
    let qualified = qualify(containers, &name);
    let id = make_node_id(
        NodeKind::EnumMember,
        &state.file_path,
        &qualified,
        line(node),
    );
    let member = build_node(
        node,
        source,
        state,
        NodeKind::EnumMember,
        &name,
        &qualified,
        id.clone(),
    );
    state.nodes.push(member);
    add_contains_edge(
        state,
        containers.last().map(|c| c.id.as_str()),
        &id,
        line(node),
    );
}

fn build_node(
    node: TsNode<'_>,
    source: &str,
    state: &RustState,
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
        language: Language::Rust,
        start_line: line(node),
        end_line: node.end_position().row as u32 + 1,
        start_column: node.start_position().column as u32,
        end_column: node.end_position().column as u32,
        signature: Some(signature.clone()),
        docstring: docstring_before(node, source),
        visibility: visibility(node, source),
        is_exported: visibility(node, source).as_deref() == Some("pub"),
        is_async: signature.contains("async fn"),
    }
}

fn resolve_calls(state: &mut RustState) {
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
            .rsplit("::")
            .next()
            .unwrap_or(&call.reference_name);
        let candidates = by_name
            .get(&call.reference_name)
            .or_else(|| by_name.get(simple));
        if let Some(candidates) = candidates {
            if let Some(target) = candidates.first() {
                state.edges.push(edge(
                    call.from_node_id,
                    target.id.clone(),
                    EdgeKind::Calls,
                    Some(call.line),
                ));
                continue;
            }
        }
        unresolved.push(call);
    }
    state.calls = unresolved;
}

fn detect_routes(source: &str, state: &mut RustState) {
    let lines: Vec<&str> = source.lines().collect();
    let mut pending_attrs: Vec<(String, String, u32)> = Vec::new();
    for (index, raw_line) in lines.iter().enumerate() {
        let line_no = index as u32 + 1;
        let line = raw_line.trim();

        if let Some((method, path)) = parse_actix_attr(line) {
            pending_attrs.push((method, path, line_no));
            continue;
        }

        if let Some(function) = rust_fn_name(line) {
            for (method, path, attr_line) in pending_attrs.drain(..) {
                add_route(state, &method, &path, Some(&function), attr_line);
            }
        } else if !line.starts_with("#[") && !line.is_empty() {
            pending_attrs.clear();
        }

        if let Some((method, path, handler)) = parse_axum_route(line) {
            add_route(state, &method, &path, Some(&handler), line_no);
        }
    }
}

fn add_route(state: &mut RustState, method: &str, path: &str, handler: Option<&str>, line: u32) {
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
        language: Language::Rust,
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
        if let Some(target) = find_node_by_name(state, handler, None) {
            state.edges.push(edge(
                id,
                target.id.clone(),
                EdgeKind::References,
                Some(line),
            ));
        }
    }
}

fn parse_axum_route(line: &str) -> Option<(String, String, String)> {
    let route_start = line.find(".route(")?;
    let after_route = &line[route_start + ".route(".len()..];
    let (path, after_path) = parse_first_string_arg(after_route)?;
    let after_comma = after_path.trim_start().strip_prefix(',')?.trim_start();
    let method_end = after_comma.find('(')?;
    let method = after_comma[..method_end].trim();
    if !matches!(
        method,
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
    ) {
        return None;
    }
    let handler = after_comma[method_end + 1..]
        .split([',', ')'])
        .next()?
        .trim()
        .trim_start_matches("move ")
        .trim_start_matches("async ")
        .to_string();
    if handler.is_empty() {
        None
    } else {
        Some((method.to_string(), path, handler))
    }
}

fn parse_actix_attr(line: &str) -> Option<(String, String)> {
    let inner = line.strip_prefix("#[")?.strip_suffix(']')?;
    let method_end = inner.find('(')?;
    let method = &inner[..method_end];
    if !matches!(
        method,
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
    ) {
        return None;
    }
    let (path, _) = parse_first_string_arg(&inner[method_end + 1..])?;
    Some((method.to_string(), path))
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

fn rust_fn_name(line: &str) -> Option<String> {
    let fn_pos = line.find("fn ")?;
    let rest = &line[fn_pos + 3..];
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

fn add_contains_edge(state: &mut RustState, parent: Option<&str>, child: &str, line: u32) {
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

fn find_node_by_name<'a>(
    state: &'a RustState,
    name: &str,
    kind: Option<NodeKind>,
) -> Option<&'a Node> {
    state.nodes.iter().find(|node| {
        kind.map(|k| node.kind == k).unwrap_or(true)
            && (node.name == name || node.qualified_name.ends_with(&format!("::{name}")))
    })
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

fn call_name(node: TsNode<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "macro_invocation" => {
            let mut cursor = node.walk();
            let result = node
                .children(&mut cursor)
                .find(|child| child.kind() == "identifier")
                .map(|child| text(child, source).trim_end_matches('!').to_string());
            result
        }
        "call_expression" => node
            .child_by_field_name("function")
            .map(|child| callable_text(child, source)),
        _ => None,
    }
    .filter(|name| !name.is_empty())
}

fn callable_text(node: TsNode<'_>, source: &str) -> String {
    match node.kind() {
        "identifier" | "scoped_identifier" | "field_identifier" | "type_identifier" => {
            text(node, source).to_string()
        }
        "field_expression" => node
            .child_by_field_name("field")
            .map(|field| text(field, source).to_string())
            .unwrap_or_else(|| compact_ws(text(node, source))),
        _ => compact_ws(text(node, source)),
    }
}

fn qualify(containers: &[Container], name: &str) -> String {
    match containers.last() {
        Some(parent) => format!("{}::{name}", parent.qualified_name),
        None => name.to_string(),
    }
}

fn impl_name(header: &str) -> String {
    let header = header.trim_end_matches('{').trim();
    header
        .strip_prefix("impl")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("impl {s}"))
        .unwrap_or_else(|| "impl".to_string())
}

fn trait_name_from_impl_header(header: &str) -> Option<String> {
    let rest = header.trim().strip_prefix("impl")?.trim();
    let (trait_part, _) = rest.split_once(" for ")?;
    trait_part.split("::").last().map(|s| {
        s.trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_string()
    })
}

fn signature_text(node: TsNode<'_>, source: &str) -> String {
    let raw = text(node, source);
    let before_body = raw.split('{').next().unwrap_or(raw);
    compact_ws(before_body.trim().trim_end_matches(';'))
}

fn visibility(node: TsNode<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let result = node
        .children(&mut cursor)
        .find(|child| child.kind() == "visibility_modifier")
        .map(|child| compact_ws(text(child, source)));
    result
}

fn docstring_before(node: TsNode<'_>, source: &str) -> Option<String> {
    let prefix = &source[..node.start_byte().min(source.len())];
    let mut docs = Vec::new();
    for line in prefix.lines().rev() {
        let trimmed = line.trim();
        if let Some(doc) = trimmed.strip_prefix("///") {
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
            language: Language::Rust,
            size: 0,
            content_hash: "hash".to_string(),
        }
    }

    fn extract(source: &str) -> ExtractedFile {
        Extractor::new()
            .extract(&scanned("src/lib.rs"), source)
            .unwrap()
    }

    fn node<'a>(file: &'a ExtractedFile, kind: NodeKind, name: &str) -> &'a Node {
        file.nodes
            .iter()
            .find(|n| n.kind == kind && n.name == name)
            .unwrap_or_else(|| panic!("missing {kind:?} {name}"))
    }

    #[test]
    fn extracts_rust_items_and_metadata() {
        let source = r#"
/// Handles requests.
pub async fn handler() {
    helper();
}

fn helper() {}
pub struct User {
    pub id: u64,
}
enum Status { Ready, Done }
trait Service { fn run(&self); }
const LIMIT: usize = 10;
static NAME: &str = "deep";
type Id = u64;
mod nested { pub fn inside() {} }
"#;
        let file = extract(source);

        let handler = node(&file, NodeKind::Function, "handler");
        assert_eq!(handler.visibility.as_deref(), Some("pub"));
        assert!(handler.is_async);
        assert_eq!(handler.docstring.as_deref(), Some("Handles requests."));
        assert!(handler
            .signature
            .as_ref()
            .unwrap()
            .contains("async fn handler"));

        assert_eq!(node(&file, NodeKind::Struct, "User").qualified_name, "User");
        assert_eq!(
            node(&file, NodeKind::Field, "id").qualified_name,
            "User::id"
        );
        assert_eq!(
            node(&file, NodeKind::EnumMember, "Ready").qualified_name,
            "Status::Ready"
        );
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Trait && n.name == "Service"));
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Constant && n.name == "LIMIT"));
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::TypeAlias && n.name == "Id"));
        assert_eq!(
            node(&file, NodeKind::Function, "inside").qualified_name,
            "nested::inside"
        );
    }

    #[test]
    fn extracts_imports_contains_calls_and_unresolved_refs() {
        let source = r#"
use crate::other::thing;

fn entry() {
    helper();
    missing();
}

fn helper() {}
"#;
        let file = extract(source);
        let entry = node(&file, NodeKind::Function, "entry");
        let helper = node(&file, NodeKind::Function, "helper");
        let import = node(&file, NodeKind::Import, "crate::other::thing");

        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::Contains && e.source == "file:src/lib.rs" && e.target == entry.id
        }));
        assert!(file
            .edges
            .iter()
            .any(|e| { e.kind == EdgeKind::Imports && e.target == import.id }));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::Calls && e.source == entry.id && e.target == helper.id
        }));
        assert!(file.unresolved_refs.iter().any(|r| {
            r.from_node_id == entry.id
                && r.reference_name == "missing"
                && r.reference_kind == "call"
        }));
    }

    #[test]
    fn extracts_impl_methods_and_trait_implements_edges() {
        let source = r#"
trait Service { fn run(&self); }
struct Worker;

impl Service for Worker {
    fn run(&self) {
        self.step();
    }

    fn step(&self) {}
}
"#;
        let file = extract(source);
        let service = node(&file, NodeKind::Trait, "Service");
        let run = node(&file, NodeKind::Method, "run");
        let step = node(&file, NodeKind::Method, "step");
        let impl_node = file
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Namespace && n.name.contains("Service for Worker"))
            .expect("impl namespace");

        assert!(run.qualified_name.contains("impl Service for Worker::run"));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::Contains && e.source == impl_node.id && e.target == run.id
        }));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::Implements && e.source == impl_node.id && e.target == service.id
        }));
        assert!(file
            .edges
            .iter()
            .any(|e| { e.kind == EdgeKind::Calls && e.source == run.id && e.target == step.id }));
    }

    #[test]
    fn extracts_rust_framework_routes() {
        let source = r#"
use axum::{routing::get, Router};

async fn list_users() {}

fn app() -> Router {
    Router::new().route("/users", get(list_users))
}

#[post("/users")]
async fn create_user() {}
"#;
        let file = extract(source);
        let list_users = node(&file, NodeKind::Function, "list_users");
        let create_user = node(&file, NodeKind::Function, "create_user");
        let get_route = node(&file, NodeKind::Route, "GET /users");
        let post_route = node(&file, NodeKind::Route, "POST /users");

        assert_eq!(get_route.signature.as_deref(), Some("GET /users"));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::References && e.source == get_route.id && e.target == list_users.id
        }));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::References
                && e.source == post_route.id
                && e.target == create_user.id
        }));
    }
}
