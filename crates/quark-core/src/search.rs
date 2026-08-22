//! Search state and result bookkeeping.
//!
//! The actual text matching happens in `quark-pdf` against PDFium's extracted
//! text; this module owns the query, the options, the accumulated results and
//! the notion of "which hit am I on", none of which need a document to test.

use serde::{Deserialize, Serialize};

/// Options that change what counts as a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SearchOptions {
    pub match_case: bool,
    pub whole_words: bool,
    /// Search every open document rather than only the active one.
    pub all_documents: bool,
    /// Include comment text and bookmark titles as well as page text.
    pub include_comments: bool,
    pub include_bookmarks: bool,
}

/// One match.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub page: usize,
    /// Character offset of the match within the page's extracted text.
    pub char_index: usize,
    pub char_len: usize,
    /// Surrounding text for the results list.
    pub context: String,
    /// Byte range within `context` covering the match itself, so the list can
    /// embolden it without re-searching.
    pub highlight: (usize, usize),
    /// Which document the hit came from, when searching across all of them.
    pub doc: usize,
}

/// Accumulated results and the cursor within them.
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    pub query: String,
    pub options: SearchOptions,
    pub hits: Vec<SearchHit>,
    /// Index into `hits`, or `None` before the first navigation.
    pub current: Option<usize>,
    /// True while a background search is still running.
    pub running: bool,
    /// Pages scanned so far, for the progress indicator.
    pub scanned_pages: usize,
    pub total_pages: usize,
}

