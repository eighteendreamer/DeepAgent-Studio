//! TypeScript / JavaScript symbol extraction.
//!
//! Covers the step-8 surface from the project-map plan: functions, arrow
//! functions assigned to variables, classes, interfaces, type aliases, ES
//! imports, CommonJS `require`, same-file calls, and class/interface
//! extends/implements edges.

use std::collections::HashMap;

use tree_sitter::Node as TsNode;

use super::{file_node_id, make_node_id, ExtractedFile, ExtractorImpl};
use crate::scanner::ScannedFile;
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind, UnresolvedRef};

/// Shared extractor for TypeScript and JavaScript.
#[derive(Debug)]
pub struct JsTsExtractor {
    language: Language,
}

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

#[derive(Debug)]
struct JsTsState {
    language: Language,
    file_path: String,
    file_id: String,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    calls: Vec<CallSite>,
}

impl JsTsExtractor {
    /// Construct an extractor for either [`Language::TypeScript`] or
    /// [`Language::JavaScript`].
    pub fn new(language: Language) -> Self {
        debug_assert!(matches!(
            language,
            Language::TypeScript | Language::JavaScript
        ));
        Self { language }
    }
}

impl ExtractorImpl for JsTsExtractor {
    fn extract(&self, tree: &tree_sitter::Tree, source: &str, file: &ScannedFile) -> ExtractedFile {
        let mut out = ExtractedFile::file_only(file, source);
        let file_path = posix_path(file);
        let mut state = JsTsState {
            language: self.language,
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
    state: &mut JsTsState,
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
    state: &mut JsTsState,
    containers: &mut Vec<Container>,
    current_callable: Option<&str>,
) {
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            visit_named_function(node, source, state, containers, NodeKind::Function)
        }
        "method_definition" | "method_signature" | "public_field_definition" => {
            visit_named_function(node, source, state, containers, NodeKind::Method)
        }
        "class_declaration" => {
            visit_container_item(node, source, state, containers, NodeKind::Class)
        }
        "interface_declaration" => {
            visit_container_item(node, source, state, containers, NodeKind::Interface)
        }
        "type_alias_declaration" => {
            visit_leaf_item(node, source, state, containers, NodeKind::TypeAlias)
        }
        "lexical_declaration" | "variable_declaration" => {
            visit_variable_declaration(node, source, state, containers, current_callable)
        }
        "import_statement" => visit_import_statement(node, source, state, containers),
        "call_expression" => {
            if let Some(from_node_id) = current_callable {
                if let Some(name) = call_name(node, source) {
                    if name == "require" {
                        if let Some(import_name) = require_arg(node, source) {
                            add_import_node(node, source, state, containers, import_name);
                        }
                    } else {
                        state.calls.push(CallSite {
                            from_node_id: from_node_id.to_string(),
                            reference_name: name,
                            line: line(node),
                            file_path: state.file_path.clone(),
                        });
                    }
                }
            }
            visit_children(node, source, state, containers, current_callable);
        }
        _ => visit_children(node, source, state, containers, current_callable),
    }
}

fn visit_named_function(
    node: TsNode<'_>,
    source: &str,
    state: &mut JsTsState,
    containers: &mut Vec<Container>,
    fallback_kind: NodeKind,
) {
    let Some(name) = name_of(node, source) else {
        visit_children(node, source, state, containers, None);
        return;
    };
    let in_class_or_interface = containers
        .last()
        .map(|c| matches!(c.kind, NodeKind::Class | NodeKind::Interface))
        .unwrap_or(false);
    let kind = if in_class_or_interface {
        NodeKind::Method
    } else {
        fallback_kind
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

    containers.push(Container {
        id: id.clone(),
        qualified_name: qualified,
        kind,
    });
    visit_children(node, source, state, containers, Some(&id));
    containers.pop();
}

fn visit_container_item(
    node: TsNode<'_>,
    source: &str,
    state: &mut JsTsState,
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
    add_heritage_edges(node, source, state, &id);

    containers.push(Container {
        id,
        qualified_name: qualified,
        kind,
    });
    visit_children(node, source, state, containers, None);
    containers.pop();
}

fn visit_leaf_item(
    node: TsNode<'_>,
    source: &str,
    state: &mut JsTsState,
    containers: &[Container],
    kind: NodeKind,
) {
    let Some(name) = name_of(node, source) else {
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
}

fn visit_variable_declaration(
    node: TsNode<'_>,
    source: &str,
    state: &mut JsTsState,
    containers: &mut Vec<Container>,
    current_callable: Option<&str>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            visit_variable_declarator(child, source, state, containers);
        } else {
            visit_node(child, source, state, containers, current_callable);
        }
    }
}

