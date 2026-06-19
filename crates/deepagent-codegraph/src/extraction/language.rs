//! tree-sitter grammar registration.
//!
//! Maps the crate's [`Language`] enum onto the compiled-in tree-sitter
//! grammars. All grammars are pure-Rust-compilable (their C source is built
//! locally via `cc`); there is no Node.js dependency and no runtime grammar
//! download.
//!
//! Each grammar crate (0.23.x) exposes its parser as a
//! `tree_sitter_language::LanguageFn` constant (e.g. `tree_sitter_rust::LANGUAGE`,
//! `tree_sitter_typescript::LANGUAGE_TYPESCRIPT`). `LanguageFn` implements
//! `Into<tree_sitter::Language>`, so we call `.into()` to obtain the runtime
//! [`tree_sitter::Language`] used to configure a parser.

use crate::types::Language;

/// Return the tree-sitter [`tree_sitter::Language`] (grammar) for `lang`.
///
/// Returns `None` for [`Language::Other`], the catch-all for files that are
/// registered (so they appear in the project map) but not parsed for symbols.
///
/// For TypeScript this selects the TypeScript grammar (`LANGUAGE_TYPESCRIPT`);
/// the TSX dialect is not distinguished at this layer because `.tsx` files map
/// to [`Language::TypeScript`] in [`Language::from_extension`].
pub fn ts_language(lang: Language) -> Option<tree_sitter::Language> {
    let language = match lang {
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Language::Python => tree_sitter_python::LANGUAGE.into(),
        Language::Go => tree_sitter_go::LANGUAGE.into(),
        Language::Java => tree_sitter_java::LANGUAGE.into(),
        Language::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
        Language::C => tree_sitter_c::LANGUAGE.into(),
        Language::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        Language::Ruby => tree_sitter_ruby::LANGUAGE.into(),
        Language::Php => tree_sitter_php::LANGUAGE_PHP.into(),
        Language::Swift => tree_sitter_swift::LANGUAGE.into(),
        Language::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
        Language::Scala => tree_sitter_scala::LANGUAGE.into(),
        Language::Dart => tree_sitter_dart::LANGUAGE.into(),
        Language::Lua => tree_sitter_lua::LANGUAGE.into(),
        Language::Shell => tree_sitter_bash::LANGUAGE.into(),
        Language::Css => tree_sitter_css::LANGUAGE.into(),
        Language::Html => tree_sitter_html::LANGUAGE.into(),
        Language::Elixir
        | Language::Haskell
        | Language::R
        | Language::Julia
        | Language::Sql
        | Language::Xml
        | Language::Vue
        | Language::Svelte
        | Language::Other => return None,
    };
    Some(language)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAMMAR_LANGUAGES: &[Language] = &[
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
        Language::Lua,
        Language::Shell,
        Language::Css,
        Language::Html,
    ];

    #[test]
    fn ts_language_returns_some_for_supported_languages() {
        for &lang in GRAMMAR_LANGUAGES {
            assert!(
                ts_language(lang).is_some(),
                "expected a grammar for {lang:?}"
            );
        }
    }

    #[test]
    fn ts_language_returns_none_for_other() {
        assert!(ts_language(Language::Other).is_none());
    }

    #[test]
    fn returned_grammars_are_usable_by_a_parser() {
        // A grammar that cannot be set on a parser would indicate an
        // ABI/version mismatch between tree-sitter core and the grammar crate.
        for &lang in GRAMMAR_LANGUAGES {
            let grammar = ts_language(lang).expect("grammar present");
            let mut parser = tree_sitter::Parser::new();
            assert!(
                parser.set_language(&grammar).is_ok(),
                "parser rejected grammar for {lang:?}"
            );
        }
    }
}
