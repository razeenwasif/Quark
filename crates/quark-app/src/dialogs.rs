//! Modal dialogs.
//!
//! One [`Dialog`] is open at a time, held by the application. Each variant
//! carries its own in-progress form state so that closing and reopening a
//! dialog does not resurrect half-typed input from the last time.

use std::path::PathBuf;

use egui::{Context, RichText};
use quark_core::geom::PageSize;
use quark_pdf::doc::{DocInfo, Metadata};
use quark_pdf::export::{Anchor, BatesOptions, ImageExport, ImageFormat, TextStamp};
use quark_pdf::security::{PermissionSet, SanitizeOptions};
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

/// What the user decided.
#[derive(Debug, Clone, PartialEq)]
pub enum DialogResult {
    /// Still open.
    Pending,
    Cancelled,
    OpenWithPassword(String),
    GoToPage(usize),
    SetMetadata(Box<Metadata>),
    Encrypt {
        user: String,
        owner: String,
        permissions: PermissionSet,
    },
    Sanitize(SanitizeOptions),
    Watermark(Box<TextStamp>),
    HeaderFooter(Box<TextStamp>),
    Bates(Box<BatesOptions>),
    ExportImages(ImageExport),
    InsertBlank { at: usize, size: PageSize },
    /// Split into parts: every so many pages, or at the pages given.
    Split { every: Option<u32>, at: Vec<u32> },
    /// The user confirmed a destructive action.
    Confirmed,
}

/// The dialog currently on screen.
pub enum Dialog {
    Password {
        path: PathBuf,
        wrong: bool,
        entry: String,
    },
    GoToPage {
        entry: String,
        max: usize,
        /// Each page's label, when the document has its own.
        labels: Vec<String>,
    },
    Properties {
        info: Box<DocInfo>,
        edited: Box<Metadata>,
    },
    Encrypt {
        user: String,
        owner: String,
        confirm: String,
        permissions: PermissionSet,
    },
    Sanitize {
        options: SanitizeOptions,
        found: Vec<String>,
    },
    Watermark {
        stamp: Box<TextStamp>,
    },
    HeaderFooter {
        stamp: Box<TextStamp>,
    },
    Bates {
        options: Box<BatesOptions>,
    },
    ExportImages {
        options: ImageExport,
    },
    InsertBlank {
        at: usize,
        width: String,
        height: String,
    },
    /// How to split a document of `pages` pages.
    Split {
        pages: usize,
        by_every: bool,
        every: String,
        at: String,
    },
    /// A yes/no confirmation for something irreversible.
    Confirm {
        title: String,
        body: String,
        confirm_label: String,
        danger: bool,
    },
    /// An error the user needs to see.
    Message {
        title: String,
        body: String,
        error: bool,
    },
    About,
}

impl Dialog {
    pub fn title(&self) -> &str {
        match self {
            Dialog::Password { .. } => "Password Required",
            Dialog::GoToPage { .. } => "Go to Page",
            Dialog::Properties { .. } => "Document Properties",
            Dialog::Encrypt { .. } => "Encrypt with Password",
            Dialog::Sanitize { .. } => "Remove Hidden Information",
            Dialog::Watermark { .. } => "Add Watermark",
            Dialog::HeaderFooter { .. } => "Add Header & Footer",
            Dialog::Bates { .. } => "Bates Numbering",
            Dialog::ExportImages { .. } => "Export Pages as Images",
            Dialog::InsertBlank { .. } => "Insert Blank Page",
            Dialog::Split { .. } => "Split Document",
            Dialog::Confirm { title, .. } => title,
            Dialog::Message { title, .. } => title,
            Dialog::About => "About Quark",
        }
    }

