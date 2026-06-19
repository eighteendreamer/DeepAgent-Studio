//! Extraction layer: tree-sitter AST parsing, route detection, and broad
//! source-symbol extraction.
//!
//! This module provides the extraction *framework*:
//!
//! - [`language::ts_language`] registers the compiled-in tree-sitter grammars
//!   for Rust / TypeScript / JavaScript / Python / Go.
//! - [`Extractor`] owns parsing: it picks the right grammar for a file, parses
//!   the source into a [`tree_sitter::Tree`], and delegates symbol/edge
//!   extraction to a per-language [`ExtractorImpl`].
//! - [`generic::GenericExtractor`] handles recognised languages that do not yet
//!   have a compiled grammar by using conservative line-oriented heuristics.
//! - [`ExtractedFile`] is the per-file result (nodes + edges + unresolved
//!   references + line count).
//! - [`make_node_id`] / [`file_node_id`] implement the stable id scheme.
//!
//! Every file — even one in an unsupported language or one that fails to
//! parse — yields at least a single `file` [`Node`], so the project map always
//! lists the file. The actual per-language symbol extractors are placeholders
//! here; they are filled in by later tasks (Rust / TS+JS / Python+Go).

pub mod generic;
pub mod go;
pub mod language;
pub mod python;
pub mod rust;
pub mod typescript;

use std::collections::HashMap;
use std::sync::Mutex;

use deepagent_core::error::Result;

use crate::scanner::ScannedFile;
use crate::types::{Edge, Language, Node, NodeKind, UnresolvedRef};

use generic::GenericExtractor;
use go::GoExtractor;
use python::PythonExtractor;
use rust::RustExtractor;
use typescript::JsTsExtractor;

pub use language::ts_language;

/// The result of extracting a single file: its nodes, edges, parked
/// references, and a line count.
///
/// `nodes` always contains at least the file's own `file` node (see
/// [`ExtractedFile::file_only`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedFile {
    /// The file that was extracted (path, language, hash, size).
    pub file_info: ScannedFile,
    /// Symbol nodes, including the file node itself.
    pub nodes: Vec<Node>,
    /// Relationship edges between nodes.
    pub edges: Vec<Edge>,
    /// References that could not be resolved within this file, parked for the
    /// cross-file [`crate::resolution`] step.
    pub unresolved_refs: Vec<UnresolvedRef>,
    /// Total number of source lines in the file.
    pub total_lines: u32,
}

impl ExtractedFile {
    /// Build a minimal [`ExtractedFile`] containing only the file node.
    ///
    /// Used for unsupported languages, parse failures, and as the starting
    /// point for per-language extractors (which append their symbols).
    pub fn file_only(file: &ScannedFile, source: &str) -> Self {
        let total_lines = count_lines(source);
        let node = file_node(file, total_lines);
        ExtractedFile {
            file_info: file.clone(),
            nodes: vec![node],
            edges: Vec::new(),
            unresolved_refs: Vec::new(),
            total_lines,
        }
    }
}

/// Per-language extraction strategy.
///
/// Given a successfully parsed [`tree_sitter::Tree`] and its source, an
/// implementation walks the AST and produces the file's nodes, edges, and
/// unresolved references. Implementations MUST include the file node (the
/// simplest way is to start from [`ExtractedFile::file_only`] and push to it).
///
/// Implementations are stored behind a shared reference and may be used from
/// multiple threads, hence the `Send + Sync` bound.
pub trait ExtractorImpl: Send + Sync {
    /// Extract nodes/edges/refs from a parsed tree.
    fn extract(&self, tree: &tree_sitter::Tree, source: &str, file: &ScannedFile) -> ExtractedFile;

    /// Extract nodes/edges/refs directly from source when no tree-sitter
    /// grammar is available for the language.
    fn extract_source(&self, source: &str, file: &ScannedFile) -> ExtractedFile {
        ExtractedFile::file_only(file, source)
    }
}

/// imports, calls, …) is implemented per language in later tasks; this keeps
/// Drives tree-sitter parsing and per-language extraction.
///
/// Holds one cached [`tree_sitter::Parser`] per language (guarded by a
/// [`Mutex`] because `Parser` is not `Sync`) and a registry of per-language
/// [`ExtractorImpl`]s.
pub struct Extractor {
    /// Per-language parser cache. A `Parser` is reused across files of the same
    /// language; the mutex serialises access since `Parser` is `!Sync`.
    parsers: Mutex<HashMap<Language, tree_sitter::Parser>>,
    /// Per-language extraction strategies.
    extractors: HashMap<Language, Box<dyn ExtractorImpl>>,
}

