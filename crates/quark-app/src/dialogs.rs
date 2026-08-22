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
use quark_ui::theme::Palette;

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
            Dialog::Confirm { title, .. } => title,
            Dialog::Message { title, .. } => title,
            Dialog::About => "About Quark",
        }
    }

    /// Draws the dialog and reports what the user did.
    pub fn show(&mut self, ctx: &Context, p: &Palette) -> DialogResult {
        let mut result = DialogResult::Pending;
        let mut open = true;

        egui::Modal::new(egui::Id::new("quark-dialog")).show(ctx, |ui| {
            ui.set_min_width(380.0);
            ui.heading(RichText::new(self.title()).size(16.0).color(p.text));
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

                Dialog::GoToPage { entry, max } => {
                    ui.label(
                        RichText::new(format!("Page number (1–{max})"))
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
                    let parsed = parse_page(entry, *max);
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

                Dialog::Confirm {
                    body,
                    confirm_label,
                    danger,
                    ..
                } => {
                    ui.label(RichText::new(body.as_str()).color(p.text).size(12.5));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let button = egui::Button::new(
                            RichText::new(confirm_label.as_str())
                                .color(if *danger { p.error } else { p.text }),
                        );
                        if ui.add(button).clicked() {
                            result = DialogResult::Confirmed;
                        }
                        if ui.button("Cancel").clicked() {
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
