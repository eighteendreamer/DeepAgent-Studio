//! Core data types: [`NodeKind`], [`EdgeKind`], [`Language`], [`Node`],
//! [`Edge`], [`FileRecord`], and [`UnresolvedRef`].
//!
//! These types are the shared vocabulary of the code-graph engine. Their
//! string representations ([`NodeKind::as_str`], [`EdgeKind::as_str`],
//! [`Language::as_str`]) are the exact values persisted in SQLite, so the
//! `as_str` / `parse` pair must round-trip losslessly to keep the store layer
//! mapping trivial. See `design.md` (Data Models) and the reference
//! `schema.sql` for the on-disk column layout.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Kind of a code symbol node.
///
/// The string form (`as_str`) is snake_case and is what gets written to the
/// `nodes.kind` column. [`NodeKind::parse`] is tolerant: unknown values fall
/// back to [`NodeKind::Variable`], the most generic symbol kind, so that a
/// schema produced by a newer version never makes the reader panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    File,
    Module,
    Class,
    Struct,
    Interface,
    Trait,
    Function,
    Method,
    Property,
    Field,
    Variable,
    Constant,
    Enum,
    EnumMember,
    TypeAlias,
    Namespace,
    Import,
    Route,
}

impl NodeKind {
    /// snake_case string used for SQLite storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeKind::File => "file",
            NodeKind::Module => "module",
            NodeKind::Class => "class",
            NodeKind::Struct => "struct",
            NodeKind::Interface => "interface",
            NodeKind::Trait => "trait",
            NodeKind::Function => "function",
            NodeKind::Method => "method",
            NodeKind::Property => "property",
            NodeKind::Field => "field",
            NodeKind::Variable => "variable",
            NodeKind::Constant => "constant",
            NodeKind::Enum => "enum",
            NodeKind::EnumMember => "enum_member",
            NodeKind::TypeAlias => "type_alias",
            NodeKind::Namespace => "namespace",
            NodeKind::Import => "import",
            NodeKind::Route => "route",
        }
    }

    /// Parse a stored string back into a [`NodeKind`].
    ///
    /// Unknown values fall back to [`NodeKind::Variable`] so reads never fail
    /// on data written by a newer schema. Use [`NodeKind::try_parse`] when the
    /// caller needs to distinguish "unknown" from a real value.
    pub fn parse(s: &str) -> NodeKind {
        NodeKind::try_parse(s).unwrap_or(NodeKind::Variable)
    }

    /// Strict parse that returns `None` for unknown values.
    pub fn try_parse(s: &str) -> Option<NodeKind> {
        let kind = match s {
            "file" => NodeKind::File,
            "module" => NodeKind::Module,
            "class" => NodeKind::Class,
            "struct" => NodeKind::Struct,
            "interface" => NodeKind::Interface,
            "trait" => NodeKind::Trait,
            "function" => NodeKind::Function,
            "method" => NodeKind::Method,
            "property" => NodeKind::Property,
            "field" => NodeKind::Field,
            "variable" => NodeKind::Variable,
            "constant" => NodeKind::Constant,
            "enum" => NodeKind::Enum,
            "enum_member" => NodeKind::EnumMember,
            "type_alias" => NodeKind::TypeAlias,
            "namespace" => NodeKind::Namespace,
            "import" => NodeKind::Import,
            "route" => NodeKind::Route,
            _ => return None,
        };
        Some(kind)
    }
}

/// Kind of an edge (relationship) between two nodes.
///
/// The string form (`as_str`) is snake_case and is what gets written to the
/// `edges.kind` column. [`EdgeKind::parse`] is tolerant: unknown values fall
/// back to [`EdgeKind::References`], the most generic relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Contains,
    Calls,
    Imports,
    Exports,
    Extends,
    Implements,
    References,
    TypeOf,
}

impl EdgeKind {
    /// snake_case string used for SQLite storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeKind::Contains => "contains",
            EdgeKind::Calls => "calls",
            EdgeKind::Imports => "imports",
            EdgeKind::Exports => "exports",
            EdgeKind::Extends => "extends",
            EdgeKind::Implements => "implements",
            EdgeKind::References => "references",
            EdgeKind::TypeOf => "type_of",
        }
    }

    /// Parse a stored string back into an [`EdgeKind`].
    ///
    /// Unknown values fall back to [`EdgeKind::References`]. Use
    /// [`EdgeKind::try_parse`] for strict parsing.
    pub fn parse(s: &str) -> EdgeKind {
        EdgeKind::try_parse(s).unwrap_or(EdgeKind::References)
    }

    /// Strict parse that returns `None` for unknown values.
    pub fn try_parse(s: &str) -> Option<EdgeKind> {
        let kind = match s {
            "contains" => EdgeKind::Contains,
            "calls" => EdgeKind::Calls,
            "imports" => EdgeKind::Imports,
            "exports" => EdgeKind::Exports,
            "extends" => EdgeKind::Extends,
            "implements" => EdgeKind::Implements,
            "references" => EdgeKind::References,
            "type_of" => EdgeKind::TypeOf,
            _ => return None,
        };
        Some(kind)
    }
}

