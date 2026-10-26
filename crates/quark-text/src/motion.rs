//! Motions and text objects, resolved against a rope.
//!
//! vim classifies every character into one of three groups — word, punctuation,
//! whitespace — and almost every motion here is "walk until the class changes".
//! Getting that one function right is most of the work; `w`, `b`, `e`, `iw` and
//! `aw` all fall out of it.

use ropey::RopeSlice;

use crate::edit::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Whitespace,
    Word,
    Punctuation,
}

pub fn class_of(c: char) -> Class {
    if c.is_whitespace() {
        Class::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punctuation
    }
}

/// `W`, `B`, `E` treat every non-blank as one class.
fn class_at(text: RopeSlice, at: usize, big: bool) -> Class {
    let c = text.char(at);
    match (big, class_of(c)) {
        (true, Class::Whitespace) => Class::Whitespace,
        (true, _) => Class::Word,
        (false, k) => k,
    }
}

/// The next word start — vim's `w`.
pub fn word_forward(text: RopeSlice, from: usize, big: bool) -> usize {
    let len = text.len_chars();
    if from >= len {
        return len;
    }
    let mut at = from;
    let start_class = class_at(text, at, big);

    // Leave the current run…
    if start_class != Class::Whitespace {
        while at < len && class_at(text, at, big) == start_class {
            at += 1;
        }
    }
    // …then skip the blanks before the next one.
    while at < len && class_at(text, at, big) == Class::Whitespace {
        at += 1;
    }
    at
}

/// The previous word start — vim's `b`.
pub fn word_backward(text: RopeSlice, from: usize, big: bool) -> usize {
    if from == 0 {
        return 0;
    }
    let mut at = from - 1;
    while at > 0 && class_at(text, at, big) == Class::Whitespace {
        at -= 1;
    }
    if class_at(text, at, big) == Class::Whitespace {
        return at;
    }
    let run = class_at(text, at, big);
    while at > 0 && class_at(text, at - 1, big) == run {
        at -= 1;
    }
    at
}

/// The end of the current or next word — vim's `e`.
pub fn word_end(text: RopeSlice, from: usize, big: bool) -> usize {
    let len = text.len_chars();
    if from + 1 >= len {
        return len.saturating_sub(1);
    }
    let mut at = from + 1;
    while at < len && class_at(text, at, big) == Class::Whitespace {
        at += 1;
    }
    if at >= len {
        return len - 1;
    }
    let run = class_at(text, at, big);
    while at + 1 < len && class_at(text, at + 1, big) == run {
        at += 1;
    }
    at
}

/// The first character of the line `at` sits on.
pub fn line_start(text: RopeSlice, at: usize) -> usize {
    text.line_to_char(text.char_to_line(at.min(text.len_chars())))
}

/// The last character *before* the newline, so `$` lands on text rather than
/// on the line break.
pub fn line_end(text: RopeSlice, at: usize) -> usize {
    let line = text.char_to_line(at.min(text.len_chars()));
    let start = text.line_to_char(line);
    let mut end = start + text.line(line).len_chars();
    while end > start && matches!(text.char(end - 1), '\n' | '\r') {
        end -= 1;
    }
    end
}

/// First non-blank on the line — vim's `^`, and where `I` inserts.
pub fn first_non_blank(text: RopeSlice, at: usize) -> usize {
    let start = line_start(text, at);
    let end = line_end(text, at);
    let mut i = start;
    while i < end && text.char(i).is_whitespace() {
        i += 1;
    }
    i
}

/// The whole line including its newline — what `dd` removes.
pub fn line_range(text: RopeSlice, at: usize, count: usize) -> Range {
    let first = text.char_to_line(at.min(text.len_chars()));
    let last = (first + count.max(1) - 1).min(text.len_lines().saturating_sub(1));
    let start = text.line_to_char(first);
    let end = if last + 1 < text.len_lines() {
        text.line_to_char(last + 1)
    } else {
        text.len_chars()
    };
    Range::new(start, end)
}

