//! Window chrome and the frame loop: menu bar, toolbar, tabs, panels, status
//! bar, command palette, and the dispatch that turns a [`Command`] into work.

use std::path::PathBuf;

use egui::{Context, RichText};
use quark_core::annot::AnnotKind;
use quark_core::command::{Command, Menu, command_table};
use quark_core::geom::{Rot, Vec2};
use quark_core::layout::{self, ZoomMode};
use quark_core::prefs::SidePanel;
use quark_core::tools::Tool;
use quark_pdf::export::{BatesOptions, ImageExport, TextStamp};
use quark_pdf::security::{RedactionMark, SanitizeOptions};
use quark_pdf::service::{DocOp, Request};
use quark_ui::icons::{self, Icon};
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

    /// The row across the top: the brand (which holds every menu), the open
    /// documents, command search and the theme.
    ///
    /// On macOS the window content runs under a transparent titlebar, so this
    /// row *is* the title bar: it leaves room for the traffic lights, and its
    /// empty space moves the window.
    fn title_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // Panels are drawn into the parent `Ui` in this version of egui; the
        // context is still needed for dispatch, and cloning it is just an Arc.
        let ctx = &ui.ctx().clone();
        let lead = if cfg!(target_os = "macos") {
            theme::TRAFFIC_LIGHT_INSET
        } else {
            theme::GAP
        };
        egui::Panel::top("titlebar")
            .exact_size(theme::TITLE_BAR_HEIGHT)
            .show_separator_line(false)
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: lead as i8,
                right: GAP_I,
                top: 0,
                bottom: 0,
            }))
            .show(ui, |ui| {
                // Registered before anything is drawn on the bar, so every
                // control below wins the pointer: only empty space drags.
                let bar = ui.max_rect();
                let drag = ui.interact(
                    bar,
                    egui::Id::new("titlebar-drag"),
                    egui::Sense::click_and_drag(),
                );
                if drag.drag_started_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    self.brand_menu(ui, p, ctx);
                    widgets::separator(ui, p);

                    // The tabs get whatever command search and the theme
                    // toggle leave, and scroll sideways past that.
                    let reserve = SEARCH_WIDTH + 36.0 + 24.0;
                    let room = (ui.available_width() - reserve).max(140.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(room, 40.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            egui::ScrollArea::horizontal()
                                .id_salt("tab-scroll")
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                )
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    ui.horizontal_centered(|ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;
                                        self.tabs_row(ui, p, ctx);
                                    });
                                });
                        },
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (icon, tip) = if self.theme.is_dark() {
                            (Icon::Sun, "Switch to the light theme")
                        } else {
                            (Icon::Moon, "Switch to the dark theme")
                        };
                        if widgets::icon_button(ui, p, icon, tip, 36.0, false, true).clicked() {
                            self.dispatch(Command::ToggleTheme, ctx);
                        }
                        ui.add_space(4.0);
                        if command_search(ui, p).clicked() {
                            self.dispatch(Command::CommandPalette, ctx);
                        }
                    });
                });
            });
    }

    /// The brand button, which opens every menu Quark has.
    ///
    /// Ten menus across the top were a second toolbar of words; behind the
    /// brand they are one click away, and the command palette beside them
    /// reaches the same commands by name.
    fn brand_menu(&mut self, ui: &mut egui::Ui, p: &Palette, ctx: &Context) {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(104.0, 36.0), egui::Sense::click());
        let open = egui::Popup::is_id_open(ctx, egui::Popup::default_response_id(&response));
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            if open || response.hovered() {
                painter.rect_filled(
                    rect,
                    egui::CornerRadius::same(theme::RADIUS_CONTROL),
                    p.wash(if open { 0x14 } else { 0x0d }),
                );
            }
            widgets::brand_mark(
                painter,
                egui::Rect::from_center_size(
                    rect.left_center() + egui::vec2(19.0, 0.0),
                    egui::Vec2::splat(22.0),
                ),
                p,
            );
            painter.text(
                rect.left_center() + egui::vec2(38.0, 0.0),
                egui::Align2::LEFT_CENTER,
                "Quark",
                theme::heavy(14.0),
                p.text,
            );
            icons::draw(
                painter,
                egui::Rect::from_center_size(
                    rect.right_center() - egui::vec2(15.0, 0.0),
                    egui::Vec2::splat(12.0),
                ),
                Icon::ChevronDown,
                p.text_faint,
            );
        }
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

        let table = command_table();
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_width(190.0);
            for menu in Menu::ALL {
                ui.menu_button(menu.label(), |ui| {
                    ui.set_min_width(270.0);
                    for info in table.iter().filter(|c| c.menu == menu) {
                        let enabled = self.command_enabled(&info.command, info.mutates);
                        let button = egui::Button::new(info.label)
                            .shortcut_text(widgets::platform_keys(info.shortcut));
                        if ui.add_enabled(enabled, button).clicked() {
                            self.dispatch(info.command.clone(), ctx);
                            ui.close();
                        }
                    }
                });
            }
        });
    }

    /// Home, then one tab per open document, then the open button.
    fn tabs_row(&mut self, ui: &mut egui::Ui, p: &Palette, ctx: &Context) {
        let mut to_close = None;
        let mut to_select = None;
        // Home first and always, with no close button — there is nothing to
        // close, and an X that does nothing invites the click anyway.
        let go_home = title_tab(ui, p, Icon::Home, "Home", self.home.visible, false, false).clicked;
        for (i, tab) in self.tabs.iter().enumerate() {
            let active = !self.home.visible && i == self.active;
            let title = widgets::elide(&tab.title(), 26, true);
            let hit = title_tab(ui, p, Icon::Document, &title, active, tab.has_unsaved_changes(), true);
            if hit.close {
                to_close = Some(i);
            } else if hit.clicked {
                to_select = Some(i);
            }
        }
        if widgets::icon_button(
            ui,
            p,
            Icon::Plus,
            &widgets::platform_keys("Open a file (Ctrl+O)"),
            32.0,
            false,
            true,
        )
        .clicked()
        {
            self.dispatch(Command::Open, ctx);
        }

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
            Command::Undo => tab.info.undo.is_some(),
            Command::Redo => tab.info.redo.is_some(),
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
            // ceiling the user never chose. Thinking counts against it too, so
            // it is sized for the reasoning as well as the reply.
            64_000,
        );

        // The credential is fetched per turn rather than held: it is then never
        // resident in Quark's memory between questions.
        let provider: Box<dyn quark_ai::Provider> = match backend {
            quark_ai::Backend::Ollama => Box::new(quark_ai::Ollama::local()),
            quark_ai::Backend::Anthropic => {
                Box::new(quark_ai::Anthropic::new(self.stored_key(backend)))
            }
            // Answers on the user's Claude subscription through their own
            // `claude` CLI. If it was not found, the bare name is tried, and a
            // failure to start comes back as a message saying how to fix it.
            quark_ai::Backend::ClaudeCode => {
                // The tools the reader has switched on, confined to the open
                // document's folder.
                let document = self.tab().and_then(|t| t.path().map(|p| p.to_path_buf()));
                let access = quark_ai::claude_code::Access {
                    files: self.prefs.ai_files,
                    edits: self.prefs.ai_edits,
                    web: self.prefs.ai_web,
                    folder: document.as_ref().and_then(|d| d.parent().map(|f| f.to_path_buf())),
                    document,
                };
                Box::new(
                    quark_ai::ClaudeCode::new(
                        self.ai
                            .claude_path
                            .clone()
                            .or_else(quark_ai::ClaudeCode::locate)
                            .unwrap_or_else(|| PathBuf::from("claude")),
                    )
                    .with_access(access),
                )
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
            quark_ai::Backend::Ollama | quark_ai::Backend::ClaudeCode => return String::new(),
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
            // Claude Code runs locally, but the model behind it does not, so
            // the warning names where the text actually goes.
            let (title, body) = if backend == quark_ai::Backend::ClaudeCode {
                (
                    "Claude Code answers from Anthropic's servers".to_string(),
                    "Asking a question sends the pages in scope, and any selected \
                     text, to Anthropic on your Claude account.",
                )
            } else {
                (
                    format!("{} runs in the cloud", backend.label()),
                    "Asking a question sends the pages in scope, and any selected \
                     text, to it.",
                )
            };
            theme::card(p).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(title).font(theme::strong(13.0)).color(p.text));
                ui.add_space(2.0);
                ui.label(RichText::new(body).font(theme::body(12.5)).color(p.text_muted));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, p, "I understand", true).clicked() {
                        self.ai.egress_acknowledged = true;
                    }
                    if widgets::text_button(ui, p, "Use Ollama instead", false).clicked() {
                        self.prefs.ai_backend =
                            assistant::backend_key(quark_ai::Backend::Ollama).into();
                        self.prefs.ai_model = quark_ai::Backend::Ollama.default_model().into();
                    }
                });
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
                quark_ai::Backend::ClaudeCode => Box::new(quark_ai::ClaudeCode::new("claude")),
                quark_ai::Backend::OpenAiCompat => Box::new(quark_ai::OpenAiCompat::new(
                    "OpenAI-compatible",
                    self.prefs.ai_base_url.clone(),
                    String::new(),
                )),
            };
            self.ai.service.list_models(lister);
        }

        // Re-checked only when the backend changes: reading the credential
        // store is a syscall, and doing it every frame is sixty a second.
        if self.ai.key_state.map(|(b, _)| b) != Some(backend) {
            self.ai.key_state = Some((backend, assistant::key_source(backend)));
            if backend == quark_ai::Backend::ClaudeCode {
                self.ai.claude_path = quark_ai::ClaudeCode::locate();
            }
        }
        let source = self.ai.key_state.map(|(_, s)| s).unwrap_or(assistant::KeySource::Missing);

        theme::card(p).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("ai-backend")
                    .selected_text(RichText::new(backend.label()).font(theme::strong(12.0)))
                    .width(112.0)
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
                    .selected_text(
                        RichText::new(widgets::elide(&model, 20, true))
                            .font(theme::body(12.0))
                            .color(p.text_muted),
                    )
                    .width(ui.available_width().max(60.0))
                    .show_ui(ui, |ui| {
                        if self.ai.models.is_empty() {
                            ui.label(
                                RichText::new(match backend {
                                    quark_ai::Backend::Ollama => "No models found — is Ollama running?",
                                    _ => "Type a model name in Settings.",
                                })
                                .font(theme::body(11.5))
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

            // --- credentials ---
            //
            // Claude Code has none to manage: the CLI holds the sign-in. What
            // matters is whether the CLI is there at all.
            if backend == quark_ai::Backend::ClaudeCode {
                let found = self.ai.claude_path.clone();
                let (label, colour) = match &found {
                    Some(_) => ("Uses your Claude Code sign-in", p.success),
                    None => ("Claude Code not found", p.warning),
                };
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.5, colour);
                    let status = ui.label(RichText::new(label).font(theme::strong(11.5)).color(colour));
                    if let Some(path) = &found {
                        status.on_hover_text(path.display().to_string());
                    } else {
                        // Only worth offering while it is missing: installed
                        // since the panel looked is the case this is for.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::ghost_button(ui, p, "Check again").clicked() {
                                self.ai.claude_path = quark_ai::ClaudeCode::locate();
                            }
                        });
                    }
                });
                if found.is_none() {
                    ui.label(
                        RichText::new(format!(
                            "Install Claude Code from claude.com/claude-code and run \
                             `claude` once to sign in, or set {} to its path.",
                            quark_ai::claude_code::PATH_OVERRIDE
                        ))
                        .font(theme::body(11.0))
                        .color(p.text_faint),
                    );
                }
            } else if backend.leaves_the_machine() {
                let (label, colour) = match source {
                    assistant::KeySource::Environment => (
                        format!("Key from {}", assistant::env_var(backend).unwrap_or("")),
                        p.success,
                    ),
                    assistant::KeySource::Stored => ("Key stored".to_string(), p.success),
                    assistant::KeySource::Missing => ("No API key set".to_string(), p.warning),
                };
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.5, colour);
                    ui.label(RichText::new(label).font(theme::strong(11.5)).color(colour));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let toggle = if self.ai.settings_open { "Hide" } else { "Set up" };
                        if widgets::ghost_button(ui, p, toggle).clicked() {
                            self.ai.settings_open = !self.ai.settings_open;
                            self.ai.key_input.clear();
                        }
                    });
                });
            }

            if self.ai.settings_open
                && backend.leaves_the_machine()
                && backend.credential_key().is_some()
            {
                ui.add_space(6.0);
                // Base URL, for the one backend that needs to be pointed somewhere.
                if backend == quark_ai::Backend::OpenAiCompat {
                    widgets::panel_header(ui, p, "Endpoint");
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
                            .font(theme::body(11.5)),
                    );
                    ui.add_space(6.0);

                    // These endpoints cannot be enumerated — a proxy serves
                    // models under whatever names its operator chose — so the
                    // model is typed rather than picked.
                    widgets::panel_header(ui, p, "Model");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.prefs.ai_model)
                            .desired_width(f32::INFINITY)
                            .hint_text("gpt-4o-mini, gemini-2.5-flash, …")
                            .font(theme::body(11.5)),
                    );
                    ui.add_space(6.0);
                }

                widgets::panel_header(ui, p, "API key");
                // Masked, and a stored key is never read back into the field:
                // the credential store is write-and-use.
                ui.add(
                    egui::TextEdit::singleline(&mut self.ai.key_input)
                        .password(true)
                        .desired_width(f32::INFINITY)
                        .hint_text(match source {
                            assistant::KeySource::Stored => "Replace the stored key",
                            _ => "Paste your key",
                        })
                        .font(theme::body(11.5)),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let typed = !self.ai.key_input.trim().is_empty();
                    if widgets::primary_button(ui, p, "Save", typed).clicked() && typed {
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
                        .font(theme::body(11.0))
                        .color(p.text_faint),
                    );
                }
            }

            // The configured model has to exist, or the failure only shows up
            // as a 404 at the moment a question is asked.
            if !self.ai.models.is_empty() && !self.ai.models.contains(&self.prefs.ai_model) {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "\u{201c}{}\u{201d} is not installed. Pick one above.",
                        self.prefs.ai_model
                    ))
                    .font(theme::body(11.5))
                    .color(p.warning),
                );
            }
        });
        ui.add_space(10.0);

        // --- how much of the document goes with the question ---
        //
        // Kept in the panel rather than buried in settings: it changes what is
        // sent and what it costs, so it belongs where the question is asked.
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(
                RichText::new(widgets::micro_caps("Context"))
                    .font(theme::strong(10.5))
                    .color(p.text_faint),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let current = assistant::scope_of(&self.prefs);
                let index = quark_ai::Scope::ALL
                    .iter()
                    .position(|s| *s == current)
                    .unwrap_or(0);
                let segments = [
                    widgets::Segment::Text("Page"),
                    widgets::Segment::Text("Nearby"),
                    widgets::Segment::Text("Whole"),
                ];
                if let Some(i) = widgets::segmented(ui, p, &segments, index, false, 26.0) {
                    self.prefs.ai_scope = assistant::scope_key(quark_ai::Scope::ALL[i]).into();
                }
            });
        });
        ui.add_space(8.0);

        // --- what Claude Code may do beyond answering ---
        //
        // Beside the scope for the same reason: it changes what leaves the
        // machine. Files and edits are confined to the document's folder, so
        // an unsaved document, which has none, cannot offer them.
        if backend == quark_ai::Backend::ClaudeCode {
            let has_folder = self.tab().and_then(|t| t.path()).is_some();
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(widgets::micro_caps("Tools"))
                        .font(theme::strong(10.5))
                        .color(p.text_faint),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let unsaved = "Save the document first: file access is confined to its folder";
                    for (label, on, tip, available) in [
                        (
                            "Edits",
                            &mut self.prefs.ai_edits,
                            "Let it create and change files in this document's folder. PDFs stay read-only.",
                            has_folder,
                        ),
                        (
                            "Web",
                            &mut self.prefs.ai_web,
                            "Let it search the web and open pages",
                            true,
                        ),
                        (
                            "Files",
                            &mut self.prefs.ai_files,
                            "Let it read and search files in this document's folder, the PDF included",
                            has_folder,
                        ),
                    ] {
                        let response = ui
                            .add_enabled_ui(available, |ui| {
                                widgets::text_button(ui, p, label, *on && available)
                            })
                            .inner;
                        if response.clicked() {
                            *on = !*on;
                        }
                        if available {
                            response.on_hover_text(tip);
                        } else {
                            response.on_disabled_hover_text(unsaved);
                        }
                    }
                });
            });
            ui.add_space(8.0);
        }

        // --- conversation ---
        let composer = 60.0;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .max_height((ui.available_height() - composer).max(60.0))
            .show(ui, |ui| {
                if self.ai.turns.is_empty() {
                    ui.add_space(28.0);
                    ui.vertical_centered(|ui| {
                        let (plate, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
                        ui.painter().circle_filled(plate.center(), 22.0, p.accent_soft);
                        icons::draw(
                            ui.painter(),
                            egui::Rect::from_center_size(plate.center(), egui::Vec2::splat(20.0)),
                            Icon::Sparkle,
                            p.accent_text,
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Ask about the document")
                                .font(theme::strong(13.0))
                                .color(p.text_muted),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{} · {}",
                                backend.label(),
                                assistant::scope_of(&self.prefs).label()
                            ))
                            .font(theme::body(11.5))
                            .color(p.text_faint),
                        );
                    });
                    return;
                }
                let width = ui.available_width();
                for turn in &self.ai.turns {
                    ui.add_space(10.0);
                    if turn.mine {
                        // The reader's own words, in a bubble on the right.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            egui::Frame::new()
                                .fill(p.accent_soft)
                                .stroke(egui::Stroke::new(1.0, p.accent_alpha(0x50)))
                                .corner_radius(egui::CornerRadius { nw: 16, ne: 16, sw: 16, se: 6 })
                                .inner_margin(egui::Margin::symmetric(12, 9))
                                .show(ui, |ui| {
                                    ui.set_max_width(width * 0.84);
                                    // Wrapped explicitly: in a right-to-left
                                    // row a label extends instead, and a long
                                    // question would push the dock wider.
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&turn.text).font(theme::body(12.5)).color(p.text),
                                        )
                                        .wrap(),
                                    );
                                });
                        });
                        continue;
                    }
                    ui.horizontal(|ui| {
                        let (plate, _) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                        ui.painter().circle_filled(plate.center(), 12.0, p.wash(0x10));
                        icons::draw(
                            ui.painter(),
                            egui::Rect::from_center_size(plate.center(), egui::Vec2::splat(13.0)),
                            Icon::Sparkle,
                            p.accent_text,
                        );
                        ui.label(RichText::new(backend.label()).font(theme::strong(12.0)).color(p.text));
                    });
                    // What was read, searched or opened on the way, so the
                    // answer's sources are on screen with it.
                    for step in &turn.activity {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 7.0;
                            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::hover());
                            let refused = step.starts_with("Not allowed");
                            ui.painter().circle_filled(
                                dot.center(),
                                2.5,
                                if refused { p.warning } else { p.accent_alpha(0x99) },
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(step)
                                        .font(theme::body(11.5))
                                        .color(if refused { p.warning } else { p.text_faint }),
                                )
                                .wrap(),
                            );
                        });
                    }
                    if !turn.thinking.is_empty() {
                        egui::CollapsingHeader::new(
                            RichText::new("Reasoning").font(theme::strong(11.5)).color(p.text_faint),
                        )
                        .id_salt(turn as *const _ as usize)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(&turn.thinking)
                                    .font(theme::body(11.5))
                                    .color(p.text_faint),
                            );
                        });
                    }
                    if !turn.text.is_empty() {
                        ui.label(RichText::new(&turn.text).font(theme::body(12.5)).color(p.text));
                    }
                    if let Some(note) = &turn.note {
                        ui.label(RichText::new(note).font(theme::body(11.5)).color(p.warning));
                    }
                }
                ui.add_space(8.0);
            });

        // --- composer ---
        ui.add_space(8.0);
        let busy = self.ai.in_flight.is_some();
        let mut send = false;
        theme::recess(p)
            .corner_radius(egui::CornerRadius::same(theme::RADIUS_CARD))
            .inner_margin(egui::Margin { left: 12, right: 5, top: 5, bottom: 5 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let width = ui.available_width() - 42.0;
                    let field = ui.add_enabled(
                        !busy,
                        egui::TextEdit::singleline(&mut self.ai.draft)
                            .frame(egui::Frame::NONE)
                            .font(theme::body(12.5))
                            .margin(egui::Margin::symmetric(0, 8))
                            .desired_width(width)
                            .hint_text("Ask about this document"),
                    );
                    // Enter sends, so the composer behaves like every other chat box.
                    if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        send = true;
                    }
                    if busy {
                        if widgets::icon_button(ui, p, Icon::Close, "Stop", 34.0, true, true).clicked() {
                            self.ai.service.cancel();
                        }
                    } else if send_button(ui, p, !self.ai.draft.trim().is_empty()).clicked() {
                        send = true;
                    }
                });
            });
        if send && self.ai.can_send() {
            self.send_question();
        }
    }

    /// The tool dock: the markup tools, in a floating column at the left.
    fn tool_dock(&mut self, ui: &mut egui::Ui, p: &Palette) {
        egui::Panel::left("rail")
            .exact_size(theme::RAIL_WIDTH + theme::GAP * 1.5)
            .resizable(false)
            .show_separator_line(false)
            .frame(island_frame(
                8,
                egui::Margin { left: GAP_I, right: HALF_I, top: 0, bottom: GAP_I },
            ))
            .show(ui, |ui| {
                paint_island(ui, 8.0, p);
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let has_doc = self.tab().is_some();
                    for (g, group) in TOOL_GROUPS.iter().enumerate() {
                        if g > 0 {
                            dock_rule(ui, p);
                        }
                        for &(tool, icon, tip) in group.iter() {
                            let active = has_doc && self.tool == tool;
                            let tip = widgets::platform_keys(tip);
                            if widgets::rail_button(ui, p, icon, &tip, active, has_doc).clicked() {
                                self.tool = tool;
                            }
                        }
                    }
                });
            });
    }

    /// The right-hand dock: comments, and the assistant.
    fn dock_panel(&mut self, ui: &mut egui::Ui, p: &Palette) {
        if !self.dock_open || self.tab().is_none() {
            return;
        }
        let mut action = PanelAction::None;
        let frame = island_frame(
            10,
            egui::Margin { left: HALF_I, right: GAP_I, top: 0, bottom: GAP_I },
        );
        // The width is stored through `total_margin`, because `default_size`
        // is an outer width while `available_width` is the width inside the
        // frame. Reading one and writing the other is what walked the side
        // panel down to its floor once per launch.
        let chrome_width = frame.total_margin().sum().x;
        let count = self.tab().map(|t| t.all_annotations().len()).unwrap_or(0);

        egui::Panel::right("dock")
            .resizable(true)
            .default_size(self.prefs.dock_width.max(theme::DOCK_MIN_WIDTH))
            .size_range(theme::DOCK_MIN_WIDTH..=theme::DOCK_MAX_WIDTH)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                paint_island(ui, 10.0, p);
                self.prefs.dock_width = ui.available_width() + chrome_width;

                let segments = [
                    widgets::Segment::Counted("Comments", count),
                    widgets::Segment::Labelled(Icon::Sparkle, "Assistant"),
                ];
                let current = match self.dock {
                    Dock::Comments => 0,
                    Dock::Assistant => 1,
                };
                if let Some(i) = widgets::segmented(ui, p, &segments, current, true, 32.0) {
                    self.dock = if i == 0 { Dock::Comments } else { Dock::Assistant };
                }
                ui.add_space(12.0);

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

    /// The header row at the top of the canvas well: which document this is
    /// on the left, the document's actions on the right.
    ///
    /// Page and zoom are not here: they float in the capsule at the foot of
    /// the canvas, beside what they act on. The tool strip and the panel
    /// toggles that used to crowd a toolbar are gone too — the dock owns tools
    /// and the pages island owns panels, and a control that exists in two
    /// places teaches that neither one is authoritative.
    fn canvas_header(&mut self, ui: &mut egui::Ui, p: &Palette, ctx: &Context) {
        let (bar, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), theme::TOOLBAR_HEIGHT),
            egui::Sense::hover(),
        );
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(bar.shrink2(egui::vec2(10.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut row;
        ui.spacing_mut().item_spacing.x = 2.0;
        let has_doc = self.tab().is_some();

        let side_open = self.prefs.side_panel != SidePanel::None;
        let tip = if side_open { "Hide the side panel (F4)" } else { "Show the side panel (F4)" };
        if widgets::tool_button(ui, p, Icon::SidebarLeft, tip, side_open, has_doc).clicked() {
            self.prefs.side_panel = if side_open {
                SidePanel::None
            } else {
                self.last_side_panel
            };
        }
        ui.add_space(8.0);

        // --- which document this is ---
        //
        // The folder is the disambiguator, and the page size and protection
        // that the old status bar carried ride along on the same line.
        let (name, detail) = self
            .tab()
            .map(|t| {
                let name = widgets::elide(&t.title(), 32, true);
                let mut parts = Vec::new();
                if let Some(dir) = t.path().and_then(|q| q.parent()) {
                    // The end of a folder path is the part that tells two
                    // documents apart, so that is the part kept.
                    parts.push(widgets::elide(&tilde(dir), 34, true));
                }
                let size = t.info.page_size(t.current_page);
                parts.push(format!("{:.2} × {:.2} in", size.width / 72.0, size.height / 72.0));
                if t.info.encrypted {
                    parts.push("Protected".to_string());
                }
                if t.info.likely_scanned {
                    parts.push("No text layer — scanned?".to_string());
                }
                (name, parts.join("  ·  "))
            })
            .unwrap_or_else(|| (String::from("No document"), String::new()));
        widgets::file_chip(ui, p, &name, &detail);

        // --- what can be done to it ---
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if widgets::tool_button(ui, p, Icon::Sidebar, "Comments and assistant", self.dock_open, has_doc)
                .clicked()
            {
                self.dock_open = !self.dock_open;
            }
            if widgets::tool_button(ui, p, Icon::Search, &widgets::platform_keys("Find (Ctrl+F)"), false, has_doc)
                .clicked()
            {
                self.dispatch(Command::Find, ctx);
            }
            widgets::separator(ui, p);
            if widgets::tool_button(ui, p, Icon::Print, &widgets::platform_keys("Print (Ctrl+P)"), false, has_doc)
                .clicked()
            {
                self.dispatch(Command::Print, ctx);
            }
            let dirty = self.tab().map(|t| t.has_unsaved_changes()).unwrap_or(false);
            let save = widgets::tool_button(
                ui,
                p,
                Icon::Save,
                &widgets::platform_keys(if dirty { "Save (Ctrl+S) — unsaved changes" } else { "Save (Ctrl+S)" }),
                false,
                has_doc,
            );
            if dirty {
                ui.painter().circle(
                    save.rect.right_top() + egui::vec2(-7.0, 7.0),
                    3.5,
                    p.accent,
                    egui::Stroke::new(2.0, p.canvas),
                );
            }
            if save.clicked() {
                self.save(false);
            }
            widgets::separator(ui, p);
            // Each says what it would do: "Undo turn page 2".
            let redo = self.tab().and_then(|t| t.info.redo.clone());
            let tip = match &redo {
                Some(said) => format!("Redo {said} (Ctrl+Y)"),
                None => "Redo (Ctrl+Y)".into(),
            };
            if widgets::tool_button(ui, p, Icon::Redo, &widgets::platform_keys(&tip), false, redo.is_some()).clicked() {
                self.dispatch(Command::Redo, ctx);
            }
            let undo = self.tab().and_then(|t| t.info.undo.clone());
            let tip = match &undo {
                Some(said) => format!("Undo {said} (Ctrl+Z)"),
                None => "Undo (Ctrl+Z)".into(),
            };
            if widgets::tool_button(ui, p, Icon::Undo, &widgets::platform_keys(&tip), false, undo.is_some()).clicked() {
                self.dispatch(Command::Undo, ctx);
            }
        });
    }

    /// How OCR is getting on, and Stop, floating at the top of the canvas.
    fn ocr_capsule(&mut self, ctx: &Context, p: &Palette, canvas: egui::Rect) {
        let Some(run) = &self.ocr else { return };
        let said = if run.done >= run.of {
            "Laying the words over the pages…".to_string()
        } else {
            format!("Reading page {} of {} with OCR…", run.done + 1, run.of)
        };
        let mut stop = false;
        egui::Area::new(egui::Id::new("ocr-capsule"))
            .order(egui::Order::Middle)
            .movable(false)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(canvas.center_top() + egui::vec2(0.0, 14.0))
            .show(ctx, |ui| {
                widgets::capsule(ui, p, |ui| {
                    ui.add(egui::Spinner::new().size(13.0));
                    ui.label(RichText::new(said).font(theme::body(12.5)).color(p.text));
                    if widgets::ghost_button(ui, p, "Stop").clicked() {
                        stop = true;
                    }
                });
            });
        if stop {
            if let Some(run) = &self.ocr {
                run.stop();
            }
        }
    }

    /// The page and zoom controls, floating at the foot of the canvas.
    fn page_capsule(&mut self, ctx: &Context, p: &Palette, canvas: egui::Rect) {
        let Some(tab) = self.tab() else { return };
        let (page, count, scale) = (tab.current_page, tab.page_count(), tab.layout.scale);
        // A page with a label of its own says it, its number after.
        let label = tab.info.pages.get(page).and_then(|p| p.label.clone());

        egui::Area::new(egui::Id::new("page-capsule"))
            .order(egui::Order::Middle)
            .movable(false)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(canvas.center_bottom() - egui::vec2(0.0, 18.0))
            .show(ctx, |ui| {
                widgets::capsule(ui, p, |ui| {
                    if widgets::tool_button(ui, p, Icon::ChevronUp, "Previous page", false, page > 0)
                        .clicked()
                    {
                        self.dispatch(Command::PrevPage, ctx);
                    }
                    ui.add_space(4.0);
                    if count == 0 {
                        ui.label(RichText::new("—").font(theme::strong(13.0)).color(p.text_faint));
                    } else {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if let Some(label) = &label {
                            ui.label(RichText::new(label).font(theme::heavy(13.0)).color(p.text));
                            ui.label(RichText::new(" · ").font(theme::body(13.0)).color(p.text_faint));
                        }
                        ui.label(
                            RichText::new((page + 1).to_string())
                                .font(if label.is_some() { theme::body(13.0) } else { theme::heavy(13.0) })
                                .color(if label.is_some() { p.text_muted } else { p.text }),
                        );
                        ui.label(
                            RichText::new(format!(" / {count}"))
                                .font(theme::body(13.0))
                                .color(p.text_faint),
                        );
                        ui.spacing_mut().item_spacing.x = 2.0;
                    }
                    ui.add_space(4.0);
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
                    let grid = self.tab().is_some_and(|t| t.grid);
                    if widgets::tool_button(ui, p, Icon::Grid, &widgets::platform_keys("Page grid (Ctrl+Shift+G)"), grid, count > 0)
                        .clicked()
                    {
                        self.dispatch(Command::PageGrid, ctx);
                    }

                    widgets::separator(ui, p);

                    if widgets::tool_button(ui, p, Icon::ZoomOut, &widgets::platform_keys("Zoom out (Ctrl+-)"), false, true)
                        .clicked()
                    {
                        self.dispatch(Command::ZoomOut, ctx);
                    }
                    let zoom = zoom_button(ui, p, &format!("{:.0}%", scale * 100.0));
                    egui::Popup::menu(&zoom).show(|ui| {
                        ui.set_min_width(200.0);
                        for (label, mode, keys) in [
                            ("Fit Width", ZoomMode::FitWidth, "Ctrl+2"),
                            ("Fit Page", ZoomMode::FitPage, "Ctrl+0"),
                            ("Fit Height", ZoomMode::FitHeight, "Ctrl+3"),
                            ("Actual Size", ZoomMode::Actual, "Ctrl+1"),
                        ] {
                            let button = egui::Button::new(label).shortcut_text(widgets::platform_keys(keys));
                            if ui.add(button).clicked() {
                                self.dispatch(Command::SetZoom(mode), ctx);
                                ui.close();
                            }
                        }
                        ui.separator();
                        for z in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 4.0] {
                            let on = (scale - z).abs() < 0.005;
                            if ui.selectable_label(on, format!("{:.0}%", z * 100.0)).clicked() {
                                self.dispatch(Command::SetZoom(ZoomMode::Custom(z)), ctx);
                                ui.close();
                            }
                        }
                    });
                    if widgets::tool_button(ui, p, Icon::ZoomIn, &widgets::platform_keys("Zoom in (Ctrl+=)"), false, true)
                        .clicked()
                    {
                        self.dispatch(Command::ZoomIn, ctx);
                    }

                    widgets::separator(ui, p);

                    if widgets::tool_button(ui, p, Icon::FitWidth, &widgets::platform_keys("Fit width (Ctrl+2)"), false, true)
                        .clicked()
                    {
                        self.dispatch(Command::SetZoom(ZoomMode::FitWidth), ctx);
                    }
                });
            });
    }

    /// The newest messages, floating at the bottom-right of the work area.
    ///
    /// They used to share the status bar's single line, where a second
    /// message silently replaced the first; stacked, the last three are all
    /// readable.
    fn toasts_overlay(&self, ctx: &Context, p: &Palette, area: egui::Rect) {
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .order(egui::Order::Foreground)
            .movable(false)
            .interactable(false)
            .pivot(egui::Align2::RIGHT_BOTTOM)
            .fixed_pos(area.right_bottom() - egui::vec2(18.0, 20.0))
            .show(ctx, |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    let start = self.toasts.len().saturating_sub(3);
                    for t in &self.toasts[start..] {
                        toast(ui, p, &t.text, t.error, t.ttl);
                    }
                });
            });
    }

    /// Tiles the velvet grain over the window ground.
    ///
    /// Built once per theme: the grain is white in the dark theme and ink in
    /// the light one.
    fn paint_grain(&mut self, ui: &egui::Ui, rect: egui::Rect, p: &Palette) {
        let dark = p.is_dark();
        if self.grain.as_ref().map(|(d, _)| *d) != Some(dark) {
            let options = egui::TextureOptions {
                wrap_mode: egui::TextureWrapMode::Repeat,
                ..egui::TextureOptions::NEAREST
            };
            let texture = ui
                .ctx()
                .load_texture("velvet-grain", theme::grain_image(p), options);
            self.grain = Some((dark, texture));
        }
        if let Some((_, tex)) = &self.grain {
            let tile = tex.size_vec2();
            let uv = egui::Rect::from_min_max(
                egui::Pos2::ZERO,
                egui::pos2(rect.width() / tile.x, rect.height() / tile.y),
            );
            ui.painter().image(tex.id(), rect, uv, egui::Color32::WHITE);
        }
    }

    /// The pages island: whichever structural panel its segments select.
    ///
    /// The segments replace the old navigation column beside the list — one
    /// island where there were two, which gives the document back the width
    /// that column took.
    fn side_panel(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // Panels are drawn into the parent `Ui` in this version of egui; the
        // context is still needed for dispatch, and cloning it is just an Arc.
        let ctx = &ui.ctx().clone();
        // Home carries its own islands, and the document panels have no
        // document to describe while it is showing.
        if self.prefs.side_panel == SidePanel::None || self.tabs.is_empty() || self.home.visible {
            return;
        }
        let which = self.prefs.side_panel;
        self.last_side_panel = which;
        let mut action = PanelAction::None;

        let frame = island_frame(
            10,
            egui::Margin { left: HALF_I, right: HALF_I, top: 0, bottom: GAP_I },
        );
        // `default_size` is an outer width but `available_width` is the width
        // inside the frame, so storing the latter directly loses the margins
        // once per launch — the panel walks itself down to the floor over a
        // few restarts.
        let chrome_width = frame.total_margin().sum().x;

        egui::Panel::left("side")
            .resizable(true)
            .default_size(self.prefs.side_panel_width.max(theme::LIST_WIDTH))
            .size_range(200.0..=520.0)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                paint_island(ui, 10.0, p);
                self.prefs.side_panel_width = ui.available_width() + chrome_width;

                let segments: Vec<widgets::Segment<'_>> = NAV_PANELS
                    .iter()
                    .map(|&(_, icon, label)| widgets::Segment::Icon(icon, label))
                    .collect();
                let current = NAV_PANELS
                    .iter()
                    .position(|&(panel, ..)| panel == which)
                    .unwrap_or(usize::MAX);
                if let Some(i) = widgets::segmented(ui, p, &segments, current, true, 32.0) {
                    let panel = NAV_PANELS[i].0;
                    // Picking the segment already showing closes the island,
                    // the quickest way to give the document the full width.
                    self.prefs.side_panel = if panel == which { SidePanel::None } else { panel };
                }
                ui.add_space(12.0);

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
                        if t.selected_annotation != Some(id) {
                            t.reply_draft.clear();
                        }
                        t.selected_annotation = Some(id);
                        t.reveal(page, rect);
                    }
                }
            }
            PanelAction::DeleteAnnotation(id) => self.delete_annotation(id),
            PanelAction::EditText(id) => self.write_in(id),
            PanelAction::Reply { id, text } => self.reply_to(id, text),
            PanelAction::Recolor { id, color } => self.edit_annotation(id, Some(color), None),
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
                // Text and choices open for writing; boxes and buttons are
                // only shown, not changed, by choosing them in the list.
                use quark_pdf::forms::FieldKind;
                let field = self.tab().and_then(|t| t.fields.iter().find(|f| f.name == name).cloned());
                if let Some(f) = field.filter(|f| {
                    !f.read_only && matches!(f.kind, FieldKind::Text | FieldKind::ComboBox | FieldKind::ListBox)
                }) {
                    self.activate_field(f);
                }
            }
        }
    }

    fn command_palette(&mut self, ctx: &Context, p: &Palette) {
        if !self.show_palette {
            self.palette_was_open = false;
            return;
        }
        // The click that opened the palette lands on its backdrop in the same
        // frame, and a backdrop click closes it — so on the frame it opens,
        // that click is not counted.
        let just_opened = !self.palette_was_open;
        self.palette_was_open = true;
        let q = self.palette_query.to_lowercase();
        let table = command_table();
        let matches: Vec<_> = table
            .iter()
            .filter(|c| q.is_empty() || c.label.to_lowercase().contains(&q))
            .take(200)
            .collect();

        // The keyboard drives the list as well as the pointer: arrows move the
        // highlight and Enter runs it, so a command is reachable without
        // leaving the keys.
        let (down, up, enter) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
            )
        });
        let last = matches.len().saturating_sub(1);
        if down {
            self.palette_index = (self.palette_index + 1).min(last);
        }
        if up {
            self.palette_index = self.palette_index.saturating_sub(1);
        }
        self.palette_index = self.palette_index.min(last);

        let mut chosen = None;
        if enter {
            if let Some(c) = matches.get(self.palette_index) {
                if self.command_enabled(&c.command, c.mutates) {
                    chosen = Some(c.command.clone());
                }
            }
        }

        let area = egui::Modal::default_area(egui::Id::new("palette"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 96.0));
        let modal = egui::Modal::new(egui::Id::new("palette"))
            .area(area)
            .frame(theme::dialog(p).inner_margin(egui::Margin::same(8)))
            .backdrop_color(theme::scrim(p))
            .show(ctx, |ui| {
                ui.set_width(600.0);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    let (glyph, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                    icons::draw(ui.painter(), glyph, Icon::Search, p.accent_text);
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.palette_query)
                            .frame(egui::Frame::NONE)
                            .hint_text("Type a command")
                            .font(theme::body(15.0))
                            .margin(egui::Margin::symmetric(4, 12))
                            .desired_width(ui.available_width() - 52.0),
                    );
                    field.request_focus();
                    if field.changed() {
                        self.palette_index = 0;
                    }
                    widgets::kbd(ui, p, "Esc");
                });
                hairline(ui, p);

                egui::ScrollArea::vertical()
                    .max_height(380.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if matches.is_empty() {
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new(format!("No command matches \u{201c}{}\u{201d}.", self.palette_query))
                                    .font(theme::body(12.5))
                                    .color(p.text_faint),
                            );
                            ui.add_space(10.0);
                        }
                        for (i, c) in matches.iter().enumerate() {
                            let enabled = self.command_enabled(&c.command, c.mutates);
                            let (rect, response) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 42.0),
                                egui::Sense::click(),
                            );
                            // Follow the pointer only when it moves, so a list
                            // scrolled by the keyboard is not snatched back by
                            // a mouse resting over it.
                            if response.hovered() && ui.input(|inp| inp.pointer.delta() != egui::Vec2::ZERO) {
                                self.palette_index = i;
                            }
                            let selected = i == self.palette_index;
                            if selected && (up || down) {
                                response.scroll_to_me(None);
                            }
                            if ui.is_rect_visible(rect) {
                                let painter = ui.painter();
                                let radius = egui::CornerRadius::same(13);
                                if selected {
                                    painter.rect_filled(rect, radius, p.accent_soft);
                                    painter.rect_stroke(
                                        rect,
                                        radius,
                                        egui::Stroke::new(1.0, p.accent_alpha(0x60)),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                let (colour, strong) = if enabled {
                                    (if selected { p.text } else { p.text_muted }, p.text)
                                } else {
                                    (p.text_disabled, p.text_disabled)
                                };
                                let galley = painter.layout_job(highlighted(c.label, &q, colour, strong));
                                painter.galley(
                                    egui::pos2(rect.left() + 14.0, rect.center().y - galley.size().y / 2.0),
                                    galley,
                                    colour,
                                );
                                let mut x = rect.right() - 12.0;
                                x = widgets::paint_tag(painter, x, rect.center().y, c.menu.label(), p);
                                if !c.shortcut.is_empty() {
                                    widgets::paint_keys(painter, x - 10.0, rect.center().y, c.shortcut, p);
                                }
                            }
                            if response.clicked() && enabled {
                                chosen = Some(c.command.clone());
                            }
                        }
                    });

                hairline(ui, p);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.spacing_mut().item_spacing.x = 4.0;
                    widgets::kbd(ui, p, "↑");
                    widgets::kbd(ui, p, "↓");
                    ui.label(RichText::new("to move").font(theme::body(11.5)).color(p.text_faint));
                    ui.add_space(10.0);
                    widgets::kbd(ui, p, "Enter");
                    ui.label(RichText::new("to run").font(theme::body(11.5)).color(p.text_faint));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        let n = matches.len();
                        ui.label(
                            RichText::new(format!("{n} command{}", if n == 1 { "" } else { "s" }))
                                .font(theme::body(11.5))
                                .color(p.text_faint),
                        );
                    });
                });
                ui.add_space(2.0);
            });

        if modal.should_close() && !just_opened {
            self.show_palette = false;
            self.palette_query.clear();
            self.palette_index = 0;
        }
        if let Some(cmd) = chosen {
            self.show_palette = false;
            self.palette_query.clear();
            self.palette_index = 0;
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
            // Closing the window asks about unsaved work, wherever it came from.
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
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
            // Into a file of their own; the document is left as it is.
            Command::ExtractPages => {
                let pages = self.target_pages();
                let Some(tab) = self.tab() else { return };
                if pages.is_empty() {
                    return;
                }
                let stem = tab.path().and_then(|p| p.file_stem()).map_or("Pages".into(), |s| s.to_string_lossy().into_owned());
                let name = format!("{stem} (pages {}).pdf", crate::dialogs::pages_said(&pages));
                let mut ask = rfd::FileDialog::new().add_filter("PDF documents", &["pdf"]).set_file_name(&name);
                if let Some(dir) = tab.path().and_then(|p| p.parent()) {
                    ask = ask.set_directory(dir);
                }
                if let Some(path) = ask.save_file() {
                    self.service.send(Request::ExtractTo { id: tab.id, pages, path });
                }
            }
            Command::SplitDocument => {
                if let Some(t) = self.tab() {
                    self.dialog = Some(Dialog::Split {
                        pages: t.page_count(),
                        by_every: true,
                        every: "1".into(),
                        at: String::new(),
                    });
                }
            }
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
            Command::Delete if self.tab().is_some_and(|t| t.grid) => self.dispatch(Command::DeletePages, ctx),
            Command::Delete => {
                let id = self.tab().and_then(|t| t.selected_annotation);
                if let Some(id) = id {
                    self.delete_annotation(id);
                }
            }
            Command::SelectAll if self.tab().is_some_and(|t| t.grid) => {
                if let Some(t) = self.tab_mut() {
                    t.selected_pages = (0..t.page_count()).collect();
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
            // Comments live in the dock, so asking for them opens the dock
            // rather than putting a second copy in the pages island.
            Command::TogglePanel(SidePanel::Comments) => {
                let showing = self.dock_open && self.dock == Dock::Comments;
                self.dock = Dock::Comments;
                self.dock_open = !showing;
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
            Command::PageGrid => {
                if let Some(t) = self.tab_mut() {
                    t.grid = !t.grid;
                    if t.grid && t.selected_pages.is_empty() {
                        t.selected_pages = vec![t.current_page];
                    }
                }
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
            // A page on, or a spread on when two are shown at a time.
            Command::NextPage => {
                if let Some(t) = self.tab_mut() {
                    t.turn(1, false);
                }
            }
            Command::PrevPage => {
                if let Some(t) = self.tab_mut() {
                    t.turn(-1, false);
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
                    let labels = if t.info.has_labels() {
                        (0..t.page_count()).map(|i| t.info.page_label(i)).collect()
                    } else {
                        Vec::new()
                    };
                    self.dialog = Some(Dialog::GoToPage {
                        entry: String::new(),
                        max: t.page_count().max(1),
                        labels,
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
                            "Delete {} page(s)? Undo brings them back, even after saving.",
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
            Command::FlattenForm => self.apply_op(DocOp::FlattenForm),
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
            // Undone like any change; kept once saved.
            Command::RemoveSecurity => {
                self.apply_op(DocOp::Decrypt { password: String::new() });
                self.toast("Its password goes when it is saved", false);
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
            Command::RecognizeText => self.recognise_text(),
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

    /// Take back the document's last change: its log is the worker's.
    fn undo(&mut self) {
        let Some(tab) = self.tab() else { return };
        self.service.send(Request::Undo { id: tab.id });
    }

    fn redo(&mut self) {
        let Some(tab) = self.tab() else { return };
        self.service.send(Request::Redo { id: tab.id });
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
            DialogResult::Ocr { pages, langs } => self.start_ocr(pages, langs),
            DialogResult::Split { every, at } => {
                let Some(tab) = self.tab() else { return };
                let stem = tab.path().and_then(|p| p.file_stem()).map_or("Part".into(), |s| s.to_string_lossy().into_owned());
                let mut ask = rfd::FileDialog::new().set_title("Split into this folder");
                if let Some(dir) = tab.path().and_then(|p| p.parent()) {
                    ask = ask.set_directory(dir);
                }
                if let Some(folder) = ask.pick_folder() {
                    self.service.send(Request::SplitTo { id: tab.id, every, at, folder, stem });
                }
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
                Dialog::Confirm { title, .. } if title == QUIT_UNSAVED => {
                    self.quit_confirmed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                _ => {}
            },
        }
    }

    // ------------------------------------------------------------ viewer glue

    fn handle_viewer_action(&mut self, action: ViewerAction, ctx: &Context) {
        match action {
            ViewerAction::Grid(a) => self.grid_action(a, ctx),
            ViewerAction::OpenLink(url) => {
                if let Err(e) = open_link(&url) {
                    self.toast(e, true);
                }
            }
            ViewerAction::Then(first, then) => {
                self.handle_viewer_action(*first, ctx);
                self.handle_viewer_action(*then, ctx);
            }
            ViewerAction::None => {}
            // A note or text box is written before it is made: none is made
            // empty.
            ViewerAction::CreateAnnotation(ann)
                if matches!(ann.kind, AnnotKind::Note { .. } | AnnotKind::FreeText { .. }) =>
            {
                if let Some(t) = self.tab_mut() {
                    t.editing = Some(crate::tab::Editing {
                        page: ann.page,
                        rect: ann.rect,
                        text: String::new(),
                        target: crate::tab::EditTarget::New(ann),
                        focused: false,
                    });
                }
                if !self.tool.is_sticky() {
                    self.tool = Tool::Select;
                }
            }
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
            // Moved on screen as it is dragged; one change when it is let go.
            ViewerAction::MoveAnnotation { id, delta } => {
                let mut moved_page = None;
                if let Some(t) = self.tab_mut() {
                    if let Some(a) = t.find_annotation_mut(id) {
                        a.translate(delta);
                        moved_page = Some(a.page);
                    }
                    let so_far = match t.moving {
                        Some((was, by)) if was == id => by,
                        _ => Vec2::ZERO,
                    };
                    t.moving = Some((id, so_far + delta));
                }
                if let Some(page) = moved_page {
                    self.textures.invalidate_page(page);
                }
            }
            ViewerAction::MoveFinished(id) => {
                let Some(t) = self.tab_mut() else { return };
                let Some((moved, by)) = t.moving.take() else { return };
                let pending = t.pending_annotations.iter().any(|a| a.id == id);
                if moved == id && !pending && by.length() > 0.01 {
                    self.apply_op(DocOp::MoveAnnotation { id, by });
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
            ViewerAction::ActivateField(field) => self.activate_field(*field),
            ViewerAction::Written { target, text } => {
                if let Some(t) = self.tab_mut() {
                    t.editing = None;
                }
                self.keep_written(target, text);
            }
            ViewerAction::StopWriting => {
                if let Some(t) = self.tab_mut() {
                    t.editing = None;
                }
            }
            ViewerAction::EditText(id) => self.write_in(id),
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

impl App {
    /// What the page grid asked for, done to the pages chosen.
    fn grid_action(&mut self, action: crate::grid::GridAction, ctx: &Context) {
        use crate::grid::GridAction;
        match action {
            GridAction::None => {}
            GridAction::Read(page) => {
                if let Some(t) = self.tab_mut() {
                    t.grid = false;
                    t.go_to_page(page);
                }
            }
            GridAction::Turn(cw) => {
                self.dispatch(if cw { Command::RotatePagesCw } else { Command::RotatePagesCcw }, ctx)
            }
            GridAction::Delete => self.dispatch(Command::DeletePages, ctx),
            GridAction::Extract => self.dispatch(Command::ExtractPages, ctx),
            GridAction::Blank => {
                let Some(t) = self.tab() else { return };
                let Some(&last) = t.selected_pages.iter().max() else { return };
                let size = t.info.page_size(last);
                self.apply_op(DocOp::InsertBlankPage { at: last + 1, size });
                if let Some(t) = self.tab_mut() {
                    t.selected_pages = vec![last + 1];
                }
            }
            GridAction::Move { pages, before } => {
                let Some(count) = self.tab().map(|t| t.page_count()) else { return };
                if let Some((order, at)) = crate::grid::moved(count, &pages, before) {
                    let n = pages.len();
                    self.apply_op(DocOp::ReorderPages(order));
                    // Still chosen, where they now are.
                    if let Some(t) = self.tab_mut() {
                        t.selected_pages = (at..at + n).collect();
                    }
                }
            }
        }
    }

    /// A field clicked, or chosen in the Fields panel.
    fn activate_field(&mut self, field: quark_pdf::forms::FormField) {
        use crate::tab::FieldClick;
        match crate::tab::clicked(&field) {
            FieldClick::Set(value) => self.apply_op(DocOp::SetFieldValue { name: field.name, value }),
            FieldClick::Write(editing) => {
                if let Some(t) = self.tab_mut() {
                    t.editing = Some(editing);
                }
            }
            FieldClick::Nothing => {}
        }
    }

    /// What was written in place, kept.
    fn keep_written(&mut self, target: crate::tab::EditTarget, text: String) {
        use crate::tab::Kept;
        let Some(kept) = self.tab().map(|t| t.kept(target, text)) else { return };
        match kept {
            Kept::Nothing => {}
            Kept::Field { name, value, text: true } => self.apply_op(DocOp::SetFieldText { name, value }),
            Kept::Field { name, value, text: false } => self.apply_op(DocOp::SetFieldValue { name, value }),
            Kept::Make(ann) => self.commit_annotation(*ann),
            Kept::Say { id, contents } => self.edit_annotation(id, None, Some(contents)),
        }
    }

    /// Write in a note or text box already on the page, or the comment on
    /// any other mark.
    fn write_in(&mut self, id: quark_core::annot::AnnotId) {
        let Some(t) = self.tab_mut() else { return };
        let Some(editing) = t.writing_in(id) else { return };
        t.go_to_page(editing.page);
        t.selected_annotation = Some(id);
        t.editing = Some(editing);
    }
}

/// A web or mail address, handed to the browser or the mail app. Anything
/// else a document links to — a file, a custom scheme, `javascript:` — is not
/// for the desktop to run on a document's say-so.
pub(crate) fn link_opens(url: &str) -> Result<&str, String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let web = lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:");
    if !web || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("Not opened — not a web address: {url}"));
    }
    Ok(url)
}

fn open_link(url: &str) -> Result<(), String> {
    let url = link_opens(url)?;
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    // One argument, no shell: the address is never read as a command.
    std::process::Command::new(program)
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open {url}: {e}"))
}

/// The confirmation quitting with unsaved work puts up.
const QUIT_UNSAVED: &str = "Quit Without Saving";

impl App {
    /// The window asked to close — ⌘Q, the red button, Quit — with changes
    /// not saved: kept open, and asked about first.
    fn ask_before_quitting(&mut self, ctx: &Context) {
        if self.quit_confirmed || !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        let unsaved: Vec<String> = self.tabs.iter().filter(|t| t.has_unsaved_changes()).map(|t| t.title()).collect();
        if unsaved.is_empty() {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        let names = match unsaved.as_slice() {
            [one] => format!("“{one}” has"),
            [one, two] => format!("“{one}” and “{two}” have"),
            [one, rest @ ..] => format!("“{one}” and {} others have", rest.len()),
            [] => unreachable!(),
        };
        self.dialog = Some(Dialog::Confirm {
            title: QUIT_UNSAVED.into(),
            body: format!("{names} changes that are not saved. Quit anyway, and lose them?"),
            confirm_label: "Quit Anyway".into(),
            danger: true,
        });
    }
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
        self.ask_before_quitting(ctx);

        // The velvet ground: a fill, the soft light that falls on it from
        // above, and the grain. All of it is static, so Quark still idles at
        // zero frames like any other document viewer.
        let screen = ui.max_rect();
        theme::paint_ground(ui.painter(), screen, &p);
        self.paint_grain(ui, screen, &p);

        // Age out toasts. They float now rather than sitting in a status bar,
        // so they need frames of their own to fade on time.
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        for t in self.toasts.iter_mut() {
            t.ttl -= dt;
        }
        self.toasts.retain(|t| t.ttl > 0.0);
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

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

        // Files sent by macOS: each opens in a tab of this window, which
        // comes forward to show it.
        let sent: Vec<PathBuf> = self.opened.try_iter().flatten().collect();
        if !sent.is_empty() {
            for f in sent {
                tracing::info!(path = %f.display(), "opening a file macOS sent");
                self.open_path(&f, None);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        // Keyboard, but not while a dialog or the palette has focus — otherwise
        // typing a password would fire every accelerator it contains.
        if self.dialog.is_none() && !self.show_palette {
            let typing = ctx.memory(|m| m.focused().is_some());
            if let Some(cmd) = ctx.input(|i| shortcuts::matched(i, &self.bindings)) {
                // While text is being typed its keys are its own — ⌘A, ⌘C,
                // ⌘Z, Delete — and only the file's own commands get past.
                let filing = matches!(
                    cmd,
                    Command::Save | Command::SaveAs | Command::Open | Command::Close | Command::CommandPalette
                );
                if !typing || filing {
                    self.dispatch(cmd, ctx);
                }
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
        // previous ones left behind. The title bar spans the window, the
        // islands claim full height down each side, and the canvas well takes
        // what remains between them.
        self.title_bar(ui, &p);

        if !self.home.visible {
            self.tool_dock(ui, &p);
        }
        self.side_panel(ui, &p);
        self.dock_panel(ui, &p);

        let docked = !self.home.visible && self.dock_open && self.tab().is_some();
        let well = if self.home.visible {
            // Home draws its own islands inside this space.
            egui::Frame::new().outer_margin(egui::Margin {
                left: GAP_I,
                right: GAP_I,
                top: 0,
                bottom: GAP_I,
            })
        } else {
            egui::Frame::new()
                .fill(p.canvas)
                .stroke(egui::Stroke::new(1.0, p.wash(0x0a)))
                .corner_radius(egui::CornerRadius::same(theme::RADIUS_ISLAND))
                .outer_margin(egui::Margin {
                    left: HALF_I,
                    right: if docked { HALF_I } else { GAP_I },
                    top: 0,
                    bottom: GAP_I,
                })
        };

        // Set by the home screen's action cards; dispatched once the central
        // panel has given back its borrow on `self`.
        let mut home_command: Option<Command> = None;
        let mut books_action = None;
        let mut work_area = None;
        let action = egui::CentralPanel::default()
            .frame(well)
            .show(ui, |ui| {
                if self.home.visible {
                    work_area = Some(ui.max_rect());
                    match crate::home::show(ui, &p, &mut self.home, &self.prefs) {
                        crate::home::Action::None => {}
                        // Queued rather than opened here: `open_path` needs
                        // `&mut self` and the central panel is still borrowing
                        // it. The queue is drained a few lines below.
                        crate::home::Action::Open(path) => self.startup_files.push(path),
                        crate::home::Action::Run(command) => home_command = Some(command),
                        crate::home::Action::Books(a) => books_action = Some(a),
                    }
                    return ViewerAction::None;
                }

                let content = ui.max_rect();
                if self.prefs.show_toolbar {
                    self.canvas_header(ui, &p, ctx);
                }
                work_area = Some(ui.available_rect_before_wrap());

                let index = self.active;
                let (service, textures, prefs, tool) =
                    (&self.service, &mut self.textures, &self.prefs, self.tool);
                let action = match self.tabs.get_mut(index) {
                    Some(tab) if tab.grid => {
                        match crate::grid::show(ui, &p, tab, service, textures, prefs) {
                            crate::grid::GridAction::None => ViewerAction::None,
                            a => ViewerAction::Grid(a),
                        }
                    }
                    Some(tab) => viewer::show(ui, &p, tab, service, textures, prefs, tool),
                    None => ViewerAction::None,
                };

                // egui clips to rectangles, so whatever the pages left in the
                // corners of the well is rounded off here, and the well's edge
                // redrawn over the top.
                let radius = theme::RADIUS_ISLAND as f32;
                inset_shade(ui.painter(), content, &p);
                theme::mask_corners(ui.painter(), content, radius - 1.0, p.ground);
                ui.ctx().layer_painter(ui.layer_id()).rect_stroke(
                    content.expand(1.0),
                    egui::CornerRadius::same(theme::RADIUS_ISLAND),
                    egui::Stroke::new(1.0, p.wash(0x0a)),
                    egui::StrokeKind::Inside,
                );
                action
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

        if let Some(area) = work_area {
            // `show_status_bar` is the old name for this: the capsule is what
            // the status bar's page readout became.
            if !self.home.visible && self.prefs.show_status_bar {
                self.page_capsule(ctx, &p, area);
                self.ocr_capsule(ctx, &p, area);
            }
            self.toasts_overlay(ctx, &p, area);
        }

        self.command_palette(ctx, &p);
        self.handle_dialog(ctx, &p);

        if let Some(a) = books_action {
            self.books_action(a);
        }
        self.tend_library(ctx);
        self.tend_ocr(ctx);

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
        self.library.flush();
        save_prefs(&self.prefs);
    }
}


/// The tools the dock offers, in groups.
///
/// Not every tool Quark has — the dock is for the ones reached constantly while
/// reading and marking up. The rest stay in the Tools menu and the command
/// palette, because a dock of thirty icons is a menu that pretends to be a
/// toolbar.
const TOOL_GROUPS: [&[(Tool, Icon, &str)]; 3] = [
    &[
        (Tool::Select, Icon::Cursor, "Select (V)"),
        (Tool::Pan, Icon::Hand, "Pan (H)"),
    ],
    &[
        (Tool::Highlight, Icon::Highlight, "Highlight (Ctrl+Shift+H)"),
        (Tool::Note, Icon::Note, "Sticky note (Ctrl+Shift+M)"),
        (Tool::Ink, Icon::Pen, "Draw (Ctrl+Shift+D)"),
        (Tool::Rectangle, Icon::Shapes, "Shapes"),
    ],
    &[(Tool::Redact, Icon::Redact, "Redact")],
];

/// The structural panels the pages island switches between.
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

/// Width of the command search field in the title bar.
const SEARCH_WIDTH: f32 = 300.0;

/// The island gap, and half of it, as the integer margins frames take.
///
/// Neighbouring islands each take half the gap, so the space between two of
/// them is a whole gap — the same as between an island and the window edge.
const GAP_I: i8 = theme::GAP as i8;
const HALF_I: i8 = (theme::GAP / 2.0) as i8;

/// A panel frame that only spaces: the island itself is painted by
/// [`paint_island`].
fn island_frame(inner: i8, outer: egui::Margin) -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::same(inner))
        .outer_margin(outer)
}

/// Paints the island behind a panel's contents.
///
/// Not the panel frame's own fill and shadow: egui clips a panel to its rect,
/// margin included, so a frame shadow would be cut off with a hard edge a few
/// pixels outside the island. Painting through the layer's unclipped painter
/// lets the shadow fall softly into the gap.
fn paint_island(ui: &egui::Ui, inner: f32, p: &Palette) {
    let painter = ui.ctx().layer_painter(ui.layer_id());
    theme::paint_island(&painter, ui.max_rect().expand(inner), p);
}

/// The short rule between groups in the tool dock.
fn dock_rule(ui: &mut egui::Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 9.0), egui::Sense::hover());
    ui.painter().rect_filled(
        egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 1.0)),
        egui::CornerRadius::same(1),
        p.wash(0x16),
    );
}

/// A one-pixel rule across a popover.
fn hairline(ui: &mut egui::Ui, p: &Palette) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 9.0), egui::Sense::hover());
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + 6.0, rect.center().y - 0.5),
            egui::pos2(rect.right() - 6.0, rect.center().y + 0.5),
        ),
        egui::CornerRadius::same(1),
        p.wash(0x10),
    );
}

/// The command search field in the title bar.
///
/// It is a button: clicking it opens the palette, which is where typing
/// actually happens. A real field here would be a second, lesser palette.
fn command_search(ui: &mut egui::Ui, p: &Palette) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(SEARCH_WIDTH, 36.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = egui::CornerRadius::same(theme::RADIUS_CONTROL);
        let hovered = response.hovered();
        painter.rect_filled(rect, radius, p.wash(if hovered { 0x10 } else { 0x09 }));
        painter.rect_stroke(rect, radius, egui::Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        icons::draw(
            painter,
            egui::Rect::from_center_size(rect.left_center() + egui::vec2(20.0, 0.0), egui::Vec2::splat(16.0)),
            Icon::Search,
            p.text_faint,
        );
        painter.text(
            rect.left_center() + egui::vec2(38.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "Search or run a command",
            theme::body(12.5),
            if hovered { p.text_muted } else { p.text_faint },
        );
        widgets::paint_keys(painter, rect.right() - 8.0, rect.center().y, "Ctrl+Shift+P", p);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// What a click on a title-bar tab asked for.
struct TabHit {
    clicked: bool,
    close: bool,
}

/// One tab in the title bar. The active one is raised out of the bar on a
/// soft shadow, the way a selected segment is.
fn title_tab(
    ui: &mut egui::Ui,
    p: &Palette,
    icon: Icon,
    label: &str,
    active: bool,
    dirty: bool,
    closable: bool,
) -> TabHit {
    let font = if active { theme::strong(12.5) } else { theme::body(12.5) };
    let colour = if active { p.text } else { p.text_muted };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, colour);
    let text_width = galley.size().x;
    let mut width = 36.0 + text_width + 12.0;
    if dirty {
        width += 12.0;
    }
    if closable {
        width += 18.0;
    }
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 36.0), egui::Sense::click());
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 17.0, rect.center().y),
        egui::Vec2::splat(22.0),
    );
    // Registered after the tab, so it sits on top and takes its own clicks.
    let close = closable.then(|| ui.interact(close_rect, response.id.with("close"), egui::Sense::click()));
    let close_hovered = close.as_ref().is_some_and(|c| c.hovered());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = egui::CornerRadius::same(theme::RADIUS_CONTROL);
        if active {
            painter.add(
                egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 14,
                    spread: 0,
                    color: if p.is_dark() {
                        egui::Color32::from_black_alpha(0x59)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(0x2a, 0x1b, 0x50, 0x1a)
                    },
                }
                .as_shape(rect, radius),
            );
            painter.rect(rect, radius, p.popover, egui::Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
            theme::paint_sheen(painter, rect, theme::RADIUS_CONTROL as f32, p);
        } else if response.hovered() || close_hovered {
            painter.rect_filled(rect, radius, p.wash(0x0b));
        }
        icons::draw(
            painter,
            egui::Rect::from_center_size(rect.left_center() + egui::vec2(20.0, 0.0), egui::Vec2::splat(15.0)),
            icon,
            if active { p.accent_text } else { p.text_muted },
        );
        let text_left = rect.left() + 36.0;
        painter.galley(egui::pos2(text_left, rect.center().y - galley.size().y / 2.0), galley, colour);
        if dirty {
            painter.circle_filled(egui::pos2(text_left + text_width + 9.0, rect.center().y), 3.0, p.accent);
        }
        if let Some(c) = &close {
            if c.hovered() {
                painter.rect_filled(close_rect, egui::CornerRadius::same(8), p.wash(0x14));
            }
            if active || response.hovered() || c.hovered() {
                icons::draw(
                    painter,
                    egui::Rect::from_center_size(close_rect.center(), egui::Vec2::splat(12.0)),
                    Icon::Close,
                    if c.hovered() { p.text } else { p.text_faint },
                );
            }
        }
    }

    // Middle-click closes, as it does in every tabbed application.
    let close_clicked =
        close.as_ref().is_some_and(|c| c.clicked()) || (closable && response.middle_clicked());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    TabHit {
        clicked: response.clicked() && !close_clicked,
        close: close_clicked,
    }
}

/// The zoom readout in the capsule, which opens the zoom menu.
fn zoom_button(ui: &mut egui::Ui, p: &Palette, label: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_owned(), theme::strong(13.0), p.text);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 32.0, 32.0),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, egui::CornerRadius::same(11), p.wash(0x0d));
        }
        painter.galley(
            egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0),
            galley,
            p.text,
        );
        icons::draw(
            painter,
            egui::Rect::from_center_size(rect.right_center() - egui::vec2(12.0, 0.0), egui::Vec2::splat(11.0)),
            Icon::ChevronDown,
            p.text_faint,
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Zoom")
}

