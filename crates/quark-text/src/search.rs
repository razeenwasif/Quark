//! Literal search, and the one definition of how it folds case.
//!
//! Two callers need the same rule and must not drift: [`crate::Buffer::matches`]
//! walks the whole rope to answer "where do I jump next", and [`find_in`] walks
//! one visible line to answer "what do I paint". A search that jumps to a match
//! it will not highlight is worse than one that does neither.

/// Vim's smartcase: a pattern containing a capital is case-sensitive, one
/// without is not. That is what makes `/err` find `Error` and `/Error` not
/// find `err`.
pub fn is_sensitive(pattern: &str) -> bool {
    pattern.chars().any(|c| c.is_ascii_uppercase())
}

/// Whether two characters match under the folding rule.
///
/// ASCII rather than Unicode: `char::to_lowercase` can change how many
/// characters there are, and a match offset that is off by one is worse than a
/// search that will not fold a Turkish dotted I.
pub fn same(a: char, b: char, sensitive: bool) -> bool {
    if sensitive {
        a == b
    } else {
        a.eq_ignore_ascii_case(&b)
    }
}

/// Every occurrence of `pattern` in one line, as `(start, end)` character
/// columns.
///
/// Character columns rather than bytes, because that is what the front end
/// indexes by and what the rest of the viewport already speaks.
pub fn find_in(line: &str, pattern: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = pattern.chars().collect();
    let hay: Vec<char> = line.chars().collect();
    if needle.is_empty() || needle.len() > hay.len() {
        return Vec::new();
    }
    let sensitive = is_sensitive(pattern);

    let mut found = Vec::new();
    let mut at = 0;
    while at + needle.len() <= hay.len() {
        if hay[at..at + needle.len()]
            .iter()
            .zip(&needle)
            .all(|(&a, &b)| same(a, b, sensitive))
        {
            found.push((at, at + needle.len()));
            // Non-overlapping, as every editor's highlight is: `//` in `///`
            // is one match and a bit, not two overlapping ones.
            at += needle.len();
        } else {
            at += 1;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smartcase_is_vims() {
        assert!(!is_sensitive("err"));
        assert!(is_sensitive("Err"));
        assert!(!is_sensitive("err_1"), "digits are not capitals");
    }

    #[test]
    fn a_lowercase_pattern_ignores_case() {
        assert_eq!(find_in("Error and error", "error"), vec![(0, 5), (10, 15)]);
    }

    #[test]
    fn a_pattern_with_a_capital_does_not() {
        assert_eq!(find_in("Error and error", "Error"), vec![(0, 5)]);
    }

    #[test]
    fn columns_are_characters_not_bytes() {
        // The front end indexes by character, and a highlight painted at a
        // byte offset lands in the wrong place the moment a line has an
        // accent in it.
        assert_eq!(
            find_in("héllo world héllo", "héllo"),
            vec![(0, 5), (12, 17)]
        );
    }

    #[test]
    fn matches_do_not_overlap() {
        // `//` in `///` is one match and a bit, not two overlapping ones —
        // which is what every editor's highlight shows.
        assert_eq!(find_in("///", "//"), vec![(0, 2)]);
        assert_eq!(find_in("aaaa", "aa"), vec![(0, 2), (2, 4)]);
    }

    #[test]
    fn nothing_matches_nothing() {
        assert!(find_in("anything", "").is_empty());
        assert!(find_in("", "x").is_empty());
        assert!(find_in("short", "much longer than the line").is_empty());
    }
}
