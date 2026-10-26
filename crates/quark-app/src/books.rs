//! The library on the home screen: the books in the person's folders, the
//! ones being read first, each where it was left, then a shelf for each
//! folder, every book with its cover.
//!
//! The books come from [`quark_pdf::library::Library`], walked on a worker.
//! Covers are asked for only as their books come on screen, drawn by the
//! service from each book's first page, and kept as textures here once given.
//! What is typed in the box above narrows the shelves to the books it names.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, TextureHandle, Ui, vec2};
use quark_pdf::library::Book;
use quark_pdf::render::Raster;
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

/// How many books Continue reading shows.
const READING: usize = 8;
/// A cover's box: books are taller than they are wide.
const COVER_W: f32 = 128.0;
const COVER_H: f32 = 176.0;
/// A whole card: its cover, two lines of title, who wrote it, and how far.
const CARD_H: f32 = COVER_H + 84.0;
/// The long side covers are drawn at: twice the box, for Retina.
pub(crate) const COVER_EDGE: u32 = (COVER_H as u32) * 2;

/// A shelf, as drawn.
#[derive(Debug, PartialEq)]
pub(crate) struct Shelf<'a> {
    pub name: String,
    pub books: Vec<&'a Book>,
    /// Continue reading: for coming back to, not for looking things up.
    pub reading: bool,
}

/// The shelves: Continue reading first, the last read first — unless words
/// are typed — then a shelf for each folder, the library folder's own books
/// first, each shelf's books by title; every one narrowed to the books the
/// words typed name.
pub(crate) fn shelves<'a>(books: &'a [Book], query: &str) -> Vec<Shelf<'a>> {
    let mut out = Vec::new();
    if query.trim().is_empty() {
        let mut reading: Vec<&Book> = books.iter().filter(|b| b.read.is_some()).collect();
        reading.sort_by(|a, b| b.read.cmp(&a.read));
        reading.truncate(READING);
        if !reading.is_empty() {
            out.push(Shelf { name: "Continue reading".into(), books: reading, reading: true });
        }
    }
    let mut by_shelf: Vec<((bool, String), Vec<&Book>)> = Vec::new();
    for b in books.iter().filter(|b| b.named_by(query)) {
        let key = (!b.shelf.is_empty(), b.shelf_name());
        match by_shelf.iter_mut().find(|(k, _)| *k == key) {
            Some((_, of)) => of.push(b),
            None => by_shelf.push((key, vec![b])),
        }
    }
    by_shelf.sort_by(|(a, _), (b, _)| (a.0, a.1.to_lowercase()).cmp(&(b.0, b.1.to_lowercase())));
    for ((_, name), mut of) in by_shelf {
        of.sort_by_key(|b| b.title.to_lowercase());
        out.push(Shelf { name, books: of, reading: false });
    }
    out
}

/// What the library wants the application to do.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BooksAction {
    None,
    Open(PathBuf),
    /// Called by another title and author in the library; both empty, its own.
    Rename { path: PathBuf, title: String, author: String },
    SetFolders(Vec<String>),
    LookAgain,
}

/// A book being renamed.
struct Renaming {
    path: PathBuf,
    file: String,
    title: String,
    author: String,
    renamed: bool,
    focused: bool,
}

#[derive(Default)]
pub(crate) struct BooksState {
    pub(crate) books: Vec<Book>,
    pub(crate) folders: Vec<String>,
    pub(crate) missing: Vec<String>,
    pub(crate) walking: bool,
    pub(crate) query: String,
    /// Covers given, by book; none when there is none to draw.
    covers: HashMap<PathBuf, Option<TextureHandle>>,
    /// Covers asked for, not given yet.
    asked: HashSet<PathBuf>,
    /// Covers come on screen this frame, to ask the service for.
    pub(crate) wanted: Vec<PathBuf>,
    renaming: Option<Renaming>,
    /// The folders being changed, one a line.
    editing: Option<String>,
}

