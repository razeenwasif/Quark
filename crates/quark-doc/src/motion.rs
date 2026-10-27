//! Where a caret goes inside one block's text: by character, as a reader
//! counts them, and by word.
//!
//! "Character" means grapheme cluster. An `é` written as `e` plus a combining
//! accent, or a flag, is one step for the arrow keys and one press of
//! Backspace, though it is several `char`s.

use unicode_segmentation::UnicodeSegmentation;

/// `char` offsets of every grapheme boundary, both ends included.
fn boundaries(text: &str) -> Vec<usize> {
    let mut out = Vec::with_capacity(text.len() + 1);
    let mut chars = 0;
    out.push(0);
    for g in text.graphemes(true) {
        chars += g.chars().count();
        out.push(chars);
    }
    out
}

/// The grapheme boundary before `offset`.
pub fn prev_char(text: &str, offset: usize) -> usize {
    boundaries(text)
        .into_iter()
        .rev()
        .find(|&b| b < offset)
        .unwrap_or(0)
}

/// The grapheme boundary after `offset`.
pub fn next_char(text: &str, offset: usize) -> usize {
    let b = boundaries(text);
    let last = *b.last().unwrap_or(&0);
    b.into_iter().find(|&b| b > offset).unwrap_or(last)
}

/// Words as (start, end) char offsets.
fn words(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut chars = 0;
    for piece in text.split_word_bounds() {
        let n = piece.chars().count();
        if piece.chars().any(|c| c.is_alphanumeric()) {
            out.push((chars, chars + n));
        }
        chars += n;
    }
    out
}

/// The start of the word before `offset`, as ⌥← goes.
pub fn prev_word(text: &str, offset: usize) -> usize {
    words(text)
        .into_iter()
        .rev()
        .map(|(s, _)| s)
        .find(|&s| s < offset)
        .unwrap_or(0)
}

/// The end of the word after `offset`, as ⌥→ goes.
pub fn next_word(text: &str, offset: usize) -> usize {
    let len = text.chars().count();
    words(text)
        .into_iter()
        .map(|(_, e)| e)
        .find(|&e| e > offset)
        .unwrap_or(len)
}

/// The word around `offset`, for a double-click. Between words, the gap.
pub fn word_at(text: &str, offset: usize) -> (usize, usize) {
    let mut chars = 0;
    for piece in text.split_word_bounds() {
        let n = piece.chars().count();
        if offset < chars + n || chars + n == text.chars().count() {
            return (chars, chars + n);
        }
        chars += n;
    }
    (offset, offset)
}

/// Words in a text, for counting.
pub fn word_count(text: &str) -> usize {
    text.unicode_words().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_combining_accent_is_one_step() {
        let text = "cafe\u{301}!";
        assert_eq!(next_char(text, 3), 5);
        assert_eq!(prev_char(text, 5), 3);
    }

    #[test]
    fn word_steps_skip_punctuation_and_space() {
        let text = "Hello, brave new world.";
        assert_eq!(next_word(text, 0), 5);
        assert_eq!(next_word(text, 5), 12);
        assert_eq!(prev_word(text, 12), 7);
        assert_eq!(prev_word(text, 7), 0);
        assert_eq!(next_word(text, 22), 23);
    }

    #[test]
    fn a_double_click_takes_the_whole_word() {
        let text = "one two three";
        assert_eq!(word_at(text, 5), (4, 7));
        assert_eq!(word_at(text, 13), (8, 13));
    }

    #[test]
    fn words_are_counted_as_a_reader_would() {
        assert_eq!(word_count("It's a well-known fact — 42 of them."), 8);
    }
}