    /// Draws the dialog and reports what the user did.
    pub fn show(&mut self, ctx: &Context, p: &Palette) -> DialogResult {
        let mut result = DialogResult::Pending;
        let mut open = true;

        egui::Modal::new(egui::Id::new("quark-dialog"))
            .frame(theme::dialog(p))
            .backdrop_color(theme::scrim(p))
            .show(ctx, |ui| {
            ui.set_min_width(400.0);
            ui.label(RichText::new(self.title()).font(theme::heavy(17.0)).color(p.text));
            ui.add_space(8.0);

            match self {
                Dialog::Password {
                    path,
                    wrong,
                    entry,
                    ..
                } => {
                    ui.label(
                        RichText::new(format!(
                            "“{}” is protected.",
                            path.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        ))
                        .color(p.text_muted)
                        .size(12.0),
                    );
                    if *wrong {
                        ui.label(
                            RichText::new("That password was not accepted.")
                                .color(p.error)
                                .size(12.0),
                        );
                    }
                    ui.add_space(6.0);
                    let field = ui.add(
                        egui::TextEdit::singleline(entry)
                            .password(true)
                            .hint_text("Password")
                            .desired_width(f32::INFINITY),
                    );
                    // Focus lands in the field so the user can simply type.
                    if !field.has_focus() && ui.memory(|m| m.focused().is_none()) {
                        field.request_focus();
                    }
                    let submit =
                        field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Open").clicked() || submit {
                            result = DialogResult::OpenWithPassword(entry.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            result = DialogResult::Cancelled;
                        }
                    });
                }

                Dialog::GoToPage { entry, max, labels } => {
                    let hint = match labels.first() {
                        Some(first) => format!("Page label ({first}…) or number (1–{max})"),
                        None => format!("Page number (1–{max})"),
                    };
                    ui.label(
                        RichText::new(hint)
                            .color(p.text_muted)
                            .size(12.0),
                    );
                    let field = ui.add(
                        egui::TextEdit::singleline(entry).desired_width(f32::INFINITY),
                    );
                    if !field.has_focus() && ui.memory(|m| m.focused().is_none()) {
                        field.request_focus();
                    }
                    let submit =
                        field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let parsed = parse_page_or_label(entry, *max, labels);
                    if entry.trim().is_empty() {
                        // No complaint before anything has been typed.
                    } else if parsed.is_none() {
                        ui.label(
                            RichText::new("Enter a page number in range.")
                                .color(p.warning)
                                .size(11.0),
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let enabled = parsed.is_some();
                        if ui.add_enabled(enabled, egui::Button::new("Go")).clicked()
                            || (submit && enabled)
                        {
                            result = DialogResult::GoToPage(parsed.unwrap());
                        }
                        if ui.button("Cancel").clicked() {
                            result = DialogResult::Cancelled;
                        }
                    });
                }

                Dialog::Properties { info, edited } => {
                    egui::Grid::new("props").num_columns(2).spacing([12.0, 6.0]).show(
                        ui,
                        |ui| {
                            ui.label("Title");
                            ui.text_edit_singleline(&mut edited.title);
                            ui.end_row();
                            ui.label("Author");
                            ui.text_edit_singleline(&mut edited.author);
                            ui.end_row();
                            ui.label("Subject");
                            ui.text_edit_singleline(&mut edited.subject);
                            ui.end_row();
                            ui.label("Keywords");
                            ui.text_edit_singleline(&mut edited.keywords);
                            ui.end_row();
                        },
                    );
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);
                    let facts = [
                        ("Pages", info.page_count.to_string()),
                        ("File size", crate::panels::human_size(info.file_size as usize)),
                        ("PDF version", info.version.clone()),
                        ("Producer", info.metadata.producer.clone()),
                        (
                            "Security",
                            if info.encrypted {
                                "Password protected".into()
                            } else {
                                "None".into()
                            },
                        ),
                        ("Form fields", info.form_field_count.to_string()),
                        ("Signatures", info.signature_count.to_string()),
                        ("Attachments", info.attachment_count.to_string()),
                        (
                            "Tagged",
                            if info.tagged { "Yes".into() } else { "No".into() },
                        ),
                    ];
                    egui::Grid::new("facts").num_columns(2).spacing([12.0, 4.0]).show(
                        ui,
                        |ui| {
                            for (k, v) in facts {
                                ui.label(RichText::new(k).color(p.text_faint).size(11.0));
                                ui.label(RichText::new(v).color(p.text).size(11.0));
                                ui.end_row();
                            }
                        },
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Apply").clicked() {
                            result = DialogResult::SetMetadata(edited.clone());
                        }
                        if ui.button("Close").clicked() {
                            result = DialogResult::Cancelled;
                        }
                    });
                }

                Dialog::Encrypt {
                    user,
                    owner,
                    confirm,
                    permissions,
                } => {
                    ui.label(
                        RichText::new(
                            "The open password is required to view the document. \
                             The permissions password is required to change these settings.",
                        )
                        .color(p.text_muted)
                        .size(11.0),
                    );
                    ui.add_space(6.0);
                    egui::Grid::new("enc").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Open password");
                        ui.add(egui::TextEdit::singleline(user).password(true));
                        ui.end_row();
                        ui.label("Confirm");
                        ui.add(egui::TextEdit::singleline(confirm).password(true));
                        ui.end_row();
                        ui.label("Permissions password");
                        ui.add(egui::TextEdit::singleline(owner).password(true));
                        ui.end_row();
                    });

                    let mismatch = user != confirm;
                    if mismatch {
                        ui.label(
                            RichText::new("The passwords do not match.")
                                .color(p.error)
                                .size(11.0),
                        );
                    }

                    ui.add_space(6.0);
                    ui.label(RichText::new("Allow").color(p.text_faint).size(11.0));
                    ui.checkbox(&mut permissions.print, "Printing");
                    ui.checkbox(&mut permissions.copy, "Copying text and images");
                    ui.checkbox(&mut permissions.modify, "Changing the document");
                    ui.checkbox(&mut permissions.annotate, "Commenting");
                    ui.checkbox(&mut permissions.fill_forms, "Filling in form fields");
                    ui.checkbox(&mut permissions.assemble, "Assembling pages");

                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "PDF permissions are advisory. Any reader can ignore them; \
                             only the open password actually restricts access.",
                        )
                        .color(p.warning)
                        .size(10.5),
                    );

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!mismatch, egui::Button::new("Encrypt"))
                            .clicked()
                        {
                            result = DialogResult::Encrypt {
                                user: user.clone(),
                                owner: owner.clone(),
                                permissions: *permissions,
                            };
                        }
                        if ui.button("Cancel").clicked() {
                            result = DialogResult::Cancelled;
                        }
                    });
                }

                Dialog::Sanitize { options, found } => {
                    if found.is_empty() {
                        ui.label(
                            RichText::new("No hidden information was found.")
                                .color(p.text_muted)
                                .size(12.0),
                        );
                    } else {
                        ui.label(
                            RichText::new("This document contains:")
                                .color(p.text_muted)
                                .size(12.0),
                        );
                        for f in found.iter() {
                            ui.label(RichText::new(format!("  • {f}")).size(11.5));
                        }
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("Remove").color(p.text_faint).size(11.0));
                    ui.checkbox(&mut options.metadata, "Document properties and XMP metadata");
                    ui.checkbox(&mut options.javascript, "JavaScript and open actions");
                    ui.checkbox(&mut options.embedded_files, "Embedded files");
                    ui.checkbox(&mut options.hidden_layers, "Hidden layers");
                    ui.checkbox(&mut options.annotations, "Comments and markup");
                    ui.checkbox(&mut options.form_fields, "Form fields");
                    ui.checkbox(&mut options.bookmarks, "Bookmarks");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Remove").clicked() {
                            result = DialogResult::Sanitize(*options);
                        }
                        if ui.button("Cancel").clicked() {
                            result = DialogResult::Cancelled;
                        }
                    });
                }

                Dialog::Watermark { stamp } => {
                    stamp_form(ui, p, stamp, false, &mut result);
                }

                Dialog::HeaderFooter { stamp } => {
                    stamp_form(ui, p, stamp, true, &mut result);
                }

                Dialog::Bates { options } => {
                    egui::Grid::new("bates").num_columns(2).spacing([10.0, 6.0]).show(
                        ui,
                        |ui| {
                            ui.label("Prefix");
                            ui.text_edit_singleline(&mut options.prefix);
                            ui.end_row();
                            ui.label("Suffix");
                            ui.text_edit_singleline(&mut options.suffix);
                            ui.end_row();
                            ui.label("Start at");
                            ui.add(egui::DragValue::new(&mut options.start).range(0..=u64::MAX));
                            ui.end_row();
                            ui.label("Digits");
                            ui.add(egui::DragValue::new(&mut options.digits).range(1..=12));
                            ui.end_row();
                        },
                    );
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("Preview: {}", options.label(0)))
                            .color(p.accent_text)
                            .size(12.0),
                    );
                    egui::ComboBox::from_label("Position")
                        .selected_text(options.anchor.label())
                        .show_ui(ui, |ui| {
                            for a in Anchor::ALL {
                                ui.selectable_value(&mut options.anchor, a, a.label());
                            }
                        });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Apply").clicked() {
                            result = DialogResult::Bates(options.clone());
                        }
                        if ui.button("Cancel").clicked() {
                            result = DialogResult::Cancelled;
                        }
                    });
                }

                Dialog::ExportImages { options } => {
                    egui::ComboBox::from_label("Format")
                        .selected_text(options.format.label())
                        .show_ui(ui, |ui| {
                            for f in ImageFormat::ALL {
                                ui.selectable_value(&mut options.format, f, f.label());
                            }
                        });
                    ui.add(egui::Slider::new(&mut options.dpi, 36.0..=600.0).text("Resolution (dpi)"));
                    ui.checkbox(&mut options.annotations, "Include comments and markup");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Choose folder…").clicked() {
                            result = DialogResult::ExportImages(*options);
                        }
                        if ui.button("Cancel").clicked() {
                            result = DialogResult::Cancelled;
                        }
                    });
                }

                Dialog::InsertBlank { at, width, height } => {
                    ui.label(
                        RichText::new(format!("Insert before page {}", *at + 1))
                            .color(p.text_muted)
                            .size(12.0),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Width");
                        ui.add(egui::TextEdit::singleline(width).desired_width(70.0));
                        ui.label("Height");
                        ui.add(egui::TextEdit::singleline(height).desired_width(70.0));
                        ui.label(RichText::new("points").color(p.text_faint).size(11.0));
                    });
                    let size = parse_size(width, height);
                    if size.is_none() {
                        ui.label(
                            RichText::new("Enter positive page dimensions.")
                                .color(p.warning)
                                .size(11.0),
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(size.is_some(), egui::Button::new("Insert"))
                            .clicked()
                        {
                            result = DialogResult::InsertBlank {
                                at: *at,
                                size: size.unwrap(),
                            };
                        }
                        if ui.button("Cancel").clicked() {
                            result = DialogResult::Cancelled;
                        }
                    });
                }

                Dialog::Split { pages, by_every, every, at } => {
                    ui.label(
                        RichText::new(format!("Into new files, side by side in a folder you choose. {pages} pages."))
                            .color(p.text_muted)
                            .size(12.0),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.radio_value(by_every, true, "Every");
                        ui.add_enabled(*by_every, egui::TextEdit::singleline(every).desired_width(48.0));
                        ui.label("pages");
                    });
                    ui.horizontal(|ui| {
                        ui.radio_value(by_every, false, "At pages");
                        ui.add_enabled(
                            !*by_every,
                            egui::TextEdit::singleline(at).hint_text("e.g. 5, 12").desired_width(160.0),
                        );
                    });
                    let parsed = parse_split(*by_every, every, at, *pages);
                    ui.add_space(4.0);
                    match &parsed {
                        Ok((_, _, parts)) => {
                            ui.label(RichText::new(format!("Makes {parts} files.")).color(p.text_faint).size(11.5));
                        }
                        Err(why) => {
                            ui.label(RichText::new(why.as_str()).color(p.warning).size(11.5));
                        }
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(parsed.is_ok(), egui::Button::new("Choose Folder…")).clicked() {
                            if let Ok((every, at, _)) = parsed {
                                result = DialogResult::Split { every, at };
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            result = DialogResult::Cancelled;
                        }
                    });
                }

                Dialog::Confirm {
                    body,
                    confirm_label,
                    danger,
                    ..
                } => {
                    ui.label(
                        RichText::new(body.as_str())
                            .font(theme::body(13.0))
                            .color(p.text_muted),
                    );
                    ui.add_space(14.0);
                    // Right to left, so the confirming action sits at the
                    // far edge where the eye ends up.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let confirmed = if *danger {
                            widgets::danger_button(ui, p, confirm_label.as_str())
                        } else {
                            widgets::primary_button(ui, p, confirm_label.as_str(), true)
                        };
                        if confirmed.clicked() {
                            result = DialogResult::Confirmed;
                        }
                        if widgets::text_button(ui, p, "Cancel", false).clicked() {
                            result = DialogResult::Cancelled;
                        }
                    });
                }

                Dialog::Message { body, error, .. } => {
                    ui.label(
                        RichText::new(body.as_str())
                            .color(if *error { p.error } else { p.text })
                            .size(12.5),
                    );
                    ui.add_space(10.0);
                    if ui.button("OK").clicked() {
                        result = DialogResult::Cancelled;
                    }
                }

                Dialog::About => {
                    ui.label(RichText::new("Quark").size(22.0).color(p.accent_text));
                    ui.label(
                        RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                            .color(p.text_muted)
                            .size(12.0),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("A PDF reader and editor for Windows.")
                            .color(p.text)
                            .size(12.0),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!(
                            "Rendering: PDFium\nEngine path: {}",
                            quark_pdf::engine::loaded_from()
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|| "not loaded".into())
                        ))
                        .color(p.text_faint)
                        .size(10.5),
                    );
                    ui.add_space(10.0);
                    if ui.button("Close").clicked() {
                        result = DialogResult::Cancelled;
                    }
                }
            }
        });

        // Escape closes any dialog, which is expected everywhere.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        if !open && result == DialogResult::Pending {
            result = DialogResult::Cancelled;
        }
        result
    }
}