impl BooksState {
    /// A cover drawn, or none to draw.
    pub(crate) fn cover_drawn(&mut self, ctx: &egui::Context, path: PathBuf, raster: Option<&Raster>) {
        self.asked.remove(&path);
        let texture = raster.map(|r| {
            let image = egui::ColorImage::from_rgba_unmultiplied([r.width as usize, r.height as usize], &r.rgba);
            ctx.load_texture(format!("cover:{}", path.display()), image, egui::TextureOptions::LINEAR)
        });
        self.covers.insert(path, texture);
    }

    /// Rename a book: its sheet, ready to type in.
    pub(crate) fn rename(&mut self, book: &Book) {
        self.renaming = Some(Renaming {
            path: book.path.clone(),
            file: book.file.clone(),
            title: book.title.clone(),
            author: book.author.clone(),
            renamed: book.renamed,
            focused: false,
        });
    }

    /// Whether a cover may be asked for: on screen, not given or asked yet.
    fn want(&mut self, book: &Book) {
        if book.in_cloud || self.covers.contains_key(&book.path) || self.asked.contains(&book.path) {
            return;
        }
        self.asked.insert(book.path.clone());
        self.wanted.push(book.path.clone());
    }
}

/// The library: its box and buttons, then its shelves.
pub(crate) fn show(ui: &mut Ui, p: &Palette, state: &mut BooksState) -> BooksAction {
    let mut action = BooksAction::None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let find = ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .hint_text("Find a book")
                .desired_width(280.0)
                .margin(vec2(10.0, 6.0)),
        );
        // Enter opens the first book the words name.
        if find.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if let Some(b) = state.books.iter().find(|b| b.named_by(&state.query)) {
                action = BooksAction::Open(b.path.clone());
            }
        }
        if state.walking {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(
                egui::RichText::new(format!("Looking through {}…", state.folders.join(", ")))
                    .font(theme::body(12.0))
                    .color(p.text_faint),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::ghost_button(ui, p, "Folders…").clicked() {
                state.editing = match state.editing {
                    Some(_) => None,
                    None => Some(state.folders.join("\n")),
                };
            }
            if !state.walking
                && widgets::ghost_button(ui, p, "Look again")
                    .on_hover_text("Walk the folders again for books added or taken away")
                    .clicked()
            {
                action = BooksAction::LookAgain;
            }
        });
    });
    ui.add_space(14.0);

    if !state.missing.is_empty() {
        ui.label(
            egui::RichText::new(format!("Not found: {}", state.missing.join(", ")))
                .font(theme::body(12.5))
                .color(p.warning),
        );
        ui.add_space(8.0);
    }
    if state.editing.is_some() || state.folders.is_empty() {
        if let Some(a) = folders_form(ui, p, state) {
            action = a;
        }
        return action;
    }
    if state.books.is_empty() {
        if !state.walking {
            faint(ui, p, &format!("No PDFs in {} yet.", state.folders.join(", ")));
        }
        return action;
    }

    let books = std::mem::take(&mut state.books);
    let shelved = shelves(&books, &state.query);
    if shelved.is_empty() {
        faint(ui, p, "None of your books is called that.");
    }
    for shelf in &shelved {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&shelf.name).font(theme::strong(15.0)).color(p.text));
            ui.label(
                egui::RichText::new(shelf.books.len().to_string())
                    .font(theme::body(12.5))
                    .color(p.text_faint),
            );
        });
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(22.0, 26.0);
            for book in &shelf.books {
                if let Some(a) = card(ui, p, state, book) {
                    action = a;
                }
            }
        });
        ui.add_space(30.0);
    }
    drop(shelved);
    state.books = books;

    if let Some(a) = rename_sheet(ui.ctx(), p, state) {
        action = a;
    }
    action
}

fn faint(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(egui::RichText::new(text).font(theme::body(12.5)).color(p.text_muted));
}

