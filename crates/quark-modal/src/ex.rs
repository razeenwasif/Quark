//! Ex commands — the `:` line.
//!
//! Parsing only. What each command *does* is in the session (`crate::session`), because
//! that is where the buffers are; this decides what was typed.
//!
//! Two conventions from vim are load-bearing rather than decorative:
//!
//! - **Names abbreviate.** `:w`, `:wr`, `:wri`, `:writ` and `:write` are one
//!   command. Nobody types `:write`, and a table of exact spellings would make
//!   the two-character form — the only one anyone uses — a special case.
//! - **`!` is a separate token from the name.** It means "I know, do it
//!   anyway", and every command that can refuse needs somewhere to say so.
//!
//! What is deliberately *not* here: ranges (`:1,5d`), registers, and filters
//! (`:%!sort`). A line this cannot parse is passed through untouched, so the
//! front end keeps handling `:vsplit` and `:q` — which are still view state.

/// A parsed ex line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ex {
    /// The command name as typed, which may be an abbreviation.
    pub name: String,
    /// A trailing `!`.
    pub bang: bool,
    /// Everything after the name, trimmed. `None` when there was nothing.
    pub arg: Option<String>,
}

impl Ex {
    /// Parse a `:` line. `None` when it is not a plain `name[!] [arg]`.
    pub fn parse(line: &str) -> Option<Ex> {
        let line = line.trim().strip_prefix(':').unwrap_or(line.trim());
        let line = line.trim_start();

        // A leading digit, `%`, `.`, `$` or `'` is a range, and this parser
        // does not do ranges. Passing it through beats half-understanding it.
        let mut chars = line.chars();
        let first = chars.next()?;
        if !first.is_ascii_alphabetic() {
            return None;
        }

        let name: String = line
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        let rest = &line[name.len()..];

        let (bang, rest) = match rest.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };

        // The argument has to be separated from the name: `:edit` is edit with
        // no argument, and `:editfoo` is not edit at all.
        if !bang && !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return None;
        }

        let arg = rest.trim();
        Some(Ex {
            name,
            bang,
            arg: (!arg.is_empty()).then(|| arg.to_string()),
        })
    }

    /// Whether the name is an abbreviation of `full`, at least `least` long.
    ///
    /// Vim's rule: a command may be shortened to any unique prefix, and each
    /// one documents its own minimum. `:w` is write; `:wr` still is.
    pub fn is(&self, full: &str, least: usize) -> bool {
        self.name.len() >= least && self.name.len() <= full.len() && full.starts_with(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Ex {
        Ex::parse(line).unwrap_or_else(|| panic!("{line:?} should parse"))
    }

    #[test]
    fn a_bare_name_is_a_command() {
        let ex = parse(":w");
        assert_eq!(ex.name, "w");
        assert!(!ex.bang);
        assert_eq!(ex.arg, None);
    }

    #[test]
    fn the_colon_is_optional_because_the_engine_may_have_eaten_it() {
        assert_eq!(parse(":w"), parse("w"));
    }

    #[test]
    fn a_bang_is_its_own_token() {
        let ex = parse(":e!");
        assert_eq!(ex.name, "e");
        assert!(ex.bang);
        assert_eq!(ex.arg, None);
    }

    #[test]
    fn an_argument_is_whatever_follows() {
        let ex = parse(":w  src/main.rs");
        assert_eq!(ex.name, "w");
        assert_eq!(ex.arg.as_deref(), Some("src/main.rs"));

        let ex = parse(":e! ../other.txt");
        assert!(ex.bang);
        assert_eq!(ex.arg.as_deref(), Some("../other.txt"));
    }

    #[test]
    fn a_path_with_spaces_survives_whole() {
        // Trimmed at the ends and untouched in the middle. Quoting is a thing
        // this does not do, and inventing one would be worse than not.
        let ex = parse(":e My Documents/notes.md");
        assert_eq!(ex.arg.as_deref(), Some("My Documents/notes.md"));
    }

    #[test]
    fn a_name_and_its_argument_have_to_be_separated() {
        // `:editfoo` is not `:edit foo`. Without this every command would
        // silently match anything starting with its letters.
        assert_eq!(Ex::parse(":editfoo").unwrap().name, "editfoo");
        assert!(!Ex::parse(":editfoo").unwrap().is("edit", 1));
    }

    #[test]
    fn a_range_is_not_parsed_rather_than_half_parsed() {
        // Passing it through to whoever else might understand it beats
        // pretending `:1,5d` is a command called nothing.
        assert!(Ex::parse(":1,5d").is_none());
        assert!(Ex::parse(":%s/a/b/").is_none());
        assert!(Ex::parse(":'a,'bd").is_none());
        assert!(Ex::parse(":").is_none());
    }

    #[test]
    fn names_abbreviate_down_to_their_documented_minimum() {
        for spelling in ["w", "wr", "wri", "writ", "write"] {
            assert!(parse(spelling).is("write", 1), "{spelling}");
        }
        // Not an abbreviation of write, and not a typo to be guessed at.
        assert!(!parse("wx").is("write", 1));
        assert!(!parse("writes").is("write", 1));

        // `:x` needs one character; `:saveas` will not answer to `:s`, which
        // is substitute.
        assert!(parse("sav").is("saveas", 3));
        assert!(!parse("s").is("saveas", 3));
        assert!(!parse("sa").is("saveas", 3));
    }
}
