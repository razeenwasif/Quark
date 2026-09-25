//! Turning a document into a prompt.
//!
//! # What gets sent
//!
//! Not the whole document, by default. A textbook is millions of characters;
//! sending all of it on every question is slow, expensive, and — on a cloud
//! backend — ships far more than the user was asking about. [`Scope::Nearby`]
//! sends the current page and its neighbours, which is what almost every
//! question is actually about, and [`Scope::Whole`] is there when it is not.
//!
//! # Page markers
//!
//! Each page is fenced with its own number so the model can cite one. Quark
//! already has per-page text, so citations cost nothing to support and turn an
//! unverifiable answer into one the reader can check.

use crate::provider::ChatRequest;

/// How much of the document to include.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// Only the page on screen.
    Page,
    /// The current page and one either side, so a question about something
    /// spanning a page break still works.
    #[default]
    Nearby,
    /// Everything. Opt-in, because on a long document it is both expensive and
    /// a lot of text to hand to a third party.
    Whole,
}

impl Scope {
    pub const ALL: [Scope; 3] = [Scope::Page, Scope::Nearby, Scope::Whole];

    pub fn label(self) -> &'static str {
        match self {
            Scope::Page => "This page",
            Scope::Nearby => "This page and neighbours",
            Scope::Whole => "Whole document",
        }
    }

    /// Which page indices this scope covers, given the current page.
    pub fn pages(self, current: usize, count: usize) -> std::ops::Range<usize> {
        if count == 0 {
            return 0..0;
        }
        match self {
            Scope::Page => current.min(count - 1)..(current + 1).min(count),
            Scope::Nearby => {
                let start = current.saturating_sub(1);
                let end = (current + 2).min(count);
                start.min(count - 1)..end
            }
            Scope::Whole => 0..count,
        }
    }
}

/// Everything the prompt needs to know about the open document.
#[derive(Debug, Clone, Default)]
pub struct DocContext {
    pub title: String,
    /// Zero-based index of the page on screen.
    pub current_page: usize,
    pub page_count: usize,
    /// `(page_index, text)` for the pages the scope selected.
    pub pages: Vec<(usize, String)>,
    /// Whatever the user has highlighted, if anything.
    pub selection: Option<String>,
}

/// Roughly how many tokens a string costs.
///
/// Four characters per token is the usual English approximation. Only used to
/// warn before sending something enormous, so being within a factor of a
/// quarter is fine — the alternative is a tokeniser per provider.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Builds the system prompt.
pub fn system_prompt(ctx: &DocContext) -> String {
    let mut s = String::with_capacity(2048);
    s.push_str(
        "You are a reading assistant embedded in Quark, a PDF reader. You answer \
         questions about the document the user is reading.\n\n\
         Rules:\n\
         - Answer only from the document text supplied below. If the answer is not \
         in it, say so and say which pages you were given.\n\
         - Cite the page number for any specific claim, like (p. 12).\n\
         - The page text is extracted automatically, so tables and formulae may be \
         garbled. Say when something looks unreliable rather than guessing.\n\
         - Be concise. The panel is narrow.\n\n",
    );

    s.push_str("# Document\n");
    if !ctx.title.is_empty() {
        s.push_str(&format!("Title: {}\n", ctx.title));
    }
    if ctx.page_count > 0 {
        s.push_str(&format!(
            "The reader is on page {} of {}.\n",
            ctx.current_page + 1,
            ctx.page_count
        ));
    }

    if let Some(sel) = &ctx.selection {
        let sel = sel.trim();
        if !sel.is_empty() {
            // Ahead of the page text: a selection is the most specific signal
            // available about what the question is actually about.
            s.push_str("\n# Selected text\nThe reader has highlighted this:\n\n");
            s.push_str(sel);
            s.push('\n');
        }
    }

    s.push_str("\n# Pages\n");
    if ctx.pages.is_empty() {
        s.push_str(
            "(No text could be extracted. The document may be a scan with no text \
             layer — say so if asked about its contents.)\n",
        );
    } else {
        for (index, text) in &ctx.pages {
            s.push_str(&format!("\n--- page {} ---\n", index + 1));
            let t = text.trim();
            if t.is_empty() {
                s.push_str("(no extractable text on this page)\n");
            } else {
                s.push_str(t);
                s.push('\n');
            }
        }
    }
    s
}