impl std::fmt::Debug for Extractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extractor")
            .field("languages", &self.extractors.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for Extractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Extractor {
    /// Construct an extractor with the per-language strategies registered.
    pub fn new() -> Self {
        let mut extractors: HashMap<Language, Box<dyn ExtractorImpl>> = HashMap::new();
        extractors.insert(Language::Rust, Box::new(RustExtractor));
        extractors.insert(
            Language::TypeScript,
            Box::new(JsTsExtractor::new(Language::TypeScript)),
        );
        extractors.insert(
            Language::JavaScript,
            Box::new(JsTsExtractor::new(Language::JavaScript)),
        );
        extractors.insert(Language::Python, Box::new(PythonExtractor));
        extractors.insert(Language::Go, Box::new(GoExtractor));
        for &language in GENERIC_LANGUAGES {
            extractors.insert(language, Box::new(GenericExtractor::new(language)));
        }
        Extractor {
            parsers: Mutex::new(HashMap::new()),
            extractors,
        }
    }

    /// Extract a single file.
    ///
    /// Behaviour:
    /// - Unsupported language ([`Language::Other`] / no grammar): returns an
    ///   [`ExtractedFile`] with just the file node (no parsing).
    /// - Parse failure (parser returns `None`, the grammar cannot be set, or
    ///   the resulting tree has an error root): logs a warning and returns the
    ///   file-only result — a single bad file never aborts the run.
    /// - Success: delegates to the language's [`ExtractorImpl`].
    ///
    /// In all cases the result contains at least the file node.
    pub fn extract(&self, file: &ScannedFile, source: &str) -> Result<ExtractedFile> {
        let Some(grammar) = ts_language(file.language) else {
            return Ok(self
                .extractors
                .get(&file.language)
                .map(|extractor| extractor.extract_source(source, file))
                .unwrap_or_else(|| ExtractedFile::file_only(file, source)));
        };

        let tree = match self.parse(file.language, &grammar, source) {
            Some(tree) if !tree.root_node().has_error() => tree,
            Some(_) => {
                tracing::warn!(
                    path = %file.relative_path.display(),
                    language = file.language.as_str(),
                    "tree-sitter parse produced errors; registering file node only",
                );
                return Ok(ExtractedFile::file_only(file, source));
            }
            None => {
                tracing::warn!(
                    path = %file.relative_path.display(),
                    language = file.language.as_str(),
                    "tree-sitter parse failed; registering file node only",
                );
                return Ok(ExtractedFile::file_only(file, source));
            }
        };

        match self.extractors.get(&file.language) {
            Some(extractor) => Ok(extractor.extract(&tree, source, file)),
            // A supported grammar without a registered strategy still yields
            // the file node rather than nothing.
            None => Ok(ExtractedFile::file_only(file, source)),
        }
    }

    /// Parse `source` with the cached parser for `language`, configuring it
    /// with `grammar` on first use. Returns `None` if the grammar cannot be
    /// set or parsing fails.
    fn parse(
        &self,
        language: Language,
        grammar: &tree_sitter::Language,
        source: &str,
    ) -> Option<tree_sitter::Tree> {
        // A poisoned lock means a previous extraction panicked while holding
        // it; recover the guard rather than propagating the panic.
        let mut parsers = self
            .parsers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let parser = match parsers.get_mut(&language) {
            Some(parser) => parser,
            None => {
                let mut parser = tree_sitter::Parser::new();
                if parser.set_language(grammar).is_err() {
                    return None;
                }
                parsers.entry(language).or_insert(parser)
            }
        };

        parser.parse(source, None)
    }
}

const GENERIC_LANGUAGES: &[Language] = &[
    Language::Java,
    Language::CSharp,
    Language::C,
    Language::Cpp,
    Language::Ruby,
    Language::Php,
    Language::Swift,
    Language::Kotlin,
    Language::Scala,
    Language::Dart,
    Language::Elixir,
    Language::Lua,
    Language::Haskell,
    Language::R,
    Language::Julia,
    Language::Shell,
    Language::Sql,
    Language::Css,
    Language::Html,
    Language::Xml,
    Language::Vue,
    Language::Svelte,
];

/// Count the number of source lines in `source`.
fn count_lines(source: &str) -> u32 {
    source.lines().count() as u32
}

/// Build the `file` node for a scanned file.
fn file_node(file: &ScannedFile, total_lines: u32) -> Node {
    let path = posix_path(file);
    let name = file
        .relative_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&path)
        .to_string();

    Node {
        id: file_node_id(&path),
        kind: NodeKind::File,
        name,
        qualified_name: path.clone(),
        file_path: path,
        language: file.language,
        start_line: 1,
        // Keep the invariant start_line <= end_line even for empty files.
        end_line: total_lines.max(1),
        start_column: 0,
        end_column: 0,
        signature: None,
        docstring: None,
        visibility: None,
        is_exported: false,
        is_async: false,
    }
}

