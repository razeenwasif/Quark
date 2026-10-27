//! Which grammar parses what, and how its captures become colours.

use tree_sitter::Language as Grammar;

/// A grammar, its highlight query, and the extensions it claims.
pub struct Language {
    pub id: &'static str,
    pub extensions: &'static [&'static str],
    grammar: fn() -> Grammar,
    /// The `highlights.scm` shipped with the grammar, joined when the query is
    /// compiled. Using the grammar's own query rather than writing one keeps
    /// Quark out of the business of tracking every language's syntax as it
    /// changes.
    ///
    /// A list rather than one string because some grammars build on another —
    /// TypeScript's own query covers only what TypeScript adds, and without
    /// JavaScript's in front of it every `const` in a `.ts` file is plain text.
    pub queries: &'static [&'static str],
    /// A second grammar for the parts the first one hands over.
    ///
    /// Markdown is two grammars by design: one for the block structure —
    /// headings, fences, lists — and one for what is inside a paragraph:
    /// emphasis, links, code spans. The block grammar marks those stretches
    /// as `inline` nodes and stops, and without the second pass a markdown
    /// file gets its headings coloured and nothing else.
    pub inline: Option<Companion>,
}

/// A grammar that parses ranges the main one delegated, and its query.
pub struct Companion {
    /// Its own name, for the compiled-query cache — which is keyed by
    /// `&'static str` and must not be handed a string built at run time.
    pub id: &'static str,
    /// The node type whose ranges it is given.
    pub inside: &'static str,
    pub grammar: fn() -> Grammar,
    pub queries: &'static [&'static str],
}

impl Language {
    pub fn grammar(&self) -> Grammar {
        (self.grammar)()
    }
}

impl std::fmt::Debug for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Language").field("id", &self.id).finish()
    }
}

/// LaTeX's colours: the grammar ships none, so Quark's own.
const LATEX: &str = include_str!("../queries/latex.scm");

pub const LANGUAGES: &[Language] = &[
    Language {
        id: "latex",
        extensions: &["tex", "sty", "cls", "ltx", "dtx"],
        grammar: || codebook_tree_sitter_latex::LANGUAGE.into(),
        queries: &[LATEX],
        inline: None,
    },
];

/// Look a language up by file extension. `None` is an ordinary answer: a file
/// with no grammar is shown as plain text, not as an error.
pub fn for_path(path: &std::path::Path) -> Option<&'static Language> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|l| l.extensions.contains(&extension.as_str()))
}
