//! Window chrome and the frame loop: menu bar, toolbar, tabs, panels, status
//! bar, command palette, and the dispatch that turns a [`Command`] into work.

use std::path::PathBuf;

use egui::{Context, RichText};
use quark_core::command::{Command, Menu, command_table};
use quark_core::geom::{Rot, Vec2};
use quark_core::layout::{self, ZoomMode};
use quark_core::prefs::SidePanel;
use quark_core::tools::Tool;
use quark_pdf::export::{BatesOptions, ImageExport, TextStamp};
use quark_pdf::security::{RedactionMark, SanitizeOptions};
use quark_pdf::service::{DocOp, Request};
use quark_ui::icons::Icon;
use quark_ui::theme::{self, Palette};
use quark_ui::widgets;

use crate::app::{App, Dock, save_prefs};
use crate::assistant;

/// Shown by Help. Deliberately short: the command palette is the real index.
const HELP_TEXT: &str = "\
Ctrl+O opens a document, Ctrl+F searches it, and Ctrl+Shift+P opens the \
command palette, which lists every command Quark has along with its shortcut.

Scroll with the wheel, zoom with Ctrl+wheel, and pan by holding space or the \
middle button. The toolbar's markup tools annotate the page; the Comments \
panel lists everything that has been added.

Page Thumbnails supports drag-to-reorder and a right-click menu for rotating \
and deleting pages.";
use crate::dialogs::{Dialog, DialogResult};
use crate::panels::{self, PanelAction};
use crate::shortcuts;
use crate::viewer::{self, ViewerAction};

impl App {
    // ---------------------------------------------------------------- chrome