/// Source language of a file / symbol.
///
/// [`Language::Other`] is the catch-all for files we register (so they appear
/// in the project map) but do not extract symbols from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Java,
    CSharp,
    C,
    Cpp,
    Ruby,
    Php,
    Swift,
    Kotlin,
    Scala,
    Dart,
    Elixir,
    Lua,
    Haskell,
    R,
    Julia,
    Shell,
    Sql,
    Css,
    Html,
    Xml,
    Vue,
    Svelte,
    Other,
}

impl Language {
    /// snake_case string used for SQLite storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::TypeScript => "typescript",
            Language::JavaScript => "javascript",
            Language::Python => "python",
            Language::Go => "go",
            Language::Java => "java",
            Language::CSharp => "csharp",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Ruby => "ruby",
            Language::Php => "php",
            Language::Swift => "swift",
            Language::Kotlin => "kotlin",
            Language::Scala => "scala",
            Language::Dart => "dart",
            Language::Elixir => "elixir",
            Language::Lua => "lua",
            Language::Haskell => "haskell",
            Language::R => "r",
            Language::Julia => "julia",
            Language::Shell => "shell",
            Language::Sql => "sql",
            Language::Css => "css",
            Language::Html => "html",
            Language::Xml => "xml",
            Language::Vue => "vue",
            Language::Svelte => "svelte",
            Language::Other => "other",
        }
    }

    /// Parse a stored language string. Unknown values map to
    /// [`Language::Other`].
    pub fn parse(s: &str) -> Language {
        match s {
            "rust" => Language::Rust,
            "typescript" => Language::TypeScript,
            "javascript" => Language::JavaScript,
            "python" => Language::Python,
            "go" => Language::Go,
            "java" => Language::Java,
            "csharp" => Language::CSharp,
            "c" => Language::C,
            "cpp" => Language::Cpp,
            "ruby" => Language::Ruby,
            "php" => Language::Php,
            "swift" => Language::Swift,
            "kotlin" => Language::Kotlin,
            "scala" => Language::Scala,
            "dart" => Language::Dart,
            "elixir" => Language::Elixir,
            "lua" => Language::Lua,
            "haskell" => Language::Haskell,
            "r" => Language::R,
            "julia" => Language::Julia,
            "shell" => Language::Shell,
            "sql" => Language::Sql,
            "css" => Language::Css,
            "html" => Language::Html,
            "xml" => Language::Xml,
            "vue" => Language::Vue,
            "svelte" => Language::Svelte,
            _ => Language::Other,
        }
    }

    /// Map a file extension (without the leading dot) to a [`Language`].
    ///
    /// Matching is case-insensitive. Unknown extensions map to
    /// [`Language::Other`].
    pub fn from_extension(ext: &str) -> Language {
        match ext.to_ascii_lowercase().as_str() {
            "rs" => Language::Rust,
            "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "py" | "pyi" => Language::Python,
            "go" => Language::Go,
            "java" => Language::Java,
            "cs" => Language::CSharp,
            "c" | "h" => Language::C,
            "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Language::Cpp,
            "rb" | "rake" => Language::Ruby,
            "php" | "phtml" => Language::Php,
            "swift" => Language::Swift,
            "kt" | "kts" => Language::Kotlin,
            "scala" | "sc" => Language::Scala,
            "dart" => Language::Dart,
            "ex" | "exs" => Language::Elixir,
            "lua" => Language::Lua,
            "hs" | "lhs" => Language::Haskell,
            "r" => Language::R,
            "jl" => Language::Julia,
            "sh" | "bash" | "zsh" | "fish" => Language::Shell,
            "sql" => Language::Sql,
            "css" | "scss" | "sass" | "less" => Language::Css,
            "html" | "htm" => Language::Html,
            "xml" | "xaml" | "svg" => Language::Xml,
            "vue" => Language::Vue,
            "svelte" => Language::Svelte,
            _ => Language::Other,
        }
    }

    /// Infer a [`Language`] from a path.
    ///
    /// Prefers the file extension; for well-known extension-less build files
    /// (e.g. `Dockerfile`, `Makefile`, `CMakeLists.txt`) it returns
    /// [`Language::Other`] so they are still registered as files. Anything
    /// unrecognised also maps to [`Language::Other`].
    pub fn from_path(path: &Path) -> Language {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            return Language::from_extension(ext);
        }

        // Extension-less files: classify well-known build/config files.
        match path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_ascii_lowercase())
            .as_deref()
        {
            Some("dockerfile")
            | Some("makefile")
            | Some("cmakelists.txt")
            | Some("rakefile")
            | Some("gemfile")
            | Some("procfile")
            | Some("vagrantfile") => Language::Other,
            _ => Language::Other,
        }
    }
}