/// Move down `count` lines, trying to hold `goal` as the column.
pub fn line_down(text: RopeSlice, at: usize, count: usize, goal: usize) -> usize {
    let line = text.char_to_line(at.min(text.len_chars()));
    let target = (line + count).min(text.len_lines().saturating_sub(1));
    column_on(text, target, goal)
}

pub fn line_up(text: RopeSlice, at: usize, count: usize, goal: usize) -> usize {
    let line = text.char_to_line(at.min(text.len_chars()));
    column_on(text, line.saturating_sub(count), goal)
}

fn column_on(text: RopeSlice, line: usize, goal: usize) -> usize {
    let start = text.line_to_char(line);
    let end = line_end(text, start);
    (start + goal).min(end)
}

/// Which column an offset sits at, for the goal column and the statusline.
pub fn column_of(text: RopeSlice, at: usize) -> usize {
    at - line_start(text, at)
}

/// `iw` — the run of same-class characters under the cursor, whitespace
/// included, because in vim `iw` on a space selects the spaces.
pub fn inner_word(text: RopeSlice, at: usize, big: bool) -> Range {
    let len = text.len_chars();
    if len == 0 {
        return Range::empty(0);
    }
    let at = at.min(len - 1);
    let run = class_at(text, at, big);

    let mut start = at;
    while start > 0 && class_at(text, start - 1, big) == run {
        start -= 1;
    }
    let mut end = at;
    while end + 1 < len && class_at(text, end + 1, big) == run {
        end += 1;
    }
    Range::new(start, end + 1)
}

/// `aw` — the word plus the whitespace after it, or before it when the word
/// ends the line. This asymmetry is vim's, and it is what makes `daw` leave a
/// sentence correctly spaced.
pub fn a_word(text: RopeSlice, at: usize, big: bool) -> Range {
    let len = text.len_chars();
    let inner = inner_word(text, at, big);
    if inner.is_empty() {
        return inner;
    }

    let mut end = inner.end;
    let mut took_trailing = false;
    while end < len && class_at(text, end, big) == Class::Whitespace && text.char(end) != '\n' {
        end += 1;
        took_trailing = true;
    }
    if took_trailing {
        return Range::new(inner.start, end);
    }

    let mut start = inner.start;
    while start > 0
        && class_at(text, start - 1, big) == Class::Whitespace
        && text.char(start - 1) != '\n'
    {
        start -= 1;
    }
    Range::new(start, inner.end)
}

/// `i(`, `i"`, `i{` … — the span between the nearest enclosing delimiters.
/// `around` includes the delimiters themselves, which is `a(`.
pub fn inside_pair(
    text: RopeSlice,
    at: usize,
    open: char,
    close: char,
    around: bool,
) -> Option<Range> {
    let len = text.len_chars();
    if len == 0 {
        return None;
    }
    let at = at.min(len - 1);

    // Quotes have no nesting, so the pair is simply the nearest one either side.
    if open == close {
        let mut start = None;
        let mut i = at;
        loop {
            if text.char(i) == open {
                start = Some(i);
                break;
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
        let start = start?;
        let mut end = start + 1;
        while end < len && text.char(end) != close {
            end += 1;
        }
        if end >= len {
            return None;
        }
        return Some(if around {
            Range::new(start, end + 1)
        } else {
            Range::new(start + 1, end)
        });
    }

    // Brackets nest, so walk out counting depth in both directions.
    let mut depth = 0i32;
    let mut i = at;
    let start = loop {
        let c = text.char(i);
        if c == close && i != at {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                break i;
            }
            depth -= 1;
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    };

    depth = 0;
    let mut j = start + 1;
    let end = loop {
        if j >= len {
            return None;
        }
        let c = text.char(j);
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                break j;
            }
            depth -= 1;
        }
        j += 1;
    };

    Some(if around {
        Range::new(start, end + 1)
    } else {
        Range::new(start + 1, end)
    })
}