/// One floating message.
fn toast(ui: &mut egui::Ui, p: &Palette, text: &str, error: bool, ttl: f32) {
    // The last moment of a toast's life is a fade rather than a blink.
    ui.set_opacity((ttl / 0.4).clamp(0.0, 1.0));
    let colour = if error { p.error } else { p.success };
    egui::Frame::new()
        .fill(p.popover)
        .stroke(egui::Stroke::new(
            1.0,
            if error {
                egui::Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 0x40)
            } else {
                p.border
            },
        ))
        .corner_radius(egui::CornerRadius::same(13))
        .inner_margin(egui::Margin { left: 10, right: 14, top: 8, bottom: 8 })
        .shadow(theme::float_shadow(p))
        .show(ui, |ui| {
            ui.set_max_width(380.0);
            ui.horizontal(|ui| {
                let (plate, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                ui.painter().circle_filled(
                    plate.center(),
                    10.0,
                    egui::Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 0x2a),
                );
                icons::draw(
                    ui.painter(),
                    egui::Rect::from_center_size(plate.center(), egui::Vec2::splat(12.0)),
                    if error { Icon::Warning } else { Icon::Check },
                    colour,
                );
                ui.add(egui::Label::new(RichText::new(text).font(theme::strong(12.0)).color(p.text)).wrap());
            });
        });
}