/// A code symbol node. Mirrors the `nodes` table columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// Stable id: `{kind}:{file}:{qualified_name}:{start_line}`.
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
    pub qualified_name: String,
    /// POSIX relative path.
    pub file_path: String,
    pub language: Language,
    pub start_line: u32,
    pub end_line: u32,
    pub start_column: u32,
    pub end_column: u32,
    pub signature: Option<String>,
    pub docstring: Option<String>,
    pub visibility: Option<String>,
    pub is_exported: bool,
    pub is_async: bool,
}

/// A relationship between two nodes. Mirrors the `edges` table columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// Source node id.
    pub source: String,
    /// Target node id.
    pub target: String,
    pub kind: EdgeKind,
    /// Arbitrary JSON metadata (stored as a JSON string in SQLite).
    pub metadata: Option<serde_json::Value>,
    pub line: Option<u32>,
    /// Provenance marker, e.g. `"heuristic"` for edges derived by name
    /// matching rather than precise resolution.
    pub provenance: Option<String>,
}

/// A tracked source file. Mirrors the `files` table columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    /// POSIX relative path (primary key in `files`).
    pub path: String,
    pub content_hash: String,
    pub language: Language,
    pub size: u64,
    pub modified_at: i64,
    pub indexed_at: i64,
}