/// The form shared by the watermark and header/footer dialogs.
///
/// They differ only in whether rotation is offered and which result they
/// produce, so they share one body rather than two that drift apart.
fn stamp_form(
    ui: &mut egui::Ui,
    p: &Palette,
    stamp: &mut TextStamp,
    is_header: bool,
    result: &mut DialogResult,
) {
    ui.label(
        RichText::new(
            "Tokens: <<n>> page number, <<N>> page count, \
             <<date>> today, <<file>> file name.",
        )
        .color(p.text_faint)
        .size(10.5),
    );
    ui.add_space(6.0);
    ui.add(
        egui::TextEdit::singleline(&mut stamp.text)
            .hint_text("Text")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(6.0);
    ui.add(egui::Slider::new(&mut stamp.font_size, 6.0..=144.0).text("Size"));
    ui.add(egui::Slider::new(&mut stamp.opacity, 0.05..=1.0).text("Opacity"));
    // A rotated running header would collide with the page content; only a
    // watermark is offered the option.
    if !is_header {
        ui.add(egui::Slider::new(&mut stamp.rotation, -90.0..=90.0).text("Rotation"));
    }
    egui::ComboBox::from_label("Position")
        .selected_text(stamp.anchor.label())
        .show_ui(ui, |ui| {
            for a in Anchor::ALL {
                ui.selectable_value(&mut stamp.anchor, a, a.label());
            }
        });
    ui.checkbox(&mut stamp.behind, "Draw behind the page content");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Apply").clicked() {
            *result = if is_header {
                DialogResult::HeaderFooter(Box::new(stamp.clone()))
            } else {
                DialogResult::Watermark(Box::new(stamp.clone()))
            };
        }
        if ui.button("Cancel").clicked() {
            *result = DialogResult::Cancelled;
        }
    });
}

