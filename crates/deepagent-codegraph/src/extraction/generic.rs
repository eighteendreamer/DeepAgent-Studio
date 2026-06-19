//! Heuristic extractor for languages without a compiled tree-sitter grammar.
//!
//! This keeps newly recognised languages useful even before their dedicated
//! grammar-backed extractor is available: files get imports, classes/modules,
//! functions/methods, and simple same-file call edges instead of only a file
//! node.

use std::collections::HashMap;

use super::{file_node_id, make_node_id, ExtractedFile, ExtractorImpl};
use crate::scanner::ScannedFile;
use crate::types::{Edge, EdgeKind, Language, Node, NodeKind, UnresolvedRef};

/// Lightweight line-oriented extractor for broad language coverage.
#[derive(Debug, Clone, Copy)]
pub struct GenericExtractor {
    language: Language,
}

impl GenericExtractor {
    pub fn new(language: Language) -> Self {
        Self { language }
    }

    fn extract_source_impl(&self, source: &str, file: &ScannedFile) -> ExtractedFile {
        let mut out = ExtractedFile::file_only(file, source);
        let file_path = posix_path(file);
        let file_id = file_node_id(&file_path);
        let mut callable_by_name: HashMap<String, String> = HashMap::new();
        let mut current_callable: Option<String> = None;
        let mut pending_calls: Vec<(String, String, u32)> = Vec::new();

        for (index, raw_line) in source.lines().enumerate() {
            let line_no = index as u32 + 1;
            let line = strip_comments(raw_line, self.language).trim().to_string();
            if line.is_empty() {
                continue;
            }

            if let Some(import_name) = parse_import(&line, self.language) {
                let id = make_node_id(NodeKind::Import, &file_path, &import_name, line_no);
                if !out.nodes.iter().any(|node| node.id == id) {
                    out.nodes.push(node(
                        id.clone(),
                        NodeKind::Import,
                        &import_name,
                        &file_path,
                        self.language,
                        line_no,
                        Some(line.clone()),
                    ));
                    out.edges
                        .push(edge(file_id.clone(), id, EdgeKind::Imports, Some(line_no)));
                }
            }

            if let Some((kind, name)) = parse_container(&line, self.language) {
                let id = make_node_id(kind, &file_path, &name, line_no);
                if !out.nodes.iter().any(|node| node.id == id) {
                    out.nodes.push(node(
                        id.clone(),
                        kind,
                        &name,
                        &file_path,
                        self.language,
                        line_no,
                        Some(line.clone()),
                    ));
                    out.edges
                        .push(edge(file_id.clone(), id, EdgeKind::Contains, Some(line_no)));
                }
            }

            if let Some(name) = parse_function(&line, self.language) {
                let id = make_node_id(NodeKind::Function, &file_path, &name, line_no);
                if !out.nodes.iter().any(|node| node.id == id) {
                    out.nodes.push(node(
                        id.clone(),
                        NodeKind::Function,
                        &name,
                        &file_path,
                        self.language,
                        line_no,
                        Some(line.clone()),
                    ));
                    out.edges.push(edge(
                        file_id.clone(),
                        id.clone(),
                        EdgeKind::Contains,
                        Some(line_no),
                    ));
                    callable_by_name.insert(name, id.clone());
                }
                current_callable = Some(id);
            }

            if let Some(from) = &current_callable {
                for call in parse_calls(&line) {
                    pending_calls.push((from.clone(), call, line_no));
                }
            }
        }

        for (from, call, line) in pending_calls {
            if let Some(target) = callable_by_name.get(&call) {
                if &from != target {
                    out.edges
                        .push(edge(from, target.clone(), EdgeKind::Calls, Some(line)));
                }
            } else {
                out.unresolved_refs.push(UnresolvedRef {
                    from_node_id: from,
                    reference_name: call,
                    reference_kind: "call".to_string(),
                    line,
                    file_path: file_path.clone(),
                });
            }
        }

        out
    }
}

impl ExtractorImpl for GenericExtractor {
    fn extract(
        &self,
        _tree: &tree_sitter::Tree,
        source: &str,
        file: &ScannedFile,
    ) -> ExtractedFile {
        self.extract_source_impl(source, file)
    }

    fn extract_source(&self, source: &str, file: &ScannedFile) -> ExtractedFile {
        self.extract_source_impl(source, file)
    }
}