/// An unresolved reference, parked for the resolver. Mirrors the
/// `unresolved_refs` table columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedRef {
    pub from_node_id: String,
    pub reference_name: String,
    pub reference_kind: String,
    pub line: u32,
    pub file_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const ALL_NODE_KINDS: &[NodeKind] = &[
        NodeKind::File,
        NodeKind::Module,
        NodeKind::Class,
        NodeKind::Struct,
        NodeKind::Interface,
        NodeKind::Trait,
        NodeKind::Function,
        NodeKind::Method,
        NodeKind::Property,
        NodeKind::Field,
        NodeKind::Variable,
        NodeKind::Constant,
        NodeKind::Enum,
        NodeKind::EnumMember,
        NodeKind::TypeAlias,
        NodeKind::Namespace,
        NodeKind::Import,
        NodeKind::Route,
    ];

    const ALL_EDGE_KINDS: &[EdgeKind] = &[
        EdgeKind::Contains,
        EdgeKind::Calls,
        EdgeKind::Imports,
        EdgeKind::Exports,
        EdgeKind::Extends,
        EdgeKind::Implements,
        EdgeKind::References,
        EdgeKind::TypeOf,
    ];

    const ALL_LANGUAGES: &[Language] = &[
        Language::Rust,
        Language::TypeScript,
        Language::JavaScript,
        Language::Python,
        Language::Go,
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
        Language::Other,
    ];

    #[test]
    fn node_kind_round_trips_through_string() {
        for &kind in ALL_NODE_KINDS {
            assert_eq!(
                NodeKind::parse(kind.as_str()),
                kind,
                "round-trip failed for {kind:?}"
            );
            assert_eq!(NodeKind::try_parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn node_kind_as_str_is_snake_case() {
        assert_eq!(NodeKind::EnumMember.as_str(), "enum_member");
        assert_eq!(NodeKind::TypeAlias.as_str(), "type_alias");
    }

    #[test]
    fn node_kind_unknown_falls_back_to_variable() {
        assert_eq!(NodeKind::parse("nonsense"), NodeKind::Variable);
        assert_eq!(NodeKind::parse(""), NodeKind::Variable);
        assert_eq!(NodeKind::try_parse("nonsense"), None);
    }

    #[test]
    fn edge_kind_round_trips_through_string() {
        for &kind in ALL_EDGE_KINDS {
            assert_eq!(
                EdgeKind::parse(kind.as_str()),
                kind,
                "round-trip failed for {kind:?}"
            );
            assert_eq!(EdgeKind::try_parse(kind.as_str()), Some(kind));
        }
    }

    #[test]
    fn edge_kind_as_str_is_snake_case() {
        assert_eq!(EdgeKind::TypeOf.as_str(), "type_of");
    }

    #[test]
    fn edge_kind_unknown_falls_back_to_references() {
        assert_eq!(EdgeKind::parse("nonsense"), EdgeKind::References);
        assert_eq!(EdgeKind::try_parse("nonsense"), None);
    }

    #[test]
    fn language_round_trips_through_string() {
        for &lang in ALL_LANGUAGES {
            assert_eq!(Language::parse(lang.as_str()), lang);
        }
    }

    #[test]
    fn language_from_extension_maps_common_extensions() {
        assert_eq!(Language::from_extension("rs"), Language::Rust);
        assert_eq!(Language::from_extension("ts"), Language::TypeScript);
        assert_eq!(Language::from_extension("tsx"), Language::TypeScript);
        assert_eq!(Language::from_extension("js"), Language::JavaScript);
        assert_eq!(Language::from_extension("jsx"), Language::JavaScript);
        assert_eq!(Language::from_extension("mjs"), Language::JavaScript);
        assert_eq!(Language::from_extension("cjs"), Language::JavaScript);
        assert_eq!(Language::from_extension("py"), Language::Python);
        assert_eq!(Language::from_extension("go"), Language::Go);
        assert_eq!(Language::from_extension("java"), Language::Java);
        assert_eq!(Language::from_extension("cs"), Language::CSharp);
        assert_eq!(Language::from_extension("c"), Language::C);
        assert_eq!(Language::from_extension("cpp"), Language::Cpp);
        assert_eq!(Language::from_extension("rb"), Language::Ruby);
        assert_eq!(Language::from_extension("php"), Language::Php);
        assert_eq!(Language::from_extension("swift"), Language::Swift);
        assert_eq!(Language::from_extension("kt"), Language::Kotlin);
        assert_eq!(Language::from_extension("scala"), Language::Scala);
        assert_eq!(Language::from_extension("dart"), Language::Dart);
        assert_eq!(Language::from_extension("ex"), Language::Elixir);
        assert_eq!(Language::from_extension("lua"), Language::Lua);
        assert_eq!(Language::from_extension("hs"), Language::Haskell);
        assert_eq!(Language::from_extension("r"), Language::R);
        assert_eq!(Language::from_extension("jl"), Language::Julia);
        assert_eq!(Language::from_extension("sh"), Language::Shell);
        assert_eq!(Language::from_extension("sql"), Language::Sql);
        assert_eq!(Language::from_extension("css"), Language::Css);
        assert_eq!(Language::from_extension("html"), Language::Html);
        assert_eq!(Language::from_extension("xml"), Language::Xml);
        assert_eq!(Language::from_extension("vue"), Language::Vue);
        assert_eq!(Language::from_extension("svelte"), Language::Svelte);
    }

    #[test]
    fn language_from_extension_is_case_insensitive() {
        assert_eq!(Language::from_extension("RS"), Language::Rust);
        assert_eq!(Language::from_extension("Ts"), Language::TypeScript);
    }

    #[test]
    fn language_from_extension_unknown_is_other() {
        assert_eq!(Language::from_extension("txt"), Language::Other);
        assert_eq!(Language::from_extension("md"), Language::Other);
        assert_eq!(Language::from_extension(""), Language::Other);
    }

    #[test]
    fn language_from_path_uses_extension() {
        assert_eq!(
            Language::from_path(Path::new("src/main.rs")),
            Language::Rust
        );
        assert_eq!(
            Language::from_path(Path::new("app/index.tsx")),
            Language::TypeScript
        );
        assert_eq!(
            Language::from_path(Path::new("scripts/run.py")),
            Language::Python
        );
    }

    #[test]
    fn language_from_path_handles_extension_less_build_files() {
        assert_eq!(
            Language::from_path(Path::new("Dockerfile")),
            Language::Other
        );
        assert_eq!(Language::from_path(Path::new("Makefile")), Language::Other);
        assert_eq!(
            Language::from_path(Path::new("path/to/CMakeLists.txt")),
            Language::Other
        );
    }

    #[test]
    fn language_from_path_unknown_is_other() {
        assert_eq!(Language::from_path(Path::new("notes.bin")), Language::Other);
        assert_eq!(Language::from_path(Path::new("LICENSE")), Language::Other);
    }
}