/// The folders the books are in, one a line.
fn folders_form(ui: &mut Ui, p: &Palette, state: &mut BooksState) -> Option<BooksAction> {
    let draft = state.editing.get_or_insert_with(|| state.folders.join("\n"));
    faint(
        ui,
        p,
        "The folders your books are in, one a line. Every PDF in them, and in the folders inside them, is shelved by the folder it is in.",
    );
    ui.add_space(8.0);
    ui.add(
        egui::TextEdit::multiline(draft)
            .hint_text(quark_pdf::library::DEFAULT_FOLDER)
            .desired_rows(3)
            .desired_width(520.0)
            .font(egui::TextStyle::Monospace),
    );
    ui.add_space(8.0);
    let mut action = None;
    ui.horizontal(|ui| {
        if widgets::primary_button(ui, p, "Use these folders", true).clicked() {
            let folders: Vec<String> =
                draft.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
            action = Some(BooksAction::SetFolders(folders));
        }
        if !state.folders.is_empty() && widgets::ghost_button(ui, p, "Cancel").clicked() {
            action = Some(BooksAction::None);
        }
    });
    if action.is_some() {
        state.editing = None;
    }
    action.filter(|a| *a != BooksAction::None)
}

/// A colour for a book with no cover yet, from its title.
fn hue(text: &str) -> f32 {
    let h = text.chars().fold(0u32, |h, c| (h.wrapping_mul(31).wrapping_add(c as u32)) % 360);
    h as f32 / 360.0
}

/// A book: its cover, title, who wrote it and how far it was read. Click to
/// open it where it was left; right-click or F2 to rename it.
fn card(ui: &mut Ui, p: &Palette, state: &mut BooksState, book: &Book) -> Option<BooksAction> {
    let (rect, response) = ui.allocate_exact_size(vec2(COVER_W, CARD_H), Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        state.want(book);
        paint_card(ui, p, state, book, rect, response.hovered());
    }
    let response = response.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(&book.title).font(theme::strong(12.5)));
        if !book.author.is_empty() {
            ui.label(&book.author);
        }
        ui.label(egui::RichText::new(quark_pdf::library::tilde(&book.path)).font(theme::body(11.5)).weak());
        if book.in_cloud {
            ui.label("In iCloud: opening it brings it down.");
        }
    });
    let mut action = None;
    if response.clicked() {
        action = Some(BooksAction::Open(book.path.clone()));
    }
    if response.hovered() && ui.input(|i| i.key_pressed(egui::Key::F2)) {
        state.rename(book);
    }
    response.context_menu(|ui| {
        if ui.button("Open").clicked() {
            action = Some(BooksAction::Open(book.path.clone()));
            ui.close();
        }
        if ui.button("Rename…").clicked() {
            state.rename(book);
            ui.close();
        }
        if book.renamed && ui.button("Its own title and author").clicked() {
            action = Some(BooksAction::Rename { path: book.path.clone(), title: String::new(), author: String::new() });
            ui.close();
        }
    });
    action
}