/// Parses a one-based page number, returning a zero-based index.
/// A split as typed: every so many pages, or a part starting at each page
/// given (from 1), and how many files it makes; or why it cannot be.
pub fn parse_split(by_every: bool, every: &str, at: &str, pages: usize) -> Result<(Option<u32>, Vec<u32>, usize), String> {
    if by_every {
        let n: usize = every.trim().parse().map_err(|_| "Say how many pages each file holds.".to_string())?;
        if n == 0 {
            return Err("Each file holds one page or more.".into());
        }
        if n >= pages {
            return Err(format!("Every {n} pages makes one file: the document as it is."));
        }
        return Ok((Some(n as u32), Vec::new(), pages.div_ceil(n)));
    }
    let mut starts = Vec::new();
    for word in at.split(|c: char| c == ',' || c.is_whitespace()).filter(|w| !w.is_empty()) {
        let n: usize = word.parse().map_err(|_| format!("“{word}” is not a page number."))?;
        if n < 2 || n > pages {
            return Err(format!("A part can start at pages 2 to {pages}."));
        }
        starts.push(n as u32);
    }
    starts.sort_unstable();
    starts.dedup();
    if starts.is_empty() {
        return Err("Say the pages new files start at.".into());
    }
    let parts = starts.len() + 1;
    Ok((None, starts, parts))
}

