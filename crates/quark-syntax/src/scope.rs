//! Twelve colours, from a few hundred capture names.
//!
//! Grammars name captures at whatever granularity their authors found useful:
//! `function.method.builtin`, `variable.parameter`, `punctuation.delimiter`.
//! A reader looking at code can tell about a dozen colours apart, so this
//! collapses all of it onto the set the theme actually defines. Anything
//! unrecognised is left uncoloured rather than given a thirteenth colour that
//! means "we did not know".

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Keyword,
    Function,
    Type,
    String,
    Number,
    Comment,
    Constant,
    Variable,
    Property,
    Operator,
    Punctuation,
    Attribute,

    /* ── prose ────────────────────────────────────────────────────────
    Markdown is a different vocabulary for a different kind of file, and
    two of these are not colours at all: emphasis and strong are a slope
    and a weight, which is how the text itself says them. Painting them as
    two more colours would be a legend to learn instead of a thing you can
    already read. */
    Heading,
    Emphasis,
    Strong,
    Link,
}

impl Scope {
    /// Map a capture name onto a colour.
    ///
    /// Matched on the leading segment, because the tail is a refinement we do
    /// not draw: `function.method` and `function.macro` are both a function as
    /// far as the eye is concerned.
    pub fn from_capture(name: &str) -> Option<Scope> {
        // One exception to matching on the head, and it earns its place.
        // Grammars disagree about literals: some capture `1` as `@number`,
        // and tree-sitter-rust captures every numeric literal as
        // `@constant.builtin`. Folding that into `Constant` would leave the
        // number colour unused in the language Proton is written in. Booleans
        // and `None` come along with it, which is what themes do anyway.
        if name.starts_with("constant.builtin") {
            return Some(Scope::Number);
        }

        // Markup is the one family where the *tail* carries the meaning:
        // `text` on its own is prose and stays uncoloured, and everything
        // interesting about it is what comes after the dot. Newer grammars
        // spell the same ideas `markup.*`, so both are read here rather than
        // waiting for the ones we use to be renamed.
        if let Some(kind) = name
            .strip_prefix("text.")
            .or_else(|| name.strip_prefix("markup."))
        {
            let head = kind.split('.').next().unwrap_or(kind);
            return Some(match head {
                "title" | "heading" => Scope::Heading,
                "emphasis" | "italic" => Scope::Emphasis,
                "strong" | "bold" => Scope::Strong,
                // A link is one thing to a reader, whichever half it is: the
                // text you click and the address it goes to.
                "uri" | "reference" | "link" => Scope::Link,
                // Inline code and fenced code, which is a literal wherever it
                // appears — the same colour a string gets, and for the same
                // reason: it is content rather than prose.
                "literal" | "raw" => Scope::String,
                _ => return None,
            });
        }

        let head = name.split('.').next().unwrap_or(name);
        Some(match head {
            "keyword" | "conditional" | "repeat" | "include" | "exception" | "storageclass" => {
                Scope::Keyword
            }
            "function" | "method" | "constructor" => Scope::Function,
            "type" | "class" | "struct" | "interface" | "namespace" | "module" | "tag" => {
                Scope::Type
            }
            "string" | "character" | "char" | "escape" | "regex" => Scope::String,
            "number" | "float" | "integer" | "boolean" => Scope::Number,
            "comment" => Scope::Comment,
            "constant" => Scope::Constant,
            "property" | "field" => Scope::Property,
            "operator" => Scope::Operator,
            "punctuation" | "delimiter" | "bracket" => Scope::Punctuation,
            "attribute" | "annotation" | "decorator" | "label" => Scope::Attribute,
            "variable" | "parameter" => Scope::Variable,
            // `@spell`, `@none`, `@text`, `@embedded` and whatever a grammar
            // invents next. Uncoloured is the right answer for all of them.
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refinements_collapse_onto_the_colour_they_refine() {
        assert_eq!(
            Scope::from_capture("function.method.builtin"),
            Some(Scope::Function)
        );
        assert_eq!(
            Scope::from_capture("variable.parameter"),
            Some(Scope::Variable)
        );
        assert_eq!(
            Scope::from_capture("punctuation.bracket"),
            Some(Scope::Punctuation)
        );
    }

    #[test]
    fn an_unknown_capture_is_left_alone() {
        // Not an error, and not a fallback colour: a grammar naming something
        // we have no colour for should render as ordinary text.
        assert_eq!(Scope::from_capture("spell"), None);
        assert_eq!(Scope::from_capture("something.new"), None);
    }
}