fn visit_variable_declarator(
    node: TsNode<'_>,
    source: &str,
    state: &mut JsTsState,
    containers: &mut Vec<Container>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        visit_children(node, source, state, containers, None);
        return;
    };
    let name = text(name_node, source).to_string();
    let value = node.child_by_field_name("value");
    let is_function = value
        .map(|v| {
            matches!(
                v.kind(),
                "arrow_function" | "function" | "function_expression"
            )
        })
        .unwrap_or(false);

    if let Some(value) = value {
        if value.kind() == "call_expression"
            && call_name(value, source).as_deref() == Some("require")
        {
            if let Some(import_name) = require_arg(value, source) {
                add_import_node(node, source, state, containers, import_name);
                return;
            }
        }
    }

    let kind = if is_function {
        NodeKind::Function
    } else {
        NodeKind::Variable
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

    if is_function {
        containers.push(Container {
            id: id.clone(),
            qualified_name: qualified,
            kind,
        });
        visit_children(node, source, state, containers, Some(&id));
        containers.pop();
    } else {
        visit_children(node, source, state, containers, None);
    }
}

fn visit_import_statement(
    node: TsNode<'_>,
    source: &str,
    state: &mut JsTsState,
    containers: &[Container],
) {
    let import_name = import_source(node, source).unwrap_or_else(|| compact_ws(text(node, source)));
    add_import_node(node, source, state, containers, import_name);
}