/// The send button in a composer: a solid accent square once there is
/// something to send.
fn send_button(ui: &mut egui::Ui, p: &Palette, enabled: bool) -> egui::Response {
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(34.0), sense);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = egui::CornerRadius::same(12);
        if enabled {
            painter.add(
                egui::epaint::Shadow {
                    offset: [0, 6],
                    blur: 16,
                    spread: 0,
                    color: p.accent_alpha(0x40),
                }
                .as_shape(rect, radius),
            );
            let fill = if response.hovered() {
                theme::composite(egui::Color32::from_white_alpha(0x1a), p.accent)
            } else {
                p.accent
            };
            painter.rect_filled(rect, radius, fill);
        } else {
            painter.rect_filled(rect, radius, p.wash(0x0d));
        }
        icons::draw(
            painter,
            egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(16.0)),
            Icon::ArrowUp,
            if enabled { p.on_accent } else { p.text_disabled },
        );
    }
    if enabled {
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Send")
    } else {
        response
    }
}

/// A command label with the part the query matched set in bold.
fn highlighted(
    label: &str,
    query: &str,
    colour: egui::Color32,
    strong: egui::Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let format = |font: egui::FontId, color: egui::Color32| egui::TextFormat {
        font_id: font,
        color,
        ..Default::default()
    };
    // Byte offsets found in the lowercased label only line up with the
    // original for ASCII; anything else is shown whole rather than mis-cut.
    let at = if !query.is_empty() && label.is_ascii() {
        label.to_lowercase().find(query)
    } else {
        None
    };
    match at {
        Some(i) => {
            let end = i + query.len();
            job.append(&label[..i], 0.0, format(theme::body(13.0), colour));
            job.append(&label[i..end], 0.0, format(theme::heavy(13.0), strong));
            job.append(&label[end..], 0.0, format(theme::body(13.0), colour));
        }
        None => job.append(label, 0.0, format(theme::body(13.0), colour)),
    }
    job
}