fn paint_card(ui: &Ui, p: &Palette, state: &BooksState, book: &Book, rect: Rect, hovered: bool) {
    let painter = ui.painter();
    let cover_box = Rect::from_min_size(rect.min, vec2(COVER_W, COVER_H));
    let radius = CornerRadius::same(6);
    // The cover sits on the shelf: fitted to the box, at its foot.
    let texture = state.covers.get(&book.path).and_then(Option::as_ref);
    let cover = match texture {
        Some(t) => {
            let [w, h] = t.size();
            let scale = (COVER_W / w as f32).min(COVER_H / h as f32);
            let size = vec2(w as f32 * scale, h as f32 * scale);
            Rect::from_min_size(
                egui::pos2(cover_box.center().x - size.x / 2.0, cover_box.bottom() - size.y),
                size,
            )
        }
        None => cover_box,
    };
    painter.add(
        egui::epaint::Shadow {
            offset: [0, if hovered { 8 } else { 4 }],
            blur: if hovered { 22 } else { 12 },
            spread: 0,
            color: p.page_shadow.gamma_multiply(if hovered { 0.9 } else { 0.6 }),
        }
        .as_shape(cover, radius),
    );
    match texture {
        Some(t) => {
            painter.add(
                egui::epaint::RectShape::filled(cover, radius, Color32::WHITE)
                    .with_texture(t.id(), Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))),
            );
        }
        None => {
            let fill: Color32 = egui::ecolor::Hsva::new(hue(&book.title), 0.38, 0.42, 1.0).into();
            painter.rect_filled(cover, radius, fill);
            let job = wrapped(&book.title, theme::strong(13.0), Color32::from_white_alpha(235), COVER_W - 24.0, 5);
            let galley = painter.layout_job(job);
            painter.galley(cover.min + vec2(12.0, 16.0), galley, Color32::WHITE);
        }
    }
    if hovered {
        painter.rect_stroke(cover, radius, Stroke::new(1.5, p.accent_alpha(0xa0)), egui::StrokeKind::Outside);
    }
    if book.in_cloud {
        let tag = Rect::from_min_size(cover.right_top() + vec2(-48.0, 6.0), vec2(42.0, 16.0));
        painter.rect_filled(tag, CornerRadius::same(8), Color32::from_black_alpha(150));
        painter.text(tag.center(), Align2::CENTER_CENTER, "iCloud", theme::strong(9.5), Color32::WHITE);
    }

    // Its title, two lines at most; who wrote it — or, said nowhere else, how
    // long it is; and how far it was read.
    let mut y = cover_box.bottom() + 10.0;
    let title = painter.layout_job(wrapped(&book.title, theme::strong(12.5), p.text, COVER_W, 2));
    // Two lines kept for every title, so what follows lines up along a shelf.
    let two = 2.0 * ui.fonts_mut(|f| f.row_height(&theme::strong(12.5)));
    painter.galley(egui::pos2(rect.left(), y), title, p.text);
    y += two + 3.0;
    let by = if !book.author.is_empty() {
        book.author.clone()
    } else if book.pages > 0 && book.page.is_none() {
        format!("{} pages", book.pages)
    } else {
        String::new()
    };
    if !by.is_empty() {
        let by = painter.layout_job(wrapped(&by, theme::body(11.5), p.text_muted, COVER_W, 1));
        painter.galley(egui::pos2(rect.left(), y), by, p.text_muted);
        y += 17.0;
    }
    if let Some(page) = book.page {
        let bar = Rect::from_min_size(egui::pos2(rect.left(), y + 4.0), vec2(COVER_W, 3.0));
        painter.rect_filled(bar, CornerRadius::same(2), p.wash(0x18));
        if let Some(done) = book.progress() {
            let filled = Rect::from_min_size(bar.min, vec2(COVER_W * done, 3.0));
            painter.rect_filled(filled, CornerRadius::same(2), p.accent);
        }
        let where_ = if book.pages > 0 {
            format!("page {} of {}", page + 1, book.pages)
        } else {
            format!("page {}", page + 1)
        };
        painter.text(egui::pos2(rect.left(), y + 11.0), Align2::LEFT_TOP, where_, theme::body(11.0), p.text_faint);
    }
}

/// Text wrapped to `width`, `rows` lines at most, cut with an ellipsis.
fn wrapped(text: &str, font: egui::FontId, color: Color32, width: f32, rows: usize) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    job
}