fn add_import_node(
    node: TsNode<'_>,
    source: &str,
    state: &mut JsTsState,
    containers: &[Container],
    import_name: String,
) {
    let qualified = qualify(containers, &format!("import {import_name}"));
    let id = make_node_id(NodeKind::Import, &state.file_path, &qualified, line(node));
    if state.nodes.iter().any(|n| n.id == id) {
        return;
    }
    let import = build_node(
        node,
        source,
        state,
        NodeKind::Import,
        &import_name,
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

fn add_heritage_edges(node: TsNode<'_>, source: &str, state: &mut JsTsState, from_id: &str) {
    for heritage in descendant_kinds(
        node,
        &["class_heritage", "extends_clause", "implements_clause"],
    ) {
        let edge_kind = if text(heritage, source).contains("implements") {
            EdgeKind::Implements
        } else {
            EdgeKind::Extends
        };
        for name in identifiers_in(heritage, source) {
            if let Some(target) = find_node_by_name(state, &name, None) {
                state.edges.push(edge(
                    from_id.to_string(),
                    target.id.clone(),
                    edge_kind,
                    Some(line(heritage)),
                ));
            }
        }
    }
}

fn build_node(
    node: TsNode<'_>,
    source: &str,
    state: &JsTsState,
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
        language: state.language,
        start_line: line(node),
        end_line: node.end_position().row as u32 + 1,
        start_column: node.start_position().column as u32,
        end_column: node.end_position().column as u32,
        signature: Some(signature.clone()),
        docstring: docstring_before(node, source),
        visibility: visibility(node, source),
        is_exported: is_exported(node, source),
        is_async: signature.contains("async "),
    }
}

fn resolve_calls(state: &mut JsTsState) {
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

fn detect_routes(source: &str, state: &mut JsTsState) {
    for (index, raw_line) in source.lines().enumerate() {
        let line_no = index as u32 + 1;
        let line = raw_line.trim();
        if let Some((method, path, handler)) = parse_express_method_route(line) {
            add_route(state, &method, &path, Some(&handler), line_no);
        }
        if let Some((method, path, handler)) = parse_express_chained_route(line) {
            add_route(state, &method, &path, Some(&handler), line_no);
        }
    }
}

fn add_route(state: &mut JsTsState, method: &str, path: &str, handler: Option<&str>, line: u32) {
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
        language: state.language,
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
    if let Some(handler) = handler.and_then(normalize_handler_name) {
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

fn parse_express_method_route(line: &str) -> Option<(String, String, String)> {
    for method in [
        "get", "post", "put", "patch", "delete", "head", "options", "all",
    ] {
        for receiver in ["app", "router"] {
            let marker = format!("{receiver}.{method}(");
            let Some(start) = line.find(&marker) else {
                continue;
            };
            let after = &line[start + marker.len()..];
            let (path, rest) = parse_first_string_arg(after)?;
            let handler = rest
                .trim_start()
                .strip_prefix(',')?
                .split([',', ')'])
                .next()?
                .trim()
                .to_string();
            if !handler.is_empty() {
                return Some((method.to_string(), path, handler));
            }
        }
    }
    None
}

fn parse_express_chained_route(line: &str) -> Option<(String, String, String)> {
    let route_start = line.find(".route(")?;
    let after_route = &line[route_start + ".route(".len()..];
    let (path, rest) = parse_first_string_arg(after_route)?;
    let after_route_call = rest.split_once(')')?.1;
    for method in [
        "get", "post", "put", "patch", "delete", "head", "options", "all",
    ] {
        let marker = format!(".{method}(");
        let Some(start) = after_route_call.find(&marker) else {
            continue;
        };
        let handler = after_route_call[start + marker.len()..]
            .split([',', ')'])
            .next()?
            .trim()
            .to_string();
        if !handler.is_empty() {
            return Some((method.to_string(), path, handler));
        }
    }
    None
}

fn parse_first_string_arg(s: &str) -> Option<(String, &str)> {
    let trimmed = s.trim_start();
    let quote = trimmed.chars().next()?;
    if quote != '"' && quote != '\'' && quote != '`' {
        return None;
    }
    let rest = &trimmed[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some((rest[..end].to_string(), &rest[end + quote.len_utf8()..]))
}

fn normalize_handler_name(handler: &str) -> Option<&str> {
    let handler = handler
        .trim()
        .trim_start_matches("async ")
        .trim_start_matches("function ")
        .trim_start_matches('&');
    let simple = handler.rsplit('.').next().unwrap_or(handler).trim();
    if simple.is_empty()
        || simple.starts_with('(')
        || simple.starts_with("req")
        || simple.contains("=>")
    {
        None
    } else {
        Some(simple)
    }
}

fn add_contains_edge(state: &mut JsTsState, parent: Option<&str>, child: &str, line: u32) {
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
    if let Some(name) = node.child_by_field_name("name") {
        return Some(text(name, source).to_string());
    }
    let mut cursor = node.walk();
    let result = node
        .children(&mut cursor)
        .find(|child| {
            matches!(
                child.kind(),
                "identifier" | "property_identifier" | "type_identifier"
            )
        })
        .map(|child| text(child, source).to_string());
    result
}

fn call_name(node: TsNode<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("function")
        .map(|child| callable_text(child, source))
        .filter(|name| !name.is_empty())
}

fn callable_text(node: TsNode<'_>, source: &str) -> String {
    match node.kind() {
        "identifier" | "property_identifier" | "member_expression" => {
            if node.kind() == "member_expression" {
                node.child_by_field_name("property")
                    .map(|field| text(field, source).to_string())
                    .unwrap_or_else(|| compact_ws(text(node, source)))
            } else {
                text(node, source).to_string()
            }
        }
        _ => compact_ws(text(node, source)),
    }
}

fn import_source(node: TsNode<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string" {
            return Some(unquote(text(child, source)));
        }
    }
    None
}

fn require_arg(node: TsNode<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "arguments" {
            let mut arg_cursor = child.walk();
            for arg in child.children(&mut arg_cursor) {
                if arg.kind() == "string" {
                    return Some(unquote(text(arg, source)));
                }
            }
        }
    }
    None
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
    if matches!(node.kind(), "identifier" | "type_identifier") {
        let value = text(node, source).to_string();
        if !matches!(value.as_str(), "extends" | "implements") {
            out.push(value);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifiers(child, source, out);
    }
}

fn find_node_by_name<'a>(
    state: &'a JsTsState,
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
    let before_body = raw.split('{').next().unwrap_or(raw);
    compact_ws(before_body.trim().trim_end_matches(';'))
}

fn docstring_before(node: TsNode<'_>, source: &str) -> Option<String> {
    let prefix = &source[..node.start_byte().min(source.len())];
    let mut docs = Vec::new();
    for line in prefix.lines().rev() {
        let trimmed = line.trim();
        if let Some(doc) = trimmed.strip_prefix("/**") {
            docs.push(doc.trim_end_matches("*/").trim().to_string());
            continue;
        }
        if let Some(doc) = trimmed.strip_prefix('*') {
            docs.push(doc.trim().to_string());
            continue;
        }
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

fn visibility(node: TsNode<'_>, source: &str) -> Option<String> {
    let sig = signature_text(node, source);
    for modifier in ["public", "private", "protected"] {
        if sig.split_whitespace().any(|part| part == modifier) {
            return Some(modifier.to_string());
        }
    }
    None
}

fn is_exported(node: TsNode<'_>, source: &str) -> bool {
    if signature_text(node, source)
        .split_whitespace()
        .any(|part| part == "export" || part == "export default")
    {
        return true;
    }

    let mut parent = node.parent();
    while let Some(p) = parent {
        if p.kind() == "export_statement" {
            return true;
        }
        if p.kind() == "program" {
            break;
        }
        parent = p.parent();
    }
    false
}

fn unquote(s: &str) -> String {
    s.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('`')
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

    fn scanned(rel: &str, language: Language) -> ScannedFile {
        ScannedFile {
            path: PathBuf::from(format!("/abs/{rel}")),
            relative_path: PathBuf::from(rel),
            language,
            size: 0,
            content_hash: "hash".to_string(),
        }
    }

    fn extract(source: &str, language: Language) -> ExtractedFile {
        Extractor::new()
            .extract(&scanned("src/app.ts", language), source)
            .unwrap()
    }

    fn node<'a>(file: &'a ExtractedFile, kind: NodeKind, name: &str) -> &'a Node {
        file.nodes
            .iter()
            .find(|n| n.kind == kind && n.name == name)
            .unwrap_or_else(|| panic!("missing {kind:?} {name}"))
    }

    fn qualified_node<'a>(
        file: &'a ExtractedFile,
        kind: NodeKind,
        qualified_name: &str,
    ) -> &'a Node {
        file.nodes
            .iter()
            .find(|n| n.kind == kind && n.qualified_name == qualified_name)
            .unwrap_or_else(|| panic!("missing {kind:?} {qualified_name}"))
    }

    #[test]
    fn extracts_typescript_symbols_imports_and_calls() {
        let source = r#"
import { api } from "./api";
const fs = require("fs");

export interface Runner { run(): void }
type Id = string;

class Base {}
export class Worker extends Base implements Runner {
  public run() {
    helper();
    missing();
  }
}

function helper() {}
const arrow = async () => helper();
"#;
        let file = extract(source, Language::TypeScript);
        let worker = node(&file, NodeKind::Class, "Worker");
        let base = node(&file, NodeKind::Class, "Base");
        let runner = node(&file, NodeKind::Interface, "Runner");
        let run = qualified_node(&file, NodeKind::Method, "Worker::run");
        let helper = node(&file, NodeKind::Function, "helper");
        let arrow = node(&file, NodeKind::Function, "arrow");

        assert_eq!(worker.language, Language::TypeScript);
        assert!(worker.is_exported);
        assert_eq!(run.qualified_name, "Worker::run");
        assert_eq!(run.visibility.as_deref(), Some("public"));
        assert!(arrow.is_async);
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::TypeAlias && n.name == "Id"));
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Import && n.name == "./api"));
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Import && n.name == "fs"));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::Extends && e.source == worker.id && e.target == base.id
        }));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::Implements && e.source == worker.id && e.target == runner.id
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
    fn extracts_javascript_commonjs_and_methods() {
        let source = r#"
const path = require("path");
class App {
  start() {
    boot();
  }
}
function boot() {}
module.exports = App;
"#;
        let file = extract(source, Language::JavaScript);
        let app = node(&file, NodeKind::Class, "App");
        let start = node(&file, NodeKind::Method, "start");
        let boot = node(&file, NodeKind::Function, "boot");

        assert_eq!(app.language, Language::JavaScript);
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Import && n.name == "path"));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::Contains && e.source == app.id && e.target == start.id
        }));
        assert!(file
            .edges
            .iter()
            .any(|e| { e.kind == EdgeKind::Calls && e.source == start.id && e.target == boot.id }));
    }

    #[test]
    fn extracts_express_routes() {
        let source = r#"
function listUsers(req, res) {}
const createUser = (req, res) => {};

app.get("/users", listUsers);
router.post('/users', createUser);
"#;
        let file = extract(source, Language::TypeScript);
        let list_users = node(&file, NodeKind::Function, "listUsers");
        let create_user = node(&file, NodeKind::Function, "createUser");
        let get_route = node(&file, NodeKind::Route, "GET /users");
        let post_route = node(&file, NodeKind::Route, "POST /users");

        assert_eq!(get_route.language, Language::TypeScript);
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