impl SearchState {
    pub fn clear(&mut self) {
        self.hits.clear();
        self.current = None;
        self.running = false;
        self.scanned_pages = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    pub fn len(&self) -> usize {
        self.hits.len()
    }

    /// Advances to the next hit, wrapping at the end.
    ///
    /// Wrapping rather than stopping matches every find bar users already know,
    /// and the caller can detect the wrap by watching the index decrease.
    pub fn next(&mut self) -> Option<&SearchHit> {
        if self.hits.is_empty() {
            return None;
        }
        self.current = Some(match self.current {
            Some(i) => (i + 1) % self.hits.len(),
            None => 0,
        });
        self.hits.get(self.current?)
    }

    pub fn prev(&mut self) -> Option<&SearchHit> {
        if self.hits.is_empty() {
            return None;
        }
        self.current = Some(match self.current {
            Some(0) | None => self.hits.len() - 1,
            Some(i) => i - 1,
        });
        self.hits.get(self.current?)
    }

    /// Selects the first hit at or after `page`, so that starting a search
    /// jumps forward from where the reader is rather than back to page 1.
    pub fn select_nearest(&mut self, page: usize) {
        if self.hits.is_empty() {
            self.current = None;
            return;
        }
        let idx = self
            .hits
            .iter()
            .position(|h| h.page >= page)
            .unwrap_or(0);
        self.current = Some(idx);
    }

    pub fn current_hit(&self) -> Option<&SearchHit> {
        self.current.and_then(|i| self.hits.get(i))
    }

    /// "3 of 128" for the find bar.
    pub fn position_label(&self) -> String {
        if self.query.is_empty() {
            return String::new();
        }
        if self.hits.is_empty() {
            return if self.running {
                "Searching…".into()
            } else {
                "No results".into()
            };
        }
        match self.current {
            Some(i) => format!("{} of {}", i + 1, self.hits.len()),
            None => format!("{} results", self.hits.len()),
        }
    }

    /// Fraction of the document scanned, for a progress bar.
    pub fn progress(&self) -> f32 {
        if self.total_pages == 0 {
            return 1.0;
        }
        (self.scanned_pages as f32 / self.total_pages as f32).clamp(0.0, 1.0)
    }
}

/// Finds every occurrence of `needle` in `haystack`, honouring the options.
///
/// Returns character offsets, not byte offsets: PDFium indexes its extracted
/// text by character, and mixing the two silently breaks on any non-ASCII
/// document.
pub fn find_matches(haystack: &str, needle: &str, opts: &SearchOptions) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }

    // Work over char vectors so offsets are in characters throughout. Case
    // folding per-character keeps the two vectors aligned; folding the whole
    // string could change its length (e.g. 'İ' lowercases to two chars) and
    // desynchronise every offset after it.
    let fold = |c: char| -> char {
        if opts.match_case {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        }
    };
    let hay: Vec<char> = haystack.chars().map(fold).collect();
    let ned: Vec<char> = needle.chars().map(fold).collect();

    if ned.len() > hay.len() {
        return Vec::new();
    }

    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut i = 0;
    while i + ned.len() <= hay.len() {
        if hay[i..i + ned.len()] == ned[..] {
            let ok = !opts.whole_words || {
                let before = i == 0 || !is_word(hay[i - 1]);
                let after = i + ned.len() >= hay.len() || !is_word(hay[i + ned.len()]);
                before && after
            };
            if ok {
                out.push((i, ned.len()));
                // Non-overlapping: advance past the whole match.
                i += ned.len();
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Builds the context snippet shown in the results list.
///
/// Returns the snippet and the byte range of the match inside it, so the UI can
/// embolden the hit without searching the snippet again.
pub fn context_for(text: &str, char_index: usize, char_len: usize, window: usize) -> (String, (usize, usize)) {
    let chars: Vec<char> = text.chars().collect();
    let start = char_index.saturating_sub(window);
    let end = (char_index + char_len + window).min(chars.len());
    let hit_start = char_index.min(chars.len());
    let hit_end = (char_index + char_len).min(chars.len());

    let mut snippet = String::new();
    if start > 0 {
        snippet.push('…');
    }
    let lead_len = snippet.len();

    let before: String = chars[start..hit_start].iter().collect();
    let matched: String = chars[hit_start..hit_end].iter().collect();
    let after: String = chars[hit_end..end].iter().collect();

    // Newlines inside a snippet break the single-line list rows.
    let before = before.replace(['\n', '\r'], " ");
    let matched_clean = matched.replace(['\n', '\r'], " ");
    let after = after.replace(['\n', '\r'], " ");

    snippet.push_str(&before);
    let h0 = lead_len + before.len();
    snippet.push_str(&matched_clean);
    let h1 = h0 + matched_clean.len();
    snippet.push_str(&after);
    if end < chars.len() {
        snippet.push('…');
    }
    (snippet, (h0, h1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> SearchOptions {
        SearchOptions::default()
    }

    #[test]
    fn finds_every_occurrence() {
        let m = find_matches("the cat sat on the mat", "at", &opts());
        assert_eq!(m.len(), 3);
        assert_eq!(m[0], (5, 2));
    }

    #[test]
    fn is_case_insensitive_by_default_and_exact_when_asked() {
        assert_eq!(find_matches("Hello HELLO hello", "hello", &opts()).len(), 3);
        let strict = SearchOptions {
            match_case: true,
            ..opts()
        };
        assert_eq!(find_matches("Hello HELLO hello", "hello", &strict).len(), 1);
    }

    #[test]
    fn whole_words_rejects_substrings() {
        let o = SearchOptions {
            whole_words: true,
            ..opts()
        };
        assert_eq!(find_matches("cat concatenate cats cat.", "cat", &o).len(), 2);
        // "cat" alone and "cat." both count; "cats" and "concatenate" do not.
    }

    #[test]
    fn offsets_are_in_characters_not_bytes() {
        // Each emoji is 4 bytes but 1 char. A byte-based implementation
        // reports 8 here and highlights the wrong text.
        let hits = find_matches("🙂🙂needle", "needle", &opts());
        assert_eq!(hits, vec![(2, 6)]);
    }

    #[test]
    fn case_folding_keeps_offsets_aligned_for_non_ascii() {
        let hits = find_matches("ÄÖÜ straße", "STRASSE", &opts());
        // No match — folding is per-character, so ß never becomes "ss". The
        // point of the test is that it does not panic or misreport an offset.
        assert!(hits.is_empty());
        let hits = find_matches("ÄÖÜ text", "äöü", &opts());
        assert_eq!(hits, vec![(0, 3)]);
    }

    #[test]
    fn matches_do_not_overlap() {
        // "aaaa" contains two non-overlapping "aa", not three.
        assert_eq!(find_matches("aaaa", "aa", &opts()).len(), 2);
    }

    #[test]
    fn empty_query_and_oversized_query_find_nothing() {
        assert!(find_matches("abc", "", &opts()).is_empty());
        assert!(find_matches("ab", "abcdef", &opts()).is_empty());
    }

    #[test]
    fn next_and_prev_wrap_around() {
        let mut s = SearchState::default();
        s.hits = (0..3)
            .map(|i| SearchHit {
                page: i,
                char_index: 0,
                char_len: 1,
                context: String::new(),
                highlight: (0, 0),
                doc: 0,
            })
            .collect();
        assert_eq!(s.next().unwrap().page, 0);
        assert_eq!(s.next().unwrap().page, 1);
        assert_eq!(s.next().unwrap().page, 2);
        assert_eq!(s.next().unwrap().page, 0, "must wrap to the start");
        assert_eq!(s.prev().unwrap().page, 2, "must wrap to the end");
    }

    #[test]
    fn navigation_on_an_empty_result_set_is_a_no_op() {
        let mut s = SearchState::default();
        assert!(s.next().is_none());
        assert!(s.prev().is_none());
        assert_eq!(s.current, None);
    }

    #[test]
    fn select_nearest_jumps_forward_from_the_current_page() {
        let mut s = SearchState::default();
        s.hits = [1usize, 4, 9]
            .iter()
            .map(|&p| SearchHit {
                page: p,
                char_index: 0,
                char_len: 1,
                context: String::new(),
                highlight: (0, 0),
                doc: 0,
            })
            .collect();
        s.select_nearest(3);
        assert_eq!(s.current_hit().unwrap().page, 4);
        // Past every hit, it wraps to the first.
        s.select_nearest(50);
        assert_eq!(s.current_hit().unwrap().page, 1);
    }

    #[test]
    fn position_label_reports_state_clearly() {
        let mut s = SearchState::default();
        assert_eq!(s.position_label(), "");
        s.query = "x".into();
        assert_eq!(s.position_label(), "No results");
        s.running = true;
        assert_eq!(s.position_label(), "Searching…");
        s.running = false;
        s.hits = vec![SearchHit {
            page: 0,
            char_index: 0,
            char_len: 1,
            context: String::new(),
            highlight: (0, 0),
            doc: 0,
        }];
        s.next();
        assert_eq!(s.position_label(), "1 of 1");
    }

    #[test]
    fn context_brackets_the_match_and_marks_its_range() {
        let text = "alpha beta gamma delta epsilon";
        let (snippet, (a, b)) = context_for(text, 11, 5, 6);
        assert_eq!(&snippet[a..b], "gamma");
        assert!(snippet.starts_with('…'));
        assert!(snippet.ends_with('…'));
    }

    #[test]
    fn context_at_the_start_has_no_leading_ellipsis() {
        let (snippet, (a, b)) = context_for("needle in a haystack", 0, 6, 5);
        assert!(!snippet.starts_with('…'));
        assert_eq!(&snippet[a..b], "needle");
    }

    #[test]
    fn context_strips_newlines_so_rows_stay_one_line() {
        let (snippet, _) = context_for("line one\nline two\nline three", 9, 4, 10);
        assert!(!snippet.contains('\n'));
    }

    #[test]
    fn context_handles_multibyte_without_slicing_mid_character() {
        // Slicing by byte here would panic.
        let text = "αβγδε needle ζηθικ";
        let (snippet, (a, b)) = context_for(text, 6, 6, 4);
        assert_eq!(&snippet[a..b], "needle");
    }

    #[test]
    fn progress_is_complete_for_an_empty_document() {
        let s = SearchState::default();
        assert_eq!(s.progress(), 1.0);
    }
}