/// Pages (from 0) as a person says them: 3–5, or 1, 4, 7–9.
pub fn pages_said(pages: &[usize]) -> String {
    let mut sorted: Vec<usize> = pages.iter().map(|p| p + 1).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut runs: Vec<String> = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        runs.push(if end == start { start.to_string() } else { format!("{start}–{end}") });
        i += 1;
    }
    runs.join(", ")
}

/// A page by its label — the document's own, when it has them — or else by
/// its number. A book whose page 1 is the fifth page goes to the fifth.
pub fn parse_page_or_label(entry: &str, max: usize, labels: &[String]) -> Option<usize> {
    let wanted = entry.trim();
    labels
        .iter()
        .position(|l| !wanted.is_empty() && l.eq_ignore_ascii_case(wanted))
        .or_else(|| parse_page(entry, max))
}

pub fn parse_page(entry: &str, max: usize) -> Option<usize> {
    let n: usize = entry.trim().parse().ok()?;
    if n == 0 || n > max {
        return None;
    }
    Some(n - 1)
}

/// Parses page dimensions in points.
pub fn parse_size(width: &str, height: &str) -> Option<PageSize> {
    let w: f32 = width.trim().parse().ok()?;
    let h: f32 = height.trim().parse().ok()?;
    // A zero or negative page is not a page, and PDFium will reject it.
    if !(w.is_finite() && h.is_finite()) || w <= 1.0 || h <= 1.0 {
        return None;
    }
    Some(PageSize::new(w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_entry_is_one_based_and_range_checked() {
        assert_eq!(parse_page("1", 10), Some(0));
        assert_eq!(parse_page("10", 10), Some(9));
        assert_eq!(parse_page("0", 10), None, "there is no page zero");
        assert_eq!(parse_page("11", 10), None, "past the end");
        assert_eq!(parse_page("abc", 10), None);
        assert_eq!(parse_page("", 10), None);
    }

    #[test]
    fn page_entry_tolerates_surrounding_whitespace() {
        assert_eq!(parse_page("  4 ", 10), Some(3));
        assert_eq!(pages_said(&[4, 2, 3]), "3–5");
        assert_eq!(pages_said(&[0, 3, 6, 7, 8]), "1, 4, 7–9");
        assert_eq!(parse_split(true, "2", "", 5), Ok((Some(2), vec![], 3)));
        assert_eq!(parse_split(false, "", "12, 5 5", 20), Ok((None, vec![5, 12], 3)));
        assert!(parse_split(true, "5", "", 5).is_err(), "one file is the document as it is");
        assert!(parse_split(true, "0", "", 5).is_err());
        assert!(parse_split(false, "", "1", 5).is_err(), "page 1 starts the first file already");
        assert!(parse_split(false, "", "9", 5).is_err());
        assert!(parse_split(false, "", "two", 5).is_err());
        assert!(parse_split(false, "", " ", 5).is_err());
        let labels: Vec<String> = ["i", "ii", "1", "2"].map(String::from).to_vec();
        assert_eq!(parse_page_or_label("II", 4, &labels), Some(1));
        assert_eq!(parse_page_or_label("1", 4, &labels), Some(2), "the label, not the first page");
        assert_eq!(parse_page_or_label("4", 4, &labels), Some(3), "no such label: its number");
        assert_eq!(parse_page_or_label("", 4, &labels), None);
    }

    #[test]
    fn page_sizes_must_be_positive_and_finite() {
        assert!(parse_size("612", "792").is_some());
        assert!(parse_size("0", "792").is_none());
        assert!(parse_size("-5", "792").is_none());
        assert!(parse_size("inf", "792").is_none());
        assert!(parse_size("abc", "792").is_none());
    }

    #[test]
    fn a_parsed_size_keeps_its_dimensions() {
        let s = parse_size("595", "842").unwrap();
        assert_eq!(s.width, 595.0);
        assert_eq!(s.height, 842.0);
    }

    #[test]
    fn dialog_titles_are_never_empty() {
        let dialogs = [
            Dialog::GoToPage {
                entry: String::new(),
                max: 1,
                labels: Vec::new(),
            },
            Dialog::About,
            Dialog::Confirm {
                title: "Apply Redactions".into(),
                body: String::new(),
                confirm_label: "Apply".into(),
                danger: true,
            },
        ];
        for d in dialogs {
            assert!(!d.title().is_empty());
        }
    }
}