/// Renaming a book: what to call it, in the library only.
fn rename_sheet(ctx: &egui::Context, p: &Palette, state: &mut BooksState) -> Option<BooksAction> {
    let r = state.renaming.as_mut()?;
    let mut action = None;
    let mut close = false;
    let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(22).corner_radius(18);
    let modal = egui::Modal::new(egui::Id::new("quark-rename-book")).frame(frame).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(egui::RichText::new("Rename").font(theme::strong(15.0)).color(p.text));
        ui.add_space(10.0);
        let title = ui.add(
            egui::TextEdit::singleline(&mut r.title)
                .hint_text(r.file.as_str())
                .desired_width(f32::INFINITY),
        );
        if !r.focused {
            title.request_focus();
            r.focused = true;
        }
        ui.add_space(6.0);
        ui.add(egui::TextEdit::singleline(&mut r.author).hint_text("Author").desired_width(f32::INFINITY));
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(format!("In your library only: {} is left as it is.", r.file))
                .font(theme::body(11.5))
                .color(p.text_faint),
        );
        ui.add_space(12.0);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            if widgets::primary_button(ui, p, "Save", true).clicked() || enter {
                // A book is called something: an emptied title is its own.
                let title = r.title.trim().to_string();
                let author = if title.is_empty() { String::new() } else { r.author.trim().to_string() };
                action = Some(BooksAction::Rename { path: r.path.clone(), title, author });
                close = true;
            }
            if widgets::ghost_button(ui, p, "Cancel").clicked() {
                close = true;
            }
            if r.renamed && widgets::ghost_button(ui, p, "Its own").on_hover_text("Its title and author as the book says them").clicked() {
                action = Some(BooksAction::Rename { path: r.path.clone(), title: String::new(), author: String::new() });
                close = true;
            }
        });
    });
    if close || modal.should_close() {
        state.renaming = None;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(shelf: &str, title: &str, read: Option<u64>) -> Book {
        Book {
            path: PathBuf::from(format!("/books/{shelf}/{title}.pdf")),
            file: format!("{title}.pdf"),
            title: title.into(),
            author: String::new(),
            pages: 100,
            size: 1,
            shelf: shelf.into(),
            folder: "~/Documents/Books".into(),
            in_cloud: false,
            page: read.map(|_| 9),
            read,
            renamed: false,
        }
    }

    fn names<'a>(shelves: &'a [Shelf<'a>]) -> Vec<(&'a str, Vec<&'a str>)> {
        shelves.iter().map(|s| (s.name.as_str(), s.books.iter().map(|b| b.title.as_str()).collect())).collect()
    }

    #[test]
    fn being_read_comes_first_then_a_shelf_for_each_folder_the_librarys_own_first() {
        let books = [
            book("ML", "Pattern Recognition", Some(10)),
            book("", "Zen", None),
            book("Physics", "Feynman Lectures", None),
            book("", "Algebra", Some(30)),
            book("ML", "Deep Learning", None),
        ];
        let s = shelves(&books, "");
        assert_eq!(
            names(&s),
            [
                ("Continue reading", vec!["Algebra", "Pattern Recognition"]),
                ("Books", vec!["Algebra", "Zen"]),
                ("ML", vec!["Deep Learning", "Pattern Recognition"]),
                ("Physics", vec!["Feynman Lectures"]),
            ]
        );
        assert!(s[0].reading && !s[1].reading);
    }

    #[test]
    fn words_typed_narrow_the_shelves_and_continue_reading_steps_aside() {
        let books = [book("ML", "Deep Learning", Some(1)), book("Physics", "Feynman Lectures", None)];
        let s = shelves(&books, "deep");
        assert_eq!(names(&s), [("ML", vec!["Deep Learning"])]);
        assert!(shelves(&books, "nothing like it").is_empty());
    }

    #[test]
    fn continue_reading_holds_the_last_eight_read() {
        let books: Vec<Book> = (0..12).map(|i| book("", &format!("B{i:02}"), Some(i))).collect();
        let s = shelves(&books, "");
        let reading: Vec<&str> = s[0].books.iter().map(|b| b.title.as_str()).collect();
        assert_eq!(reading, ["B11", "B10", "B09", "B08", "B07", "B06", "B05", "B04"]);
    }

    #[test]
    fn a_cover_on_screen_is_asked_for_once_and_none_for_a_book_in_icloud() {
        let mut state = BooksState::default();
        let b = book("", "Algebra", None);
        state.want(&b);
        state.want(&b);
        assert_eq!(state.wanted, [b.path.clone()]);
        let ctx = egui::Context::default();
        state.cover_drawn(&ctx, b.path.clone(), None);
        state.wanted.clear();
        state.want(&b);
        assert!(state.wanted.is_empty(), "given, even as none, it is not asked for again");
        state.want(&Book { in_cloud: true, ..book("", "Far away", None) });
        assert!(state.wanted.is_empty());
    }
}