    fn menu_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // Panels are drawn into the parent `Ui` in this version of egui; the
        // context is still needed for dispatch, and cloning it is just an Arc.
        let ctx = &ui.ctx().clone();
        let table = command_table();
        egui::Panel::top("menu")
            .exact_size(28.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(14, 2)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Quark")
                            .size(13.0)
                            .strong()
                            .color(p.accent_text),
                    );
                    ui.add_space(8.0);
                    for menu in Menu::ALL {
                        ui.menu_button(menu.label(), |ui| {
                            ui.set_min_width(230.0);
                            for info in table.iter().filter(|c| c.menu == menu) {
                                let enabled = self.command_enabled(&info.command, info.mutates);
                                let label = if info.shortcut.is_empty() {
                                    info.label.to_string()
                                } else {
                                    format!("{}\t{}", info.label, info.shortcut)
                                };
                                if ui
                                    .add_enabled(enabled, egui::Button::new(label))
                                    .clicked()
                                {
                                    self.dispatch(info.command.clone(), ctx);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
            });
    }

    /// Whether a command can run right now.
    fn command_enabled(&self, cmd: &Command, mutates: bool) -> bool {
        let Some(tab) = self.tab() else {
            // With nothing open, only the commands that do not need a document
            // are available.
            return matches!(
                cmd,
                Command::Open
                    | Command::Quit
                    | Command::Preferences
                    | Command::About
                    | Command::Help
                    | Command::NewFromBlank
                    | Command::NewFromImages
                    | Command::CombineFiles
                    | Command::CommandPalette
                    | Command::ToggleTheme
            ) || matches!(cmd, Command::OpenRecent(_));
        };
        match cmd {
            Command::Undo => tab.history.can_undo(),
            Command::Redo => tab.history.can_redo(),
            Command::GoBack => tab.can_go_back(),
            Command::GoForward => tab.can_go_forward(),
            Command::FindNext | Command::FindPrev => !tab.search.is_empty(),
            Command::Save => tab.has_unsaved_changes(),
            Command::RemoveSecurity => tab.info.encrypted,
            Command::EncryptWithPassword => !tab.info.encrypted,
            Command::ApplyRedactions => tab
                .all_annotations()
                .iter()
                .any(|a| matches!(a.kind, quark_core::annot::AnnotKind::Redact { .. })),
            Command::ExtractAttachment => tab.info.attachment_count > 0,
            Command::FlattenForm
            | Command::ExportFormData
            | Command::ImportFormData
            | Command::ClearForm => tab.info.has_form,
            // Permissions are advisory, but honouring them is what a user who
            // set them expects to see.
            _ if mutates => tab.info.permissions.modify_contents,
            _ => true,
        }
    }

    /// The far-left tool rail.
    ///
    /// Dark in both themes: it reads as the frame of the application rather
    /// than as another surface, which is what keeps the four light columns
    /// beside it from blurring into one.
    /// Drains the assistant worker into the conversation.
    ///
    /// Called every frame from `update`, like the PDF service drain: a streamed
    /// answer only appears if something is pulling events off the channel.
    pub(crate) fn drain_assistant(&mut self, ctx: &Context) {
        let mut got_any = false;
        while let Some(event) = self.ai.service.try_recv() {
            got_any = true;
            match event {
                quark_ai::AiEvent::Delta { id, delta } => self.ai.apply(id, delta),
                quark_ai::AiEvent::Failed { id, message, .. } => self.ai.fail(id, message),
                quark_ai::AiEvent::Models { names } => {
                    // A fresh install points at a default model the user may
                    // well not have pulled, and the only symptom is a 404 when
                    // they finally ask something. Adopting the first real model
                    // fixes that — but only when the current value is still the
                    // untouched default, so a deliberate choice is never
                    // overridden.
                    let backend = assistant::backend_of(&self.prefs);
                    let untouched = self.prefs.ai_model == backend.default_model();
                    if untouched && !names.is_empty() && !names.contains(&self.prefs.ai_model) {
                        self.prefs.ai_model = names[0].clone();
                    }
                    self.ai.models = names;
                }
            }
        }
        // A streaming answer is the one thing in Quark that changes without
        // input, so it needs a repaint asked for explicitly.
        if got_any || self.ai.in_flight.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
    }

    /// Builds the request and hands it to the worker.
    fn send_question(&mut self) {
        let question = std::mem::take(&mut self.ai.draft).trim().to_string();
        if question.is_empty() {
            return;
        }
        let backend = assistant::backend_of(&self.prefs);
        let scope = assistant::scope_of(&self.prefs);

        // Assemble the document context from the open tab.
        let mut doc = quark_ai::DocContext::default();
        if let Some(tab) = self.tab() {
            doc.title = tab.title();
            doc.current_page = tab.current_page;
            doc.page_count = tab.page_count();
            doc.selection = tab.selected_text();
            let range = scope.pages(doc.current_page, doc.page_count);
            doc.pages = range
                .filter_map(|i| tab.text(i).map(|t| (i, t.text.clone())))
                .collect();
        }

        let id = self.ai.service.new_turn();
        let history = {
            let mut h = self.ai.history();
            h.push(quark_ai::Message::user(question.clone()));
            h
        };
        let request = quark_ai::context::build_request(
            self.prefs.ai_model.clone(),
            &doc,
            history,
            // Generous, and streamed, so a long answer is not truncated by a
            // ceiling the user never chose.
            8192,
        );

        // The credential is fetched per turn rather than held: it is then never
        // resident in Quark's memory between questions.
        let provider: Box<dyn quark_ai::Provider> = match backend {
            quark_ai::Backend::Ollama => Box::new(quark_ai::Ollama::local()),
            quark_ai::Backend::Anthropic => {
                Box::new(quark_ai::Anthropic::new(self.stored_key(backend)))
            }
            quark_ai::Backend::OpenAiCompat => Box::new(quark_ai::OpenAiCompat::new(
                "OpenAI-compatible",
                self.prefs.ai_base_url.clone(),
                self.stored_key(backend),
            )),
        };

        self.ai.begin(id, question);
        self.ai.service.ask(id, provider, request);
    }

    /// Reads a backend's key out of the OS credential store.
    ///
    /// An environment variable wins when set, which is how the Anthropic SDKs
    /// resolve credentials and makes it easy to override for a one-off test
    /// without touching what is stored.
    fn stored_key(&self, backend: quark_ai::Backend) -> String {
        let env_name = match backend {
            quark_ai::Backend::Anthropic => "ANTHROPIC_API_KEY",
            quark_ai::Backend::OpenAiCompat => "OPENAI_API_KEY",
            quark_ai::Backend::Ollama => return String::new(),
        };
        if let Ok(k) = std::env::var(env_name) {
            if !k.trim().is_empty() {
                return k;
            }
        }
        backend
            .credential_key()
            .and_then(|t| quark_shell::load_secret(t).ok().flatten())
            .unwrap_or_default()
    }

    /// The assistant dock.
    fn assistant_panel(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let backend = assistant::backend_of(&self.prefs);

        // A cloud backend sends document text off the machine. Said once, up
        // front, rather than buried in settings — and not at all for Ollama,
        // where it would be untrue.
        if backend.leaves_the_machine() && !self.ai.egress_acknowledged {
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "{} runs in the cloud. Asking a question sends the pages in \
                     scope, and any selected text, to it.",
                    backend.label()
                ))
                .size(11.5)
                .color(p.text_muted),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if widgets::text_button(ui, p, "I understand", false).clicked() {
                    self.ai.egress_acknowledged = true;
                }
                if widgets::text_button(ui, p, "Use Ollama instead", false).clicked() {
                    self.prefs.ai_backend = assistant::backend_key(quark_ai::Backend::Ollama).into();
                    self.prefs.ai_model = quark_ai::Backend::Ollama.default_model().into();
                }
            });
            return;
        }

        // --- backend and model ---
        //
        // The model list is fetched once per backend, through the worker: for
        // Ollama it is an HTTP call, and asking on every frame would be sixty
        // requests a second.
        if self.ai.models_requested_for != Some(backend) {
            self.ai.models_requested_for = Some(backend);
            self.ai.models.clear();
            let lister: Box<dyn quark_ai::Provider> = match backend {
                quark_ai::Backend::Ollama => Box::new(quark_ai::Ollama::local()),
                quark_ai::Backend::Anthropic => Box::new(quark_ai::Anthropic::new(String::new())),
                quark_ai::Backend::OpenAiCompat => Box::new(quark_ai::OpenAiCompat::new(
                    "OpenAI-compatible",
                    self.prefs.ai_base_url.clone(),
                    String::new(),
                )),
            };
            self.ai.service.list_models(lister);
        }

        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("ai-backend")
                .selected_text(RichText::new(backend.label()).size(11.5))
                .width(108.0)
                .show_ui(ui, |ui| {
                    for b in quark_ai::Backend::ALL {
                        if ui.selectable_label(b == backend, b.label()).clicked() && b != backend {
                            self.prefs.ai_backend = assistant::backend_key(b).into();
                            // The old model name means nothing to the new
                            // backend, so it cannot be carried across.
                            self.prefs.ai_model = b.default_model().into();
                        }
                    }
                });

            let model = self.prefs.ai_model.clone();
            egui::ComboBox::from_id_salt("ai-model")
                .selected_text(RichText::new(widgets::elide(&model, 18, true)).size(11.5))
                .width(ui.available_width().max(60.0))
                .show_ui(ui, |ui| {
                    if self.ai.models.is_empty() {
                        ui.label(
                            RichText::new(match backend {
                                quark_ai::Backend::Ollama => "No models found — is Ollama running?",
                                _ => "Type a model name in Settings.",
                            })
                            .size(11.0)
                            .color(p.text_faint),
                        );
                    }
                    for name in &self.ai.models {
                        if ui.selectable_label(*name == model, name).clicked() {
                            self.prefs.ai_model = name.clone();
                        }
                    }
                });
        });
        ui.add_space(4.0);

        // --- credentials ---
        //
        // Re-checked only when the backend changes: reading the credential
        // store is a syscall, and doing it every frame is sixty a second.
        if self.ai.key_state.map(|(b, _)| b) != Some(backend) {
            self.ai.key_state = Some((backend, assistant::key_source(backend)));
        }
        let source = self.ai.key_state.map(|(_, s)| s).unwrap_or(assistant::KeySource::Missing);

        if backend.leaves_the_machine() {
            let (label, colour) = match source {
                assistant::KeySource::Environment => (
                    format!("Key from {}", assistant::env_var(backend).unwrap_or("")),
                    p.success,
                ),
                assistant::KeySource::Stored => ("Key stored".to_string(), p.success),
                assistant::KeySource::Missing => ("No API key set".to_string(), p.warning),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(11.0).color(colour));
                if widgets::text_button(
                    ui,
                    p,
                    if self.ai.settings_open { "Hide" } else { "Set up" },
                    self.ai.settings_open,
                )
                .clicked()
                {
                    self.ai.settings_open = !self.ai.settings_open;
                    self.ai.key_input.clear();
                }
            });
            ui.add_space(4.0);
        }

        if self.ai.settings_open && backend.leaves_the_machine() {
            // Base URL, for the one backend that needs to be pointed somewhere.
            if backend == quark_ai::Backend::OpenAiCompat {
                ui.label(RichText::new("Endpoint").size(10.5).color(p.text_faint));
                ui.horizontal_wrapped(|ui| {
                    for (name, url) in assistant::BASE_URL_PRESETS {
                        let active = self.prefs.ai_base_url == url;
                        if widgets::text_button(ui, p, name, active).clicked() {
                            self.prefs.ai_base_url = url.into();
                        }
                    }
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.prefs.ai_base_url)
                        .desired_width(f32::INFINITY)
                        .font(egui::FontId::proportional(11.0)),
                );
                ui.add_space(6.0);

                // These endpoints cannot be enumerated — a proxy serves models
                // under whatever names its operator chose — so the model is
                // typed rather than picked.
                ui.label(RichText::new("Model").size(10.5).color(p.text_faint));
                ui.add(
                    egui::TextEdit::singleline(&mut self.prefs.ai_model)
                        .desired_width(f32::INFINITY)
                        .hint_text("gpt-4o-mini, gemini-2.5-flash, …")
                        .font(egui::FontId::proportional(11.0)),
                );
                ui.add_space(6.0);
            }

            ui.label(RichText::new("API key").size(10.5).color(p.text_faint));
            // Masked, and a stored key is never read back into the field: the
            // credential store is write-and-use.
            ui.add(
                egui::TextEdit::singleline(&mut self.ai.key_input)
                    .password(true)
                    .desired_width(f32::INFINITY)
                    .hint_text(match source {
                        assistant::KeySource::Stored => "Replace the stored key",
                        _ => "Paste your key",
                    })
                    .font(egui::FontId::proportional(11.0)),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let typed = !self.ai.key_input.trim().is_empty();
                if widgets::text_button(ui, p, "Save", false).clicked() && typed {
                    if let Some(target) = backend.credential_key() {
                        match quark_shell::store_secret(target, self.ai.key_input.trim()) {
                            Ok(()) => {
                                self.ai.key_input.clear();
                                self.ai.key_state = None;
                                self.ai.settings_open = false;
                                self.toasts.push(crate::app::Toast {
                                    text: format!("Key saved for {}", backend.label()),
                                    error: false,
                                    ttl: 3.5,
                                });
                            }
                            Err(e) => self.toasts.push(crate::app::Toast {
                                text: format!("Could not save the key: {e}"),
                                error: true,
                                ttl: 8.0,
                            }),
                        }
                    }
                }
                if source == assistant::KeySource::Stored
                    && widgets::text_button(ui, p, "Remove", false).clicked()
                {
                    if let Some(target) = backend.credential_key() {
                        let _ = quark_shell::delete_secret(target);
                        self.ai.key_state = None;
                    }
                }
            });
            if source == assistant::KeySource::Environment {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "The environment variable takes priority, so a saved key \
                         will not be used until it is unset.",
                    )
                    .size(10.5)
                    .color(p.text_faint),
                );
            }
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);
        }

        // The configured model has to exist, or the failure only shows up as a
        // 404 at the moment a question is asked.
        if !self.ai.models.is_empty() && !self.ai.models.contains(&self.prefs.ai_model) {
            ui.label(
                RichText::new(format!(
                    "\u{201c}{}\u{201d} is not installed. Pick one above.",
                    self.prefs.ai_model
                ))
                .size(11.0)
                .color(p.warning),
            );
            ui.add_space(4.0);
        }

        // --- how much of the document goes with the question ---
        //
        // Kept in the panel rather than buried in settings: it changes what is
        // sent and what it costs, so it belongs where the question is asked.
        ui.horizontal(|ui| {
            let current = assistant::scope_of(&self.prefs);
            for scope in quark_ai::Scope::ALL {
                let label = match scope {
                    quark_ai::Scope::Page => "Page",
                    quark_ai::Scope::Nearby => "Nearby",
                    quark_ai::Scope::Whole => "Whole",
                };
                if widgets::text_button(ui, p, label, current == scope).clicked() {
                    self.prefs.ai_scope = assistant::scope_key(scope).into();
                }
            }
        });
        ui.add_space(4.0);

        // --- conversation ---
        let composer = 74.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .max_height((ui.available_height() - composer).max(60.0))
            .show(ui, |ui| {
                if self.ai.turns.is_empty() {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new("Ask about the document.")
                            .size(12.0)
                            .color(p.text_muted),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!(
                            "{} · {}",
                            backend.label(),
                            assistant::scope_of(&self.prefs).label()
                        ))
                        .size(11.0)
                        .color(p.text_faint),
                    );
                    return;
                }
                for turn in &self.ai.turns {
                    ui.add_space(8.0);
                    let who = if turn.mine { "You" } else { backend.label() };
                    ui.label(
                        RichText::new(who)
                            .size(10.5)
                            .color(if turn.mine { p.accent_text } else { p.text_faint }),
                    );
                    if !turn.thinking.is_empty() {
                        egui::CollapsingHeader::new(
                            RichText::new("Reasoning").size(10.5).color(p.text_faint),
                        )
                        .id_salt(turn as *const _ as usize)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(&turn.thinking)
                                    .size(11.0)
                                    .color(p.text_faint),
                            );
                        });
                    }
                    if !turn.text.is_empty() {
                        ui.label(RichText::new(&turn.text).size(12.0).color(p.text));
                    }
                    if let Some(note) = &turn.note {
                        ui.label(RichText::new(note).size(11.0).color(p.warning));
                    }
                }
                ui.add_space(8.0);
            });

        // --- composer ---
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        let busy = self.ai.in_flight.is_some();
        let mut send = false;
        ui.horizontal(|ui| {
            let width = ui.available_width() - 34.0;
            let field = ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut self.ai.draft)
                    .desired_width(width)
                    .hint_text("Ask about this document"),
            );
            // Enter sends, so the composer behaves like every other chat box.
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                send = true;
            }
            if busy {
                if widgets::tool_button(ui, p, Icon::Close, "Stop", true, true).clicked() {
                    self.ai.service.cancel();
                }
            } else if widgets::tool_button(
                ui,
                p,
                Icon::ChevronRight,
                "Send",
                false,
                !self.ai.draft.trim().is_empty(),
            )
            .clicked()
            {
                send = true;
            }
        });
        if send && self.ai.can_send() {
            self.send_question();
        }
    }

    fn tool_rail(&mut self, ui: &mut egui::Ui, p: &Palette) {
        egui::Panel::left("rail")
            .exact_size(theme::RAIL_WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(p.rail)
                    .inner_margin(egui::Margin::symmetric(8, 9)),
            )
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    let has_doc = self.tab().is_some();
                    for (tool, icon, tip) in TOOL_RAIL {
                        let active = has_doc && self.tool == tool;
                        if widgets::rail_button(ui, p, icon, tip, active, has_doc).clicked() {
                            self.tool = tool;
                        }
                    }
                });
            });
    }

    /// The navigation sidebar: which structural panel the list column shows.
    fn nav_sidebar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = &ui.ctx().clone();
        egui::Panel::left("nav")
            .exact_size(theme::NAV_WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(p.card)
                    .inner_margin(egui::Margin::symmetric(9, 11)),
            )
            .show(ui, |ui| {
                theme::hairline(ui.painter(), ui.max_rect(), p, theme::Edge::Right);
                ui.spacing_mut().item_spacing.y = 2.0;

                let title = self
                    .tab()
                    .map(|t| widgets::elide(&t.title(), 18, true))
                    .unwrap_or_else(|| "Quark".into());
                widgets::nav_title(ui, p, &title);
                ui.add_space(9.0);

                for (panel, icon, label) in NAV_PANELS {
                    // Selecting the panel already showing closes the column,
                    // which is the only way to give the document full width.
                    let active = self.prefs.side_panel == panel;
                    if widgets::nav_item(ui, p, icon, label, active).clicked() {
                        self.prefs.side_panel = if active { SidePanel::None } else { panel };
                    }
                }

                // The page count sits at the foot of the rail in the reference,
                // where it is out of the way but always readable.
                let footer = self
                    .tab()
                    .map(|t| format!("{} pages", t.page_count().max(1)))
                    .unwrap_or_default();
                if !footer.is_empty() {
                    let avail = ui.available_height();
                    ui.add_space((avail - 20.0).max(0.0));
                    ui.label(RichText::new(footer).size(11.0).color(p.text_faint));
                }
                let _ = ctx;
            });
    }

    /// The right-hand dock: comments, and the assistant once it has a backend.
    fn dock_panel(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if !self.dock_open || self.tab().is_none() {
            return;
        }
        let mut action = PanelAction::None;
        // Flush and flat, so the frame contributes no margin — but the width is
        // still stored through `total_margin`, because `default_size` is an
        // outer width while `available_width` is the width inside the frame.
        // Reading one and writing the other is what walked the side panel down
        // to its floor once per launch.
        // Horizontal padding belongs on the frame, not on each pane: without
        // it the contents start hard against the panel edge and clip.
        let frame = egui::Frame::new()
            .fill(p.card)
            .inner_margin(egui::Margin::symmetric(10, 0));
        let chrome_width = frame.total_margin().sum().x;

        egui::Panel::right("dock")
            .resizable(true)
            .default_size(self.prefs.dock_width.max(theme::DOCK_MIN_WIDTH))
            .size_range(theme::DOCK_MIN_WIDTH..=theme::DOCK_MAX_WIDTH)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                theme::hairline(ui.painter(), ui.max_rect(), p, theme::Edge::Left);
                self.prefs.dock_width = ui.available_width() + chrome_width;

                // Tab strip for the dock.
                ui.add_space(7.0);
                ui.horizontal(|ui| {
                    for (which, label) in [(Dock::Comments, "Comments"), (Dock::Assistant, "Assistant")] {
                        if widgets::text_button(ui, p, label, self.dock == which).clicked() {
                            self.dock = which;
                        }
                    }
                });
                ui.add_space(5.0);
                let line = ui.max_rect();
                ui.painter().line_segment(
                    [
                        egui::pos2(line.left(), ui.cursor().top()),
                        egui::pos2(line.right(), ui.cursor().top()),
                    ],
                    egui::Stroke::new(1.0, p.border),
                );
                ui.add_space(7.0);

                match self.dock {
                    Dock::Comments => {
                        let index = self.active;
                        let (service, textures, prefs) =
                            (&self.service, &mut self.textures, &self.prefs);
                        if let Some(tab) = self.tabs.get_mut(index) {
                            action = panels::show(
                                ui,
                                p,
                                SidePanel::Comments,
                                tab,
                                service,
                                textures,
                                prefs,
                            );
                        }
                    }
                    Dock::Assistant => self.assistant_panel(ui, p),
                }
            });
        self.handle_panel_action(action, &ui.ctx().clone());
    }

    /// The document toolbar: identity on the left, navigation centred, actions
    /// on the right.
    ///
    /// The tool strip and the panel toggles that used to live here are gone —
    /// the rail owns tools and the nav owns panels, and a control that exists
    /// in two places teaches that neither one is authoritative.
    fn toolbar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // Panels are drawn into the parent `Ui` in this version of egui; the
        // context is still needed for dispatch, and cloning it is just an Arc.
        let ctx = &ui.ctx().clone();
        egui::Panel::top("toolbar")
            .exact_size(theme::bar_panel_size(theme::TOOLBAR_HEIGHT))
            .show_separator_line(false)
            .frame(theme::bar(p))
            .show(ui, |ui| {
                let bar = ui.max_rect();
                theme::hairline(ui.painter(), bar, p, theme::Edge::Bottom);
                let has_doc = self.tab().is_some();

                // --- left: which document this is ---
                ui.horizontal_centered(|ui| {
                    let (name, folder) = self
                        .tab()
                        .map(|t| {
                            let name = widgets::elide(&t.title(), 22, true);
                            let folder = t
                                .path()
                                .and_then(|q| q.parent().map(|d| d.to_string_lossy().into_owned()))
                                .map(|d| widgets::elide(&d, 24, false))
                                .unwrap_or_default();
                            (name, folder)
                        })
                        .unwrap_or_else(|| (String::from("No document"), String::new()));
                    widgets::file_chip(ui, p, &name, &folder);
                });

                // --- centre: page and zoom, truly centred on the bar ---
                //
                // Placed at an explicit rect rather than reached by padding:
                // the left and right groups change width with the filename and
                // the theme icon, and anything measured from them drifts.
                let centre = egui::Rect::from_center_size(
                    bar.center(),
                    egui::vec2(CENTRE_GROUP_WIDTH, bar.height()),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(centre), |ui| {
                    ui.horizontal_centered(|ui| {
                        let (page, count) = self
                            .tab()
                            .map(|t| (t.current_page, t.page_count()))
                            .unwrap_or((0, 0));

                        widgets::pill(ui, p, |ui| {
                            if widgets::tool_button(
                                ui,
                                p,
                                Icon::ChevronUp,
                                "Previous page",
                                false,
                                page > 0,
                            )
                            .clicked()
                            {
                                self.dispatch(Command::PrevPage, ctx);
                            }
                            let label = if count == 0 {
                                String::from("—")
                            } else {
                                format!("{} / {}", page + 1, count)
                            };
                            ui.label(RichText::new(label).size(12.0).color(p.text_muted));
                            if widgets::tool_button(
                                ui,
                                p,
                                Icon::ChevronDown,
                                "Next page",
                                false,
                                count > 0 && page + 1 < count,
                            )
                            .clicked()
                            {
                                self.dispatch(Command::NextPage, ctx);
                            }
                        });

                        ui.add_space(6.0);

                        widgets::pill(ui, p, |ui| {
                            if widgets::tool_button(
                                ui,
                                p,
                                Icon::ZoomOut,
                                "Zoom out (Ctrl+-)",
                                false,
                                has_doc,
                            )
                            .clicked()
                            {
                                self.dispatch(Command::ZoomOut, ctx);
                            }
                            let zoom_label = self
                                .tab()
                                .map(|t| format!("{:.0}%", t.layout.scale * 100.0))
                                .unwrap_or_else(|| String::from("—"));
                            ui.menu_button(zoom_label, |ui| {
                                for (label, mode) in [
                                    ("Fit Width", ZoomMode::FitWidth),
                                    ("Fit Page", ZoomMode::FitPage),
                                    ("Fit Height", ZoomMode::FitHeight),
                                    ("Actual Size", ZoomMode::Actual),
                                ] {
                                    if ui.button(label).clicked() {
                                        self.dispatch(Command::SetZoom(mode), ctx);
                                        ui.close();
                                    }
                                }
                                ui.separator();
                                for z in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 4.0] {
                                    if ui.button(format!("{:.0}%", z * 100.0)).clicked() {
                                        self.dispatch(Command::SetZoom(ZoomMode::Custom(z)), ctx);
                                        ui.close();
                                    }
                                }
                            });
                            if widgets::tool_button(
                                ui,
                                p,
                                Icon::ZoomIn,
                                "Zoom in (Ctrl+=)",
                                false,
                                has_doc,
                            )
                            .clicked()
                            {
                                self.dispatch(Command::ZoomIn, ctx);
                            }
                        });

                        ui.add_space(8.0);
                        widgets::separator(ui, p);
                        ui.add_space(2.0);

                        let can_undo = self.tab().map(|t| t.history.can_undo()).unwrap_or(false);
                        let can_redo = self.tab().map(|t| t.history.can_redo()).unwrap_or(false);
                        if widgets::tool_button(ui, p, Icon::Undo, "Undo (Ctrl+Z)", false, can_undo)
                            .clicked()
                        {
                            self.dispatch(Command::Undo, ctx);
                        }
                        if widgets::tool_button(ui, p, Icon::Redo, "Redo (Ctrl+Y)", false, can_redo)
                            .clicked()
                        {
                            self.dispatch(Command::Redo, ctx);
                        }
                        let dirty = self.tab().map(|t| t.has_unsaved_changes()).unwrap_or(false);
                        if widgets::tool_button(ui, p, Icon::Save, "Save (Ctrl+S)", dirty, has_doc)
                            .clicked()
                        {
                            self.save(false);
                        }
                        if widgets::tool_button(ui, p, Icon::Print, "Print (Ctrl+P)", false, has_doc)
                            .clicked()
                        {
                            self.dispatch(Command::Print, ctx);
                        }
                    });
                });

                // --- right: search, the dock, and the theme ---
                ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let dark = self.theme.is_dark();
                        if widgets::tool_button(
                            ui,
                            p,
                            if dark { Icon::Sun } else { Icon::Moon },
                            "Toggle light/dark theme",
                            false,
                            true,
                        )
                        .clicked()
                        {
                            self.dispatch(Command::ToggleTheme, ctx);
                        }
                        if widgets::tool_button(
                            ui,
                            p,
                            Icon::Comment,
                            "Comments and assistant",
                            self.dock_open,
                            has_doc,
                        )
                        .clicked()
                        {
                            self.dock_open = !self.dock_open;
                        }
                        if widgets::tool_button(ui, p, Icon::Search, "Find (Ctrl+F)", false, has_doc)
                            .clicked()
                        {
                            self.dispatch(Command::Find, ctx);
                        }
                    });
                });
            });
    }
    fn tab_strip(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // Panels are drawn into the parent `Ui` in this version of egui; the
        // context is still needed for dispatch, and cloning it is just an Arc.
        let _ctx = &ui.ctx().clone();
        // Home is always in the strip, so unlike before there is no count at
        // which the strip is not worth drawing.
        egui::Panel::top("tabs")
            .exact_size(theme::bar_panel_size(theme::TAB_HEIGHT))
            .show_separator_line(false)
            .frame(theme::bar(p))
            .show(ui, |ui| {
                let mut to_close = None;
                let mut to_select = None;
                let mut go_home = false;
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Home first and always, with no close button —
                            // there is nothing to close, and an X that does
                            // nothing invites the click anyway.
                            if widgets::text_button(ui, p, "Home", self.home.visible).clicked() {
                                go_home = true;
                            }
                            widgets::separator(ui, p);

                            for (i, tab) in self.tabs.iter().enumerate() {
                                let active = !self.home.visible && i == self.active;
                                let title = widgets::elide(&tab.title(), 26, true);
                                let label = if tab.has_unsaved_changes() {
                                    format!("• {title}")
                                } else {
                                    title
                                };
                                if widgets::text_button(ui, p, &label, active).clicked() {
                                    to_select = Some(i);
                                }
                                if widgets::tool_button(ui, p, Icon::Close, "Close", false, true)
                                    .clicked()
                                {
                                    to_close = Some(i);
                                }
                                widgets::separator(ui, p);
                            }
                        });
                    });
                if go_home {
                    self.show_home();
                }
                if let Some(i) = to_select {
                    self.show_tab(i);
                    self.textures.clear();
                }
                if let Some(i) = to_close {
                    self.close_tab(i);
                }
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // Panels are drawn into the parent `Ui` in this version of egui; the
        // context is still needed for dispatch, and cloning it is just an Arc.
        let _ctx = &ui.ctx().clone();
        egui::Panel::bottom("status")
            .exact_size(theme::bar_panel_size(theme::STATUS_HEIGHT))
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(p.card)
                    .stroke(egui::Stroke::new(1.0, p.border))
                    .corner_radius(egui::CornerRadius::same(theme::RADIUS_GLASS))
                    .outer_margin(egui::Margin {
                        left: theme::GLASS_INSET,
                        right: theme::GLASS_INSET,
                        top: 0,
                        bottom: theme::GLASS_INSET,
                    })
                    .inner_margin(egui::Margin::symmetric(12, 3)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let small = |s: String| RichText::new(s).size(11.0);
                    match self.tab() {
                        Some(tab) => {
                            ui.label(small(format!(
                                "Page {} of {}",
                                tab.current_page + 1,
                                tab.page_count().max(1)
                            )));
                            ui.label(RichText::new("·").color(p.text_faint));
                            let size = tab.info.page_size(tab.current_page);
                            ui.label(
                                small(format!(
                                    "{:.2} × {:.2} in",
                                    size.width / 72.0,
                                    size.height / 72.0
                                ))
                                .color(p.text_muted),
                            );
                            if tab.info.encrypted {
                                ui.label(RichText::new("·").color(p.text_faint));
                                ui.label(small("Protected".into()).color(p.warning));
                            }
                            if tab.info.likely_scanned {
                                ui.label(RichText::new("·").color(p.text_faint));
                                ui.label(
                                    small("No text layer — scanned?".into()).color(p.text_muted),
                                );
                            }
                        }
                        None => {
                            ui.label(small("No document open".into()).color(p.text_faint));
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Newest toast wins the space.
                        if let Some(t) = self.toasts.last() {
                            ui.label(
                                RichText::new(&t.text)
                                    .size(11.0)
                                    .color(if t.error { p.error } else { p.success }),
                            );
                        } else {
                            ui.label(
                                RichText::new(format!(
                                    "{:.0} MB cached · {} textures",
                                    self.textures.used_mb(),
                                    self.textures.len()
                                ))
                                .size(10.0)
                                .color(p.text_faint),
                            );
                        }
                    });
                });
            });
    }

    /// The list column: whatever the nav sidebar has selected.
    fn side_panel(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // Panels are drawn into the parent `Ui` in this version of egui; the
        // context is still needed for dispatch, and cloning it is just an Arc.
        let ctx = &ui.ctx().clone();
        // Home carries its own rail, and the document panels have no document
        // to describe while it is showing.
        if self.prefs.side_panel == SidePanel::None || self.tabs.is_empty() || self.home.visible {
            return;
        }
        let which = self.prefs.side_panel;
        let mut action = PanelAction::None;

        // Flush and flat: fill plus the hairline that separates it from the
        // canvas. No inset, no rounding, no shadow, so `default_size` and
        // `available_width` differ only by the inner margin.
        let frame = egui::Frame::new()
            .fill(p.card)
            .inner_margin(egui::Margin::symmetric(8, 10));
        // `default_size` is an outer width but `available_width` is the width
        // inside the frame, so storing the latter directly loses the margins
        // once per launch — the panel walks itself down to the floor over a
        // few restarts.
        let chrome_width = frame.total_margin().sum().x;

        egui::Panel::left("side")
            .resizable(true)
            .default_size(self.prefs.side_panel_width.max(theme::LIST_WIDTH))
            .size_range(170.0..=520.0)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                theme::hairline(ui.painter(), ui.max_rect(), p, theme::Edge::Right);
                self.prefs.side_panel_width = ui.available_width() + chrome_width;
                let Some(index) = self.tabs.get_mut(self.active).map(|_| self.active) else {
                    return;
                };
                // Split the borrow: the panel needs the tab mutably and the
                // service and cache immutably at the same time.
                let (service, textures, prefs) =
                    (&self.service, &mut self.textures, &self.prefs);
                if let Some(tab) = self.tabs.get_mut(index) {
                    action = panels::show(ui, p, which, tab, service, textures, prefs);
                }
            });

        self.handle_panel_action(action, ctx);
    }

    fn handle_panel_action(&mut self, action: PanelAction, ctx: &Context) {
        match action {
            PanelAction::None | PanelAction::SelectPages => {}
            PanelAction::GoToPage(page) => {
                if let Some(t) = self.tab_mut() {
                    t.go_to_page(page);
                }
            }
            PanelAction::RevealAnnotation(id) => {
                if let Some(t) = self.tab_mut() {
                    if let Some(a) = t.find_annotation(id) {
                        let (page, rect) = (a.page, a.rect);
                        t.selected_annotation = Some(id);
                        t.reveal(page, rect);
                    }
                }
            }
            PanelAction::DeleteAnnotation(id) => self.delete_annotation(id),
            PanelAction::GoToHit(i) => {
                let hit = self.tab().and_then(|t| t.search.hits.get(i).cloned());
                if let (Some(hit), Some(t)) = (hit, self.tab_mut()) {
                    t.search.current = Some(i);
                    crate::app::reveal_hit(t, &hit);
                }
            }
            PanelAction::RunSearch(q) => self.run_search(q),
            PanelAction::MovePage { from, to } => {
                self.apply_op(DocOp::MovePage { from, to });
            }
            PanelAction::DeleteSelectedPages => self.dispatch(Command::DeletePages, ctx),
            PanelAction::RotateSelectedPages(cw) => {
                self.dispatch(
                    if cw {
                        Command::RotatePagesCw
                    } else {
                        Command::RotatePagesCcw
                    },
                    ctx,
                );
            }
            PanelAction::ExtractAttachment(index) => {
                let Some(tab) = self.tab() else { return };
                let name = tab
                    .attachments
                    .iter()
                    .find(|a| a.index == index)
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| "attachment".into());
                if let Some(to) = rfd::FileDialog::new().set_file_name(&name).save_file() {
                    self.service.send(Request::ExtractAttachment {
                        id: tab.id,
                        index,
                        to,
                    });
                }
            }
            PanelAction::FocusField(name) => {
                // Scroll the field into view so it can be filled in on the page.
                let target = self
                    .tab()
                    .and_then(|t| t.fields.iter().find(|f| f.name == name).map(|f| (f.page, f.rect)));
                if let (Some((page, rect)), Some(t)) = (target, self.tab_mut()) {
                    t.reveal(page, rect);
                }
            }
        }
    }

    fn command_palette(&mut self, ctx: &Context, p: &Palette) {
        if !self.show_palette {
            return;
        }
        let mut chosen = None;
        egui::Modal::new(egui::Id::new("palette")).show(ctx, |ui| {
            ui.set_min_width(460.0);
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.palette_query)
                    .hint_text("Type a command")
                    .desired_width(f32::INFINITY),
            );
            field.request_focus();
            ui.add_space(6.0);

            let q = self.palette_query.to_lowercase();
            let table = command_table();
            let matches: Vec<_> = table
                .iter()
                .filter(|c| q.is_empty() || c.label.to_lowercase().contains(&q))
                .take(200)
                .collect();

            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    for c in matches {
                        let enabled = self.command_enabled(&c.command, c.mutates);
                        ui.horizontal(|ui| {
                            let r = ui.add_enabled(
                                enabled,
                                egui::Button::new(c.label).min_size(egui::vec2(320.0, 0.0)),
                            );
                            ui.label(
                                RichText::new(c.menu.label())
                                    .size(10.0)
                                    .color(p.text_faint),
                            );
                            if !c.shortcut.is_empty() {
                                ui.label(
                                    RichText::new(c.shortcut).size(10.0).color(p.accent_text),
                                );
                            }
                            if r.clicked() {
                                chosen = Some(c.command.clone());
                            }
                        });
                    }
                });
        });

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_palette = false;
            self.palette_query.clear();
        }
        if let Some(cmd) = chosen {
            self.show_palette = false;
            self.palette_query.clear();
            self.dispatch(cmd, ctx);
        }
    }

    // ------------------------------------------------------------- dispatch

    fn run_search(&mut self, query: String) {
        let Some(tab) = self.tab_mut() else { return };
        if query.trim().is_empty() {
            tab.search.clear();
            return;
        }
        tab.search.query = query.clone();
        tab.search.clear();
        tab.search.running = true;
        tab.search.total_pages = tab.page_count();
        let (id, options) = (tab.id, tab.search.options);
        self.search_token = self.search_token.wrapping_add(1);
        self.service.send(Request::Search {
            id,
            query,
            options,
            token: self.search_token,
        });
    }

    /// The pages an operation should act on: the thumbnail selection if there
    /// is one, otherwise the page being read.
    fn target_pages(&self) -> Vec<usize> {
        let Some(tab) = self.tab() else {
            return Vec::new();
        };
        if tab.selected_pages.is_empty() {
            vec![tab.current_page]
        } else {
            let mut v = tab.selected_pages.clone();
            v.sort_unstable();
            v.dedup();
            v
        }
    }

    pub(crate) fn dispatch(&mut self, cmd: Command, ctx: &Context) {
        match cmd {
            // --- file ---
            Command::Open => self.open_dialog(),
            Command::OpenRecent(i) => {
                if let Some(r) = self.prefs.recent.get(i).cloned() {
                    self.open_path(&PathBuf::from(r.path), None);
                }
            }
            Command::Save => self.save(false),
            Command::SaveAs | Command::SaveACopy => self.save(true),
            // Close acts on the surface being looked at, and home cannot be
            // closed. Without this guard Ctrl+W on the home screen would shut
            // a document the user cannot even see — `command_enabled` greys
            // these out in the menu and the palette, but keyboard shortcuts
            // reach `dispatch` directly.
            Command::Close => {
                if !self.home.visible {
                    self.close_tab(self.active);
                }
            }
            Command::CloseAll => {
                if !self.home.visible {
                    while !self.tabs.is_empty() {
                        self.force_close_tab(0);
                    }
                }
            }
            Command::Revert => {
                if let Some(path) = self.tab().and_then(|t| t.path().cloned()) {
                    self.force_close_tab(self.active);
                    self.open_path(&path, None);
                }
            }
            Command::Quit => {
                self.quit_confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Command::Properties => {
                if let Some(tab) = self.tab() {
                    self.dialog = Some(Dialog::Properties {
                        info: Box::new(tab.info.clone()),
                        edited: Box::new(tab.info.metadata.clone()),
                    });
                }
            }
            Command::Print => self.print(),
            Command::NewFromBlank => {
                self.toast("Creating a blank document — use File ▸ Save As to name it", false);
                // Handled by creating a one-page document through the engine.
                let id = self.service.new_doc_id();
                let tmp = std::env::temp_dir().join("quark-blank.pdf");
                match quark_pdf::export::pdf_from_images(&[], quark_core::geom::PageSize::LETTER) {
                    // pdf_from_images refuses an empty list, which is correct;
                    // a blank document is built by the engine instead.
                    _ => {
                        if let Ok(doc) = quark_pdf::Document::create_blank(
                            quark_core::geom::PageSize::LETTER,
                        ) {
                            if let Ok(bytes) = doc.to_bytes() {
                                if std::fs::write(&tmp, bytes).is_ok() {
                                    self.service.send(Request::Open {
                                        id,
                                        path: tmp,
                                        password: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            Command::NewFromImages => self.new_from_images(),
            Command::NewFromText => self.toast(
                "Creating a PDF from a text file is not implemented — \
                 open the text in another application and print to Quark",
                false,
            ),
            Command::CombineFiles => self.combine_files(),
            Command::ExportPagesAsImages => {
                self.dialog = Some(Dialog::ExportImages {
                    options: ImageExport::default(),
                });
            }
            Command::ExportText => {
                let Some(tab) = self.tab() else { return };
                let name = tab
                    .path()
                    .and_then(|p| p.file_stem())
                    .map(|s| format!("{}.txt", s.to_string_lossy()))
                    .unwrap_or_else(|| "document.txt".into());
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Text", &["txt"])
                    .set_file_name(&name)
                    .save_file()
                {
                    self.service.send(Request::ExportText { id: tab.id, path });
                }
            }
            Command::ExtractPages => {
                let pages = self.target_pages();
                if !pages.is_empty() {
                    self.apply_op(DocOp::ExtractPages(pages));
                }
            }
            Command::SplitDocument => self.toast(
                "Use Pages ▸ Extract Pages with a thumbnail selection to split a document",
                false,
            ),
            Command::Optimize => self.apply_op(DocOp::Optimize),

            // --- edit ---
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Copy => {
                if let Some(text) = self.tab().and_then(|t| t.selected_text()) {
                    ctx.copy_text(text);
                    self.toast("Copied", false);
                }
            }
            Command::Cut | Command::Paste => {
                self.toast("Cut and paste apply to page content, which is not editable yet", false)
            }
            Command::Delete => {
                let id = self.tab().and_then(|t| t.selected_annotation);
                if let Some(id) = id {
                    self.delete_annotation(id);
                }
            }
            Command::SelectAll => {
                // Selects all text on the page being read.
                if let Some(t) = self.tab_mut() {
                    let page = t.current_page;
                    if let Some(text) = t.page_text.get(&page) {
                        let n = text.char_count();
                        t.selection = Some(crate::tab::TextSelection {
                            page,
                            start: 0,
                            end: n,
                        });
                    }
                }
            }
            Command::Deselect => {
                if let Some(t) = self.tab_mut() {
                    t.selection = None;
                    t.selected_annotation = None;
                    t.selected_pages.clear();
                }
            }
            Command::CopyAsImage => self.toast("Use the Snapshot tool to copy a region", false),
            Command::Preferences => self.toast("Preferences live in the View and Tools menus", false),

            // --- view ---
            Command::ZoomIn => {
                let anchor = self.tab().map(|t| t.viewport * 0.5).unwrap_or(Vec2::ZERO);
                if let Some(t) = self.tab_mut() {
                    let next = layout::zoom_step_up(t.layout.scale);
                    t.zoom_to(next, anchor);
                }
            }
            Command::ZoomOut => {
                let anchor = self.tab().map(|t| t.viewport * 0.5).unwrap_or(Vec2::ZERO);
                if let Some(t) = self.tab_mut() {
                    let next = layout::zoom_step_down(t.layout.scale);
                    t.zoom_to(next, anchor);
                }
            }
            Command::SetZoom(mode) => {
                self.prefs.zoom = mode;
                if let Some(t) = self.tab_mut() {
                    t.zoom = mode;
                    t.invalidate_layout();
                    t.bump_token();
                }
            }
            Command::SetPageMode(mode) => {
                self.prefs.page_mode = mode;
                if let Some(t) = self.tab_mut() {
                    t.page_mode = mode;
                    t.invalidate_layout();
                    t.bump_token();
                }
            }
            Command::RotateViewCw | Command::RotateViewCcw => {
                let cw = cmd == Command::RotateViewCw;
                if let Some(t) = self.tab_mut() {
                    t.rotation = if cw { t.rotation.cw() } else { t.rotation.ccw() };
                    t.invalidate_layout();
                    t.bump_token();
                }
                self.textures.clear();
            }
            Command::SetTint(tint) => {
                self.prefs.tint = tint;
                self.textures.clear();
                if let Some(t) = self.tab_mut() {
                    t.bump_token();
                }
            }
            Command::TogglePanel(panel) => {
                self.prefs.side_panel = if self.prefs.side_panel == panel {
                    SidePanel::None
                } else {
                    panel
                };
            }
            Command::ToggleToolbar => self.prefs.show_toolbar = !self.prefs.show_toolbar,
            Command::ToggleStatusBar => self.prefs.show_status_bar = !self.prefs.show_status_bar,
            Command::FullScreen => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
            Command::ReadingMode => {
                // Hides everything but the page.
                self.prefs.side_panel = SidePanel::None;
                self.prefs.show_toolbar = !self.prefs.show_toolbar;
            }
            Command::ToggleTheme => {
                self.theme = self.theme.toggled();
                self.prefs.theme_dark = self.theme.is_dark();
                theme::apply(ctx, self.theme);
            }

            // --- navigation ---
            Command::NextPage => {
                if let Some(t) = self.tab_mut() {
                    let next = (t.current_page + 1).min(t.page_count().saturating_sub(1));
                    t.go_to_page(next);
                }
            }
            Command::PrevPage => {
                if let Some(t) = self.tab_mut() {
                    let prev = t.current_page.saturating_sub(1);
                    t.go_to_page(prev);
                }
            }
            Command::FirstPage => {
                if let Some(t) = self.tab_mut() {
                    t.go_to_page(0);
                }
            }
            Command::LastPage => {
                if let Some(t) = self.tab_mut() {
                    let last = t.page_count().saturating_sub(1);
                    t.go_to_page(last);
                }
            }
            Command::GoToPage(_) => {
                if let Some(t) = self.tab() {
                    self.dialog = Some(Dialog::GoToPage {
                        entry: String::new(),
                        max: t.page_count().max(1),
                    });
                }
            }
            Command::GoBack => {
                if let Some(t) = self.tab_mut() {
                    t.go_back();
                }
            }
            Command::GoForward => {
                if let Some(t) = self.tab_mut() {
                    t.go_forward();
                }
            }
            Command::ScrollUp | Command::ScrollDown => {
                let down = cmd == Command::ScrollDown;
                if let Some(t) = self.tab_mut() {
                    let step = t.viewport.y * 0.9;
                    t.scroll.y += if down { step } else { -step };
                    t.clamp_scroll();
                    t.sync_current_page();
                }
            }

            // --- find ---
            Command::Find | Command::AdvancedSearch => {
                self.prefs.side_panel = SidePanel::Search;
            }
            Command::FindNext => {
                let hit = self.tab_mut().and_then(|t| t.search.next().cloned());
                if let (Some(hit), Some(t)) = (hit, self.tab_mut()) {
                    crate::app::reveal_hit(t, &hit);
                }
            }
            Command::FindPrev => {
                let hit = self.tab_mut().and_then(|t| t.search.prev().cloned());
                if let (Some(hit), Some(t)) = (hit, self.tab_mut()) {
                    crate::app::reveal_hit(t, &hit);
                }
            }

            // --- tools ---
            Command::SetTool(tool) => self.tool = tool,
            Command::Comment => self.prefs.side_panel = SidePanel::Comments,

            // --- pages ---
            Command::InsertPagesFromFile => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PDF documents", &["pdf"])
                    .pick_file()
                {
                    let at = self.tab().map(|t| t.current_page).unwrap_or(0);
                    self.apply_op(DocOp::InsertPagesFromFile { path, at });
                }
            }
            Command::InsertBlankPage => {
                let at = self.tab().map(|t| t.current_page).unwrap_or(0);
                self.dialog = Some(Dialog::InsertBlank {
                    at,
                    width: "612".into(),
                    height: "792".into(),
                });
            }
            Command::DeletePages => {
                let pages = self.target_pages();
                let Some(tab) = self.tab() else { return };
                if pages.len() >= tab.page_count() {
                    self.toast("A document must keep at least one page", true);
                    return;
                }
                if self.prefs.confirm_destructive {
                    self.dialog = Some(Dialog::Confirm {
                        title: "Delete Pages".into(),
                        body: format!(
                            "Delete {} page(s)? This can be undone until the document is saved.",
                            pages.len()
                        ),
                        confirm_label: "Delete".into(),
                        danger: true,
                    });
                } else {
                    self.apply_op(DocOp::DeletePages(pages));
                }
            }
            Command::RotatePagesCw | Command::RotatePagesCcw => {
                let by = if cmd == Command::RotatePagesCw {
                    Rot::D90
                } else {
                    Rot::D270
                };
                let pages = self.target_pages();
                self.apply_op(DocOp::RotatePages { pages, by });
            }
            Command::MovePages => self.toast("Drag thumbnails to reorder pages", false),
            Command::CropPages => {
                self.tool = Tool::Crop;
                self.toast("Drag a rectangle on the page to crop it", false);
            }
            Command::ResizePages => self.toast("Page resizing is not implemented", false),
            Command::AddWatermark => {
                self.dialog = Some(Dialog::Watermark {
                    stamp: Box::new(TextStamp::default()),
                });
            }
            Command::AddBackground => {
                self.dialog = Some(Dialog::Watermark {
                    stamp: Box::new(TextStamp {
                        behind: true,
                        ..TextStamp::default()
                    }),
                });
            }
            Command::AddHeaderFooter => {
                self.dialog = Some(Dialog::HeaderFooter {
                    stamp: Box::new(TextStamp {
                        text: "Page <<n>> of <<N>>".into(),
                        font_size: 10.0,
                        anchor: quark_pdf::export::Anchor::BottomCenter,
                        opacity: 1.0,
                        rotation: 0.0,
                        color: [0.0, 0.0, 0.0],
                        margin: 24.0,
                        behind: false,
                    }),
                });
            }
            Command::AddBatesNumbering => {
                self.dialog = Some(Dialog::Bates {
                    options: Box::new(BatesOptions::default()),
                });
            }
            Command::RemoveWatermark | Command::RemoveBatesNumbering => self.toast(
                "Stamps are part of the page content once applied and cannot be removed \
                 selectively — undo before saving, or keep an unstamped copy",
                false,
            ),
            Command::AddBookmark => {
                let Some(tab) = self.tab() else { return };
                let page = tab.current_page;
                let mut outline = tab.outline.clone();
                outline.push(quark_pdf::outline::Bookmark {
                    title: format!("Page {}", page + 1),
                    page: Some(page),
                    children: Vec::new(),
                    depth: 0,
                });
                self.apply_op(DocOp::SetOutline(outline));
            }
            Command::DeleteBookmark | Command::RenameBookmark => {
                self.toast("Bookmark editing is available through Pages ▸ Add Bookmark", false)
            }
            Command::AttachFile => self.toast("Attaching files is not implemented", false),
            Command::ExtractAttachment => {
                self.prefs.side_panel = SidePanel::Attachments;
            }

            // --- forms ---
            Command::PrepareForm | Command::HighlightFields => {
                self.prefs.side_panel = SidePanel::Fields;
            }
            Command::FlattenForm => self.toast("Form flattening is not implemented", false),
            Command::ExportFormData => self.toast("Form export is not implemented", false),
            Command::ImportFormData => self.toast("Form import is not implemented", false),
            Command::ClearForm => self.apply_op(DocOp::ClearForm),

            // --- protect ---
            Command::EncryptWithPassword => {
                self.dialog = Some(Dialog::Encrypt {
                    user: String::new(),
                    owner: String::new(),
                    confirm: String::new(),
                    permissions: quark_pdf::security::PermissionSet::default(),
                });
            }
            Command::RemoveSecurity => {
                self.toast("Reopen the document with its password, then Save As", false)
            }
            Command::RestrictEditing => self.dispatch(Command::EncryptWithPassword, ctx),
            Command::MarkForRedaction => self.tool = Tool::Redact,
            Command::ApplyRedactions => {
                let marks = self.redaction_marks();
                if marks.is_empty() {
                    self.toast("Nothing is marked for redaction", true);
                    return;
                }
                self.dialog = Some(Dialog::Confirm {
                    title: "Apply Redactions".into(),
                    body: format!(
                        "Permanently remove the content under {} marked area(s)? \
                         This cannot be undone, and the removed text will not be \
                         recoverable from the saved file.",
                        marks.len()
                    ),
                    confirm_label: "Apply Redactions".into(),
                    danger: true,
                });
            }
            Command::SanitizeDocument | Command::RemoveHiddenInformation => {
                let found = self
                    .tab()
                    .map(|t| {
                        let mut v = Vec::new();
                        if !t.info.metadata.is_empty() {
                            v.push("Document properties".to_string());
                        }
                        if t.info.has_form {
                            v.push("Form fields".to_string());
                        }
                        if t.info.attachment_count > 0 {
                            v.push("Embedded files".to_string());
                        }
                        if !t.annotations.is_empty() {
                            v.push("Comments and markup".to_string());
                        }
                        if t.info.bookmark_count > 0 {
                            v.push("Bookmarks".to_string());
                        }
                        v
                    })
                    .unwrap_or_default();
                self.dialog = Some(Dialog::Sanitize {
                    options: SanitizeOptions::default(),
                    found,
                });
            }
            Command::SignDocument | Command::CertifyDocument => self.toast(
                "Quark cannot yet create digital signatures — it reads and reports them only",
                false,
            ),
            Command::ValidateSignatures => {
                self.prefs.side_panel = SidePanel::Signatures;
            }

            // --- other ---
            Command::RecognizeText => self.toast("OCR is not built in yet", false),
            Command::CompareDocuments => self.toast("Document comparison is not implemented", false),
            Command::ReadOutLoud => self.toast("Read Out Loud is not implemented", false),
            Command::ShowTags | Command::AutoTagDocument => {
                self.toast("Accessibility tagging is not implemented", false)
            }
            // Home is position 0 of the strip, so cycling runs over
            // `tabs.len() + 1` slots and wraps through it like any other tab.
            Command::NextTab => {
                let slot = self.strip_position();
                self.select_strip_position((slot + 1) % (self.tabs.len() + 1));
            }
            Command::PrevTab => {
                let slots = self.tabs.len() + 1;
                let slot = self.strip_position();
                self.select_strip_position((slot + slots - 1) % slots);
            }
            Command::CommandPalette => self.show_palette = true,
            Command::Help => {
                self.dialog = Some(Dialog::Message {
                    title: "Quark Help".into(),
                    body: HELP_TEXT.into(),
                    error: false,
                });
            }
            Command::About => self.dialog = Some(Dialog::About),
        }
    }

    /// Every marked-but-unapplied redaction in the active document.
    fn redaction_marks(&self) -> Vec<RedactionMark> {
        let Some(tab) = self.tab() else {
            return Vec::new();
        };
        tab.all_annotations()
            .iter()
            .filter(|a| matches!(a.kind, quark_core::annot::AnnotKind::Redact { .. }))
            .map(|a| RedactionMark {
                page: a.page,
                rect: a.rect,
            })
            .collect()
    }

    fn undo(&mut self) {
        let Some(tab) = self.tab_mut() else { return };
        let Some(edit) = tab.history.undo() else { return };
        apply_inverse(tab, &edit);
    }

    fn redo(&mut self) {
        let Some(tab) = self.tab_mut() else { return };
        let Some(edit) = tab.history.redo() else { return };
        apply_edit(tab, &edit);
    }

    fn print(&mut self) {
        let Some(path) = self.tab().and_then(|t| t.path().cloned()) else {
            self.toast("Save the document before printing", true);
            return;
        };
        #[cfg(windows)]
        {
            match quark_shell::print_document(&path) {
                Ok(()) => self.toast("Sent to the printer", false),
                Err(e) => self.toast(format!("Could not print: {e}"), true),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = path;
            self.toast("Printing is available on Windows", true);
        }
    }

    fn new_from_images(&mut self) {
        let Some(files) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp"])
            .pick_files()
        else {
            return;
        };
        match quark_pdf::export::pdf_from_images(&files, quark_core::geom::PageSize::LETTER) {
            Ok(bytes) => {
                let tmp = std::env::temp_dir().join("quark-from-images.pdf");
                if std::fs::write(&tmp, bytes).is_ok() {
                    self.open_path(&tmp, None);
                    self.toast("Created a PDF from images — use Save As to keep it", false);
                }
            }
            Err(e) => self.toast(format!("Could not build a PDF: {e}"), true),
        }
    }

    fn combine_files(&mut self) {
        let Some(files) = rfd::FileDialog::new()
            .add_filter("PDF documents", &["pdf"])
            .pick_files()
        else {
            return;
        };
        if files.len() < 2 {
            self.toast("Choose at least two documents to combine", true);
            return;
        }
        let mut buffers = Vec::new();
        for f in &files {
            match std::fs::read(f) {
                Ok(b) => buffers.push(b),
                Err(e) => {
                    self.toast(format!("Could not read {}: {e}", f.display()), true);
                    return;
                }
            }
        }
        match quark_pdf::organize::merge(&buffers) {
            Ok(bytes) => {
                let tmp = std::env::temp_dir().join("quark-combined.pdf");
                if std::fs::write(&tmp, bytes).is_ok() {
                    self.open_path(&tmp, None);
                    self.toast("Combined — use Save As to keep the result", false);
                }
            }
            Err(e) => self.toast(format!("Could not combine: {e}"), true),
        }
    }

    // --------------------------------------------------------------- dialogs

    fn handle_dialog(&mut self, ctx: &Context, p: &Palette) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let result = dialog.show(ctx, p);
        match result {
            DialogResult::Pending => {
                self.dialog = Some(dialog);
            }
            DialogResult::Cancelled => {}
            DialogResult::OpenWithPassword(pw) => {
                if let Dialog::Password { path, .. } = &dialog {
                    let path = path.clone();
                    self.open_path(&path, Some(pw));
                }
            }
            DialogResult::GoToPage(page) => {
                if let Some(t) = self.tab_mut() {
                    t.go_to_page(page);
                }
            }
            DialogResult::SetMetadata(m) => self.apply_op(DocOp::SetMetadata(m)),
            DialogResult::Encrypt {
                user,
                owner,
                permissions,
            } => self.apply_op(DocOp::Encrypt {
                user,
                owner,
                permissions,
            }),
            DialogResult::Sanitize(o) => self.apply_op(DocOp::Sanitize(o)),
            DialogResult::Watermark(stamp) | DialogResult::HeaderFooter(stamp) => {
                let pages: Vec<usize> = (0..self.tab().map(|t| t.page_count()).unwrap_or(0)).collect();
                self.apply_op(DocOp::StampText { pages, stamp });
            }
            DialogResult::Bates(options) => {
                let pages: Vec<usize> = (0..self.tab().map(|t| t.page_count()).unwrap_or(0)).collect();
                self.apply_op(DocOp::ApplyBates { pages, options });
            }
            DialogResult::ExportImages(options) => {
                let Some(tab) = self.tab() else { return };
                let (id, count) = (tab.id, tab.page_count());
                let stem = tab
                    .path()
                    .and_then(|p| p.file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "page".into());
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.service.send(Request::ExportImages {
                        id,
                        pages: (0..count).collect(),
                        dir,
                        stem,
                        options,
                    });
                }
            }
            DialogResult::InsertBlank { at, size } => {
                self.apply_op(DocOp::InsertBlankPage { at, size })
            }
            DialogResult::Confirmed => match &dialog {
                Dialog::Confirm { title, .. } if title == "Apply Redactions" => {
                    let marks = self.redaction_marks();
                    self.apply_op(DocOp::ApplyRedactions(marks));
                }
                Dialog::Confirm { title, .. } if title == "Delete Pages" => {
                    let pages = self.target_pages();
                    self.apply_op(DocOp::DeletePages(pages));
                }
                Dialog::Confirm { title, .. } if title == "Close Without Saving" => {
                    self.force_close_tab(self.active);
                }
                _ => {}
            },
        }
    }

    // ------------------------------------------------------------ viewer glue

    fn handle_viewer_action(&mut self, action: ViewerAction, ctx: &Context) {
        match action {
            ViewerAction::None => {}
            ViewerAction::CreateAnnotation(ann) => {
                // The page raster is now out of date for that one page; the
                // rest of the document is untouched, so only it is dropped.
                let page = ann.page;
                self.commit_annotation(*ann);
                self.textures.invalidate_page(page);
                // Shape tools drop back to Select after one use, matching what
                // every other editor does; run tools stay put.
                if !self.tool.is_sticky() {
                    self.tool = Tool::Select;
                }
            }
            ViewerAction::MoveAnnotation { id, delta } => {
                let mut moved_page = None;
                if let Some(t) = self.tab_mut() {
                    if let Some(a) = t.find_annotation_mut(id) {
                        a.translate(delta);
                        moved_page = Some(a.page);
                    }
                    t.dirty = true;
                }
                if let Some(page) = moved_page {
                    self.textures.invalidate_page(page);
                }
            }
            ViewerAction::DeleteAnnotation(id) => {
                let page = self.tab().and_then(|t| t.find_annotation(id)).map(|a| a.page);
                self.delete_annotation(id);
                if let Some(page) = page {
                    self.textures.invalidate_page(page);
                }
            }
            ViewerAction::ZoomToRect { page, rect } => {
                if let Some(t) = self.tab_mut() {
                    viewer::zoom_to_rect(t, page, rect);
                }
                self.tool = Tool::Select;
            }
            ViewerAction::Snapshot { page, rect } => self.snapshot(page, rect, ctx),
            ViewerAction::Crop { page, rect } => {
                let pages = self.target_pages();
                let _ = page;
                self.apply_op(DocOp::CropPages {
                    pages,
                    box_: [rect.min.x, rect.min.y, rect.max.x, rect.max.y],
                });
                self.tool = Tool::Select;
            }
            ViewerAction::ActivateField(name) => {
                self.prefs.side_panel = SidePanel::Fields;
                self.toast(format!("Field: {name}"), false);
            }
            ViewerAction::GoToPage(page) => {
                if let Some(t) = self.tab_mut() {
                    t.go_to_page(page);
                }
            }
        }
    }

    /// Copies a rectangle of the page as an image.
    fn snapshot(&mut self, page: usize, rect: quark_core::geom::Rect, _ctx: &Context) {
        let Some(path) = self.tab().and_then(|t| t.path().cloned()) else {
            self.toast("Save the document first", true);
            return;
        };
        // Rendered fresh at print resolution rather than lifted from the
        // on-screen texture, so the snapshot is sharp regardless of zoom.
        let out = std::env::temp_dir().join("quark-snapshot.png");
        match quark_pdf::Document::open(&path, None)
            .map_err(|e| e.to_string())
            .and_then(|doc| {
                quark_pdf::export::page_to_image_bytes(&doc, page, &ImageExport {
                    dpi: 300.0,
                    ..Default::default()
                })
                .map_err(|e| e.to_string())
            }) {
            Ok(bytes) => {
                // Crop to the swept region.
                match image::load_from_memory(&bytes) {
                    Ok(img) => {
                        let scale = 300.0 / 72.0;
                        let size = self
                            .tab()
                            .map(|t| t.info.page_size(page))
                            .unwrap_or_default();
                        // Page space is y-up; the image is y-down.
                        let x = (rect.min.x * scale).max(0.0) as u32;
                        let y = ((size.height - rect.max.y) * scale).max(0.0) as u32;
                        let w = (rect.width() * scale) as u32;
                        let h = (rect.height() * scale) as u32;
                        let w = w.min(img.width().saturating_sub(x)).max(1);
                        let h = h.min(img.height().saturating_sub(y)).max(1);
                        let cropped = image::imageops::crop_imm(&img, x, y, w, h).to_image();
                        match cropped.save(&out) {
                            Ok(()) => self.toast(format!("Snapshot saved to {}", out.display()), false),
                            Err(e) => self.toast(format!("Could not save snapshot: {e}"), true),
                        }
                    }
                    Err(e) => self.toast(format!("Could not decode the page: {e}"), true),
                }
            }
            Err(e) => self.toast(format!("Could not render the page: {e}"), true),
        }
        self.tool = Tool::Select;
    }
}

/// Re-applies an edit to the in-memory model (redo).
fn apply_edit(tab: &mut crate::tab::Tab, edit: &quark_core::history::Edit) {
    use quark_core::history::Edit;
    match edit {
        Edit::AddAnnotation(a) => tab.pending_annotations.push((**a).clone()),
        Edit::RemoveAnnotation(a) => {
            tab.pending_annotations.retain(|x| x.id != a.id);
            tab.annotations.retain(|x| x.id != a.id);
        }
        Edit::ModifyAnnotation { after, .. } => {
            if let Some(x) = tab.find_annotation_mut(after.id) {
                *x = (**after).clone();
            }
        }
        // Structural edits are re-applied through the engine, not here.
        _ => {}
    }
    tab.dirty = true;
}

/// Applies an edit's inverse (undo).
fn apply_inverse(tab: &mut crate::tab::Tab, edit: &quark_core::history::Edit) {
    apply_edit(tab, &edit.inverse());
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let _ = frame;
        // Cloning the context is an Arc bump, and it frees the borrow on `ui`
        // that the panel calls below need.
        let ctx = ui.ctx().clone();
        let ctx = &ctx;

        let p = self.palette();
        self.textures.begin_frame();
        self.drain_service(ctx);
        self.drain_assistant(ctx);

        // A flat ground behind the panels. Nothing drifts any more, so this
        // also drops the continuous 30fps repaint the ambient light needed —
        // Quark now idles at zero frames like any other document viewer.
        ui.painter()
            .rect_filled(ui.max_rect(), egui::CornerRadius::ZERO, p.ground);

        // Age out toasts.
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        for t in self.toasts.iter_mut() {
            t.ttl -= dt;
        }
        self.toasts.retain(|t| t.ttl > 0.0);

        // Files dropped onto the window.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        for f in dropped {
            self.open_path(&f, None);
        }

        // Keyboard, but not while a dialog or the palette has focus — otherwise
        // typing a password would fire every accelerator it contains.
        if self.dialog.is_none() && !self.show_palette {
            let typing = ctx.memory(|m| m.focused().is_some());
            if let Some(cmd) = ctx.input(|i| shortcuts::matched(i, &self.bindings)) {
                self.dispatch(cmd, ctx);
            }
            if !typing {
                if let Some(tool) = ctx.input(shortcuts::tool_shortcut) {
                    self.tool = tool;
                }
                // Escape abandons whatever a tool is part-way through, which is
                // the only way out of a half-drawn polygon.
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    if let Some(t) = self.tab_mut() {
                        if t.drafting.is_active() {
                            t.drafting = crate::tab::Drafting::None;
                        } else {
                            t.selection = None;
                            t.selected_annotation = None;
                        }
                    }
                }
            }
        }

        // Panel order is layout order: egui hands each panel the rect the
        // previous ones left behind. The menu spans the window, then the three
        // left columns and the right dock claim full height, and only then do
        // the toolbar and tab strip run across what remains — which is what
        // puts them over the document rather than over the whole window.
        self.menu_bar(ui, &p);

        if !self.home.visible {
            self.tool_rail(ui, &p);
            self.nav_sidebar(ui, &p);
        }
        self.side_panel(ui, &p);
        self.dock_panel(ui, &p);

        // The toolbar acts on a document, so on home it would be a full row of
        // greyed-out controls. The menu bar still covers everything reachable
        // without one.
        if self.prefs.show_toolbar && !self.home.visible {
            self.toolbar(ui, &p);
        }
        self.tab_strip(ui, &p);
        if self.prefs.show_status_bar {
            self.status_bar(ui, &p);
        }


        // Set by the home screen's action cards; dispatched once the central
        // panel has given back its borrow on `self`.
        let mut home_command: Option<Command> = None;
        let action = egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p.canvas)
                    .stroke(egui::Stroke::new(1.0, p.border))
                    .corner_radius(egui::CornerRadius::same(theme::RADIUS_GLASS))
                    .outer_margin(egui::Margin::same(theme::GLASS_INSET)),
            )
            .show(ui, |ui| {
                if self.home.visible {
                    match crate::home::show(ui, &p, &mut self.home, &self.prefs) {
                        crate::home::Action::None => {}
                        // Queued rather than opened here: `open_path` needs
                        // `&mut self` and the central panel is still borrowing
                        // it. The queue is drained a few lines below.
                        crate::home::Action::Open(path) => self.startup_files.push(path),
                        crate::home::Action::Run(command) => home_command = Some(command),
                    }
                    return ViewerAction::None;
                }
                let index = self.active;
                let (service, textures, prefs, tool) =
                    (&self.service, &mut self.textures, &self.prefs, self.tool);
                match self.tabs.get_mut(index) {
                    Some(tab) => viewer::show(ui, &p, tab, service, textures, prefs, tool),
                    None => ViewerAction::None,
                }
            })
            .inner;
        self.handle_viewer_action(action, ctx);
        if let Some(cmd) = home_command {
            self.dispatch(cmd, ctx);
        }

        // Anything the home screen queued.
        let queued = std::mem::take(&mut self.startup_files);
        for f in queued {
            self.open_path(&f, None);
        }

        self.command_palette(ctx, &p);
        self.handle_dialog(ctx, &p);

        // Keep the remembered reading position current.
        if let Some(tab) = self.tab() {
            if let Some(path) = tab.path() {
                let key = path.to_string_lossy().into_owned();
                let page = tab.current_page;
                if let Some(r) = self.prefs.recent.iter_mut().find(|r| r.path == key) {
                    r.page = page;
                }
            }
        }

        // A page still rendering means another frame is coming; ask for it so
        // the document appears without waiting for the next input event.
        let waiting = self
            .tab()
            .map(|t| t.page_count() > 0 && self.textures.len() == 0)
            .unwrap_or(false);
        if waiting {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn on_exit(&mut self) {
        save_prefs(&self.prefs);
    }
}


/// The tools the rail offers, in order.
///
/// Not every tool Quark has — the rail is for the ones reached constantly while
/// reading and marking up. The rest stay in the Tools menu and the command
/// palette, because a rail of thirty icons is a menu that pretends to be a
/// toolbar.
const TOOL_RAIL: [(Tool, Icon, &str); 7] = [
    (Tool::Select, Icon::Cursor, "Select"),
    (Tool::Pan, Icon::Hand, "Pan"),
    (Tool::Highlight, Icon::Highlight, "Highlight"),
    (Tool::Note, Icon::Note, "Note"),
    (Tool::Ink, Icon::Pen, "Draw"),
    (Tool::Rectangle, Icon::Square, "Shapes"),
    (Tool::Redact, Icon::Redact, "Redact"),
];

/// The structural panels the nav sidebar switches between.
///
/// Comments is deliberately absent: it lives in the right dock with the
/// assistant, because both are a conversation about the document rather than a
/// view of its structure.
const NAV_PANELS: [(SidePanel, Icon, &str); 5] = [
    (SidePanel::Thumbnails, Icon::Thumbnails, "Thumbnails"),
    (SidePanel::Bookmarks, Icon::Bookmark, "Outline"),
    (SidePanel::Fields, Icon::Fields, "Form fields"),
    (SidePanel::Attachments, Icon::Attachment, "Attachments"),
    (SidePanel::Layers, Icon::Layers, "Layers"),
];


/// Width reserved for the toolbar's centred group.
///
/// Fixed rather than measured: the group is placed at an explicit rect so it
/// stays on the bar's centre line no matter how long the filename on the left
/// or the icon set on the right becomes.
const CENTRE_GROUP_WIDTH: f32 = 420.0;