/// The POSIX-style relative path string for a scanned file.
fn posix_path(file: &ScannedFile) -> String {
    file.relative_path.to_string_lossy().replace('\\', "/")
}

/// Stable id for a `file` node: `file:{relative_path}`.
pub fn file_node_id(relative_path: &str) -> String {
    format!("file:{relative_path}")
}

/// Build a stable node id of the form `{kind}:{file}:{qualified_name}:{start_line}`.
///
/// The same symbol (same kind, file, qualified name, and start line) always
/// produces the same id across extractions, which is what makes incremental
/// re-indexing and cross-run edge references stable.
///
/// For `file` nodes use [`file_node_id`] instead, which omits the empty
/// qualified-name / line components.
pub fn make_node_id(kind: NodeKind, file: &str, qualified_name: &str, start_line: u32) -> String {
    format!(
        "{}:{}:{}:{}",
        kind.as_str(),
        file,
        qualified_name,
        start_line
    )
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
    fn extracts_rust_source_symbols() {
        let extractor = Extractor::new();
        let file = scanned("src/main.rs", Language::Rust);
        let source = "fn main() {\n    println!(\"hi\");\n}\n";

        let extracted = extractor.extract(&file, source).unwrap();

        assert_eq!(extracted.file_info, file);
        assert_eq!(extracted.total_lines, 3);
        let file_node = extracted
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::File)
            .expect("file node present");
        assert_eq!(file_node.id, "file:src/main.rs");
        assert_eq!(file_node.language, Language::Rust);
        let main = extracted
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Function && n.name == "main")
            .expect("main function extracted");
        assert_eq!(main.qualified_name, "main");
    }

    #[test]
    fn other_language_returns_only_file_node() {
        let extractor = Extractor::new();
        let file = scanned("README.md", Language::Other);
        let source = "# Title\n\nSome prose.\n";

        let extracted = extractor.extract(&file, source).unwrap();

        assert_eq!(extracted.nodes.len(), 1);
        assert_eq!(extracted.nodes[0].kind, NodeKind::File);
        assert_eq!(extracted.nodes[0].id, "file:README.md");
        assert!(extracted.edges.is_empty());
        assert!(extracted.unresolved_refs.is_empty());
    }

    #[test]
    fn syntactically_broken_source_is_tolerated() {
        let extractor = Extractor::new();
        let file = scanned("broken.rs", Language::Rust);
        // Clearly invalid Rust: produces ERROR nodes in the parse tree.
        let source = "fn main( {{{ let x = ;;; @@@ unterminated";

        let extracted = extractor.extract(&file, source).unwrap();

        // Tolerant fallback: still get the file node, no panic.
        let file_node = extracted
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::File)
            .expect("file node present even on parse error");
        assert_eq!(file_node.id, "file:broken.rs");
    }

    #[test]
    fn file_node_id_format() {
        assert_eq!(file_node_id("src/lib.rs"), "file:src/lib.rs");
        assert_eq!(file_node_id("a/b/c.py"), "file:a/b/c.py");
    }

    #[test]
    fn make_node_id_format_is_stable() {
        let id = make_node_id(NodeKind::Function, "src/main.rs", "main", 1);
        assert_eq!(id, "function:src/main.rs:main:1");

        // Stable: identical inputs produce identical ids.
        let again = make_node_id(NodeKind::Function, "src/main.rs", "main", 1);
        assert_eq!(id, again);

        // Method with a qualified name.
        assert_eq!(
            make_node_id(NodeKind::Method, "src/foo.rs", "Foo::bar", 42),
            "method:src/foo.rs:Foo::bar:42"
        );
    }

    #[test]
    fn empty_file_keeps_line_invariant() {
        let extractor = Extractor::new();
        let file = scanned("empty.rs", Language::Rust);

        let extracted = extractor.extract(&file, "").unwrap();

        assert_eq!(extracted.total_lines, 0);
        let file_node = &extracted.nodes[0];
        assert!(file_node.start_line <= file_node.end_line);
    }

    #[test]
    fn parsing_reuses_cached_parser_across_calls() {
        let extractor = Extractor::new();
        let file = scanned("src/a.rs", Language::Rust);

        // Two extractions of the same language should both succeed, exercising
        // the parser cache (insert then reuse).
        assert!(extractor.extract(&file, "fn a() {}").is_ok());
        assert!(extractor.extract(&file, "fn b() {}").is_ok());
    }

    #[test]
    fn grammar_backed_generic_languages_extract_basic_symbols() {
        let cases = [
            (
                Language::Java,
                "src/UserService.java",
                "import java.util.List;\nclass UserService { void entry() { helper(); } void helper() {} }\n",
                "entry",
            ),
            (
                Language::CSharp,
                "src/UserService.cs",
                "using System;\nclass UserService { void Entry() { Helper(); } void Helper() {} }\n",
                "Entry",
            ),
            (
                Language::C,
                "src/main.c",
                "#include <stdio.h>\nvoid helper() {}\nvoid entry() { helper(); }\n",
                "entry",
            ),
            (
                Language::Cpp,
                "src/main.cpp",
                "#include <vector>\nclass Worker {};\nvoid helper() {}\nvoid entry() { helper(); }\n",
                "entry",
            ),
            (
                Language::Ruby,
                "lib/worker.rb",
                "require \"json\"\nclass Worker\n  def run\n    helper()\n  end\n  def helper\n  end\nend\n",
                "run",
            ),
            (
                Language::Php,
                "src/worker.php",
                "<?php\nuse Foo\\Bar;\nclass Worker {}\nfunction helper() {}\nfunction entry() { helper(); }\n",
                "entry",
            ),
            (
                Language::Swift,
                "Sources/App.swift",
                "import Foundation\nclass Worker {}\nfunc helper() {}\nfunc entry() { helper() }\n",
                "entry",
            ),
            (
                Language::Kotlin,
                "src/App.kt",
                "import foo.Bar\nclass Worker\nfun helper() {}\nfun entry() { helper() }\n",
                "entry",
            ),
            (
                Language::Scala,
                "src/App.scala",
                "import foo.Bar\nclass Worker\ndef helper(): Unit = {}\ndef entry(): Unit = helper()\n",
                "entry",
            ),
            (
                Language::Dart,
                "lib/app.dart",
                "import 'dart:math';\nclass Worker {}\nvoid helper() {}\nvoid entry() { helper(); }\n",
                "entry",
            ),
            (
                Language::Lua,
                "src/app.lua",
                "require \"json\"\nfunction helper() end\nfunction entry() helper() end\n",
                "entry",
            ),
            (
                Language::Shell,
                "scripts/run.sh",
                "source ./env.sh\nhelper() {\n  echo hi\n}\nentry() {\n  helper\n}\n",
                "entry",
            ),
            (
                Language::Css,
                "src/app.css",
                "@import \"base.css\";\n.button { color: red; }\n",
                "base.css",
            ),
            (
                Language::Html,
                "index.html",
                "<script src=\"app.js\"></script>\n<div></div>\n",
                "app.js",
            ),
        ];

        let extractor = Extractor::new();
        for (language, path, source, expected_name) in cases {
            let extracted = extractor
                .extract(&scanned(path, language), source)
                .unwrap_or_else(|err| panic!("extract failed for {language:?}: {err}"));
            assert!(
                extracted
                    .nodes
                    .iter()
                    .any(|node| node.language == language && node.name == expected_name),
                "missing expected node {expected_name} for {language:?}: {:?}",
                extracted.nodes
            );
            assert!(
                extracted.nodes.len() > 1,
                "expected symbols beyond file node for {language:?}"
            );
        }
    }
}