/// Assembles a full request from the document context and the conversation.
pub fn build_request(
    model: impl Into<String>,
    ctx: &DocContext,
    history: Vec<crate::provider::Message>,
    max_tokens: u32,
) -> ChatRequest {
    ChatRequest {
        model: model.into(),
        system: system_prompt(ctx),
        messages: history,
        max_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> DocContext {
        DocContext {
            title: "Convex Optimization".into(),
            current_page: 4,
            page_count: 10,
            pages: vec![(3, "page four text".into()), (4, "page five text".into())],
            selection: None,
        }
    }

    #[test]
    fn nearby_covers_the_page_either_side() {
        assert_eq!(Scope::Nearby.pages(4, 10), 3..6);
        assert_eq!(Scope::Page.pages(4, 10), 4..5);
        assert_eq!(Scope::Whole.pages(4, 10), 0..10);
    }

    #[test]
    fn the_first_and_last_pages_do_not_run_off_the_ends() {
        // `current - 1` underflows on page one and `current + 2` runs past the
        // end on the last; both would panic when used to slice.
        assert_eq!(Scope::Nearby.pages(0, 10), 0..2);
        assert_eq!(Scope::Nearby.pages(9, 10), 8..10);
        assert_eq!(Scope::Page.pages(9, 10), 9..10);
    }

    #[test]
    fn an_empty_document_yields_an_empty_range_rather_than_panicking() {
        // A document still loading has no pages yet.
        for scope in Scope::ALL {
            assert!(scope.pages(0, 0).is_empty(), "{scope:?}");
        }
    }

    #[test]
    fn a_current_page_past_the_end_is_clamped() {
        // Page count arrives asynchronously; `current` can briefly lead it.
        for scope in Scope::ALL {
            let r = scope.pages(99, 3);
            assert!(r.end <= 3, "{scope:?} ran past the document: {r:?}");
            assert!(r.start <= r.end, "{scope:?} produced an inverted range");
        }
    }

    #[test]
    fn pages_are_numbered_from_one_for_the_reader() {
        // The model cites what the reader sees. Zero-based citations would be
        // wrong on every single page.
        let p = system_prompt(&ctx());
        assert!(p.contains("--- page 4 ---"), "{p}");
        assert!(p.contains("--- page 5 ---"), "{p}");
        assert!(p.contains("page 5 of 10"), "{p}");
    }

    #[test]
    fn the_selection_comes_before_the_page_text() {
        // It is the strongest signal about what is being asked, and burying it
        // after several pages of text makes it easy to overlook.
        let mut c = ctx();
        c.selection = Some("a highlighted phrase".into());
        let p = system_prompt(&c);
        let sel = p.find("a highlighted phrase").expect("selection missing");
        let pages = p.find("--- page 4 ---").expect("pages missing");
        assert!(sel < pages, "the selection was placed after the page text");
    }

    #[test]
    fn an_empty_selection_is_not_announced() {
        // Clicking without dragging leaves an empty string behind; a "Selected
        // text" heading with nothing under it just confuses the model.
        let mut c = ctx();
        c.selection = Some("   ".into());
        assert!(!system_prompt(&c).contains("Selected text"));
    }

    #[test]
    fn a_scanned_document_says_so_rather_than_looking_empty() {
        // With no text layer the model would otherwise answer as if the
        // document were blank.
        let mut c = ctx();
        c.pages.clear();
        let p = system_prompt(&c);
        assert!(p.contains("scan"), "{p}");
    }

    #[test]
    fn the_prompt_asks_for_page_citations() {
        // The whole reason page markers are in there.
        assert!(system_prompt(&ctx()).contains("Cite the page number"));
    }

    #[test]
    fn token_estimates_are_in_the_right_ballpark() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        // Rounds up: a partial token still costs one.
        assert_eq!(estimate_tokens("abcde"), 2);
        let big = "x".repeat(400_000);
        assert_eq!(estimate_tokens(&big), 100_000);
    }

    #[test]
    fn the_request_carries_the_context_as_the_system_prompt() {
        let req = build_request(
            "claude-opus-5",
            &ctx(),
            vec![crate::provider::Message::user("what is this about?")],
            4096,
        );
        assert!(req.system.contains("--- page 4 ---"));
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.model, "claude-opus-5");
    }
}