fn node(
    id: String,
    kind: NodeKind,
    name: &str,
    file_path: &str,
    language: Language,
    line: u32,
    signature: Option<String>,
) -> Node {
    Node {
        id,
        kind,
        name: name.to_string(),
        qualified_name: name.to_string(),
        file_path: file_path.to_string(),
        language,
        start_line: line,
        end_line: line,
        start_column: 0,
        end_column: 0,
        signature,
        docstring: None,
        visibility: None,
        is_exported: false,
        is_async: false,
    }
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

fn parse_import(line: &str, language: Language) -> Option<String> {
    let trimmed = line.trim().trim_end_matches(';').trim();
    match language {
        Language::Java
        | Language::Kotlin
        | Language::Scala
        | Language::Swift
        | Language::Dart
        | Language::Haskell
        | Language::Julia => trimmed
            .strip_prefix("import ")
            .map(|s| clean_token(s).to_string()),
        Language::C | Language::Cpp => trimmed
            .strip_prefix("#include ")
            .map(|s| s.trim_matches(['<', '>', '"']).to_string()),
        Language::CSharp => trimmed
            .strip_prefix("using ")
            .map(|s| clean_token(s).to_string()),
        Language::Ruby => trimmed
            .strip_prefix("require ")
            .map(|s| s.trim_matches(['"', '\'']).to_string()),
        Language::Php => trimmed
            .strip_prefix("use ")
            .map(|s| clean_token(s).to_string()),
        Language::Elixir => trimmed
            .strip_prefix("alias ")
            .or_else(|| trimmed.strip_prefix("import "))
            .map(|s| clean_token(s).to_string()),
        Language::Lua => trimmed
            .strip_prefix("require")
            .and_then(|s| first_quoted(s).map(|q| q.to_string())),
        Language::Shell => trimmed
            .strip_prefix("source ")
            .or_else(|| trimmed.strip_prefix(". "))
            .map(|s| clean_token(s).to_string()),
        Language::Sql => trimmed
            .strip_prefix("FROM ")
            .or_else(|| trimmed.strip_prefix("from "))
            .map(|s| clean_token(s).to_string()),
        Language::Css => trimmed
            .strip_prefix("@import ")
            .map(|s| s.trim_matches(['"', '\'', ';']).to_string()),
        Language::Html | Language::Xml | Language::Vue | Language::Svelte => {
            attr_value(trimmed, "src").or_else(|| attr_value(trimmed, "href"))
        }
        _ => None,
    }
}

fn parse_container(line: &str, language: Language) -> Option<(NodeKind, String)> {
    match language {
        Language::Php => after_keyword(line, "class ")
            .or_else(|| after_keyword(line, "interface "))
            .map(|name| (NodeKind::Class, name)),
        Language::Elixir => after_keyword(line, "defmodule ").map(|name| (NodeKind::Module, name)),
        Language::Lua => None,
        Language::Haskell => after_keyword(line, "module ").map(|name| (NodeKind::Module, name)),
        Language::R | Language::Shell | Language::Sql | Language::Css => None,
        Language::Html | Language::Xml | Language::Vue | Language::Svelte => {
            element_name(line).map(|name| (NodeKind::Module, name))
        }
        _ => after_keyword(line, "class ")
            .map(|name| (NodeKind::Class, name))
            .or_else(|| after_keyword(line, "interface ").map(|name| (NodeKind::Interface, name)))
            .or_else(|| after_keyword(line, "struct ").map(|name| (NodeKind::Struct, name)))
            .or_else(|| after_keyword(line, "enum ").map(|name| (NodeKind::Enum, name)))
            .or_else(|| after_keyword(line, "trait ").map(|name| (NodeKind::Trait, name))),
    }
}

fn parse_function(line: &str, language: Language) -> Option<String> {
    match language {
        Language::Ruby => after_keyword(line, "def "),
        Language::Php => after_keyword(line, "function "),
        Language::Swift => after_keyword(line, "func "),
        Language::Kotlin => after_keyword(line, "fun "),
        Language::Elixir => after_keyword(line, "def "),
        Language::Lua => after_keyword(line, "function "),
        Language::Julia => after_keyword(line, "function "),
        Language::Shell => shell_function_name(line),
        Language::Sql => sql_routine_name(line),
        Language::Css | Language::Html | Language::Xml | Language::Vue | Language::Svelte => None,
        Language::Haskell => haskell_function_name(line),
        Language::R => r_function_name(line),
        _ => c_family_function_name(line),
    }
}

fn parse_calls(line: &str) -> Vec<String> {
    let mut calls = Vec::new();
    let bytes = line.as_bytes();
    for (idx, byte) in bytes.iter().enumerate() {
        if *byte != b'(' || idx == 0 {
            continue;
        }
        let before = &line[..idx];
        let name = before
            .rsplit(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '$'))
            .next()
            .unwrap_or("")
            .trim();
        if !name.is_empty() && !is_control_keyword(name) {
            calls.push(name.trim_start_matches('$').to_string());
        }
    }
    calls
}