/// A folder, with the home directory written as `~`.
fn tilde(dir: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = dir.strip_prefix(&home) {
            return if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    dir.display().to_string()
}

/// The shade just inside the top of the canvas well, which is what makes it
/// read as recessed rather than as a darker panel.
fn inset_shade(painter: &egui::Painter, well: egui::Rect, p: &Palette) {
    let depth = 14.0;
    let shade = if p.is_dark() {
        egui::Color32::from_black_alpha(0x66)
    } else {
        egui::Color32::from_rgba_unmultiplied(0x2a, 0x1b, 0x50, 0x14)
    };
    let mut mesh = egui::Mesh::default();
    let top = well.top();
    mesh.colored_vertex(egui::pos2(well.left(), top), shade);
    mesh.colored_vertex(egui::pos2(well.right(), top), shade);
    mesh.colored_vertex(egui::pos2(well.right(), top + depth), egui::Color32::TRANSPARENT);
    mesh.colored_vertex(egui::pos2(well.left(), top + depth), egui::Color32::TRANSPARENT);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod chrome_tests {
    use super::*;

    #[test]
    fn a_query_is_highlighted_where_it_matches() {
        let job = highlighted("Rotate Pages Clockwise", "pages", egui::Color32::GRAY, egui::Color32::WHITE);
        assert_eq!(job.sections.len(), 3);
        let range = &job.sections[1].byte_range;
        assert_eq!(&job.text[range.start.0..range.end.0], "Pages");
    }

    #[test]
    fn a_non_ascii_label_is_never_cut_mid_character() {
        let job = highlighted("Größe ändern", "ä", egui::Color32::GRAY, egui::Color32::WHITE);
        assert_eq!(job.sections.len(), 1);
    }

    #[test]
    fn the_home_directory_is_written_as_a_tilde() {
        if let Some(home) = dirs::home_dir() {
            assert_eq!(tilde(&home.join("Documents")), "~/Documents");
            assert_eq!(tilde(&home), "~");
        }
        assert_eq!(tilde(std::path::Path::new("/tmp")), "/tmp");
    }

    #[test]
    fn only_web_and_mail_addresses_are_opened() {
        assert_eq!(link_opens(" https://example.org/a?b=c "), Ok("https://example.org/a?b=c"));
        assert!(link_opens("HTTP://example.org").is_ok());
        assert!(link_opens("mailto:someone@example.org").is_ok());
        for refused in ["file:///etc/passwd", "javascript:alert(1)", "smb://host/share", "/Applications/Calculator.app", "https://a.org/x y", "https://a.org/\nx"] {
            assert!(link_opens(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn neighbouring_islands_share_one_gap() {
        assert_eq!(HALF_I * 2, GAP_I);
    }

    #[test]
    fn every_tool_in_the_dock_is_reachable_once() {
        let tools: Vec<Tool> = TOOL_GROUPS.iter().flat_map(|g| g.iter().map(|t| t.0)).collect();
        for (i, a) in tools.iter().enumerate() {
            assert!(!tools[i + 1..].contains(a), "{a:?} appears twice");
        }
    }
}