fn c_family_function_name(line: &str) -> Option<String> {
    if !line.contains('(') || line.starts_with('#') || line.ends_with(';') {
        return None;
    }
    let before = line.split('(').next()?.trim();
    let name = before
        .split_whitespace()
        .last()?
        .trim_start_matches('*')
        .trim_start_matches('&');
    if is_control_keyword(name) || name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn shell_function_name(line: &str) -> Option<String> {
    line.strip_suffix("()")
        .map(str::trim)
        .or_else(|| line.strip_suffix("() {").map(str::trim))
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
}

fn sql_routine_name(line: &str) -> Option<String> {
    let upper = line.to_ascii_uppercase();
    if upper.starts_with("CREATE FUNCTION ") {
        after_keyword(line, "CREATE FUNCTION ")
    } else if upper.starts_with("CREATE PROCEDURE ") {
        after_keyword(line, "CREATE PROCEDURE ")
    } else {
        None
    }
}

fn haskell_function_name(line: &str) -> Option<String> {
    if line.starts_with(' ') || !line.contains('=') {
        return None;
    }
    let name = line.split('=').next()?.split_whitespace().next()?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn r_function_name(line: &str) -> Option<String> {
    let (name, rest) = line.split_once("<-")?;
    if rest.trim_start().starts_with("function(") {
        Some(name.trim().to_string())
    } else {
        None
    }
}

fn after_keyword(line: &str, keyword: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix(keyword)?;
    let name = rest
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '$' || ch == ':'))
        .next()?
        .trim_matches(':')
        .trim_start_matches('$');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn clean_token(s: &str) -> &str {
    s.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(['"', '\'', ';'])
}

fn first_quoted(s: &str) -> Option<&str> {
    let quote = s.find(['"', '\''])?;
    let rest = &s[quote + 1..];
    let end = rest.find(['"', '\''])?;
    Some(&rest[..end])
}

fn attr_value(line: &str, attr: &str) -> Option<String> {
    let marker = format!("{attr}=");
    let start = line.find(&marker)? + marker.len();
    let rest = &line[start..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &rest[quote.len_utf8()..];
    let end = value.find(quote)?;
    Some(value[..end].to_string())
}

fn element_name(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix('<')?;
    if rest.starts_with('/') || rest.starts_with('!') || rest.starts_with('?') {
        return None;
    }
    let name = rest
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '-' || ch == '_'))
        .next()?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn strip_comments(line: &str, language: Language) -> &str {
    match language {
        Language::Ruby | Language::Shell | Language::R => line.split('#').next().unwrap_or(line),
        Language::Sql => line.split("--").next().unwrap_or(line),
        Language::Html | Language::Xml | Language::Vue | Language::Svelte => line,
        _ => line.split("//").next().unwrap_or(line),
    }
}

fn is_control_keyword(name: &str) -> bool {
    matches!(
        name,
        "if" | "for"
            | "while"
            | "switch"
            | "catch"
            | "return"
            | "sizeof"
            | "new"
            | "super"
            | "this"
            | "function"
            | "fn"
            | "def"
    )
}

fn posix_path(file: &ScannedFile) -> String {
    file.relative_path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn extracts_java_like_symbols_imports_and_calls() {
        let extractor = GenericExtractor::new(Language::Java);
        let source = r#"
import java.util.List;
class UserService {
  void listUsers() {
    helper();
  }
  void helper() {}
}
"#;

        let file =
            extractor.extract_source(source, &scanned("src/UserService.java", Language::Java));

        let list = file
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Function && n.name == "listUsers")
            .expect("function");
        let helper = file
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Function && n.name == "helper")
            .expect("helper");
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Class && n.name == "UserService"));
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Import && n.name == "java.util.List"));
        assert!(file.edges.iter().any(|e| {
            e.kind == EdgeKind::Calls && e.source == list.id && e.target == helper.id
        }));
    }

    #[test]
    fn extracts_ruby_symbols_imports_and_calls() {
        let extractor = GenericExtractor::new(Language::Ruby);
        let source = r#"
require "json"
class Worker
  def run
    helper()
  end
  def helper
  end
end
"#;

        let file = extractor.extract_source(source, &scanned("lib/worker.rb", Language::Ruby));

        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Class && n.name == "Worker"));
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Function && n.name == "run"));
        assert!(file
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Import && n.name == "json"));
    }
}
