//! User preferences, persisted as TOML next to the rest of Quark's state.

use crate::layout::{PageMode, ZoomMode};
use crate::tools::ToolStyle;
use serde::{Deserialize, Serialize};

/// How pages are tinted for reading in the dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PageTint {
    /// Render the page exactly as authored.
    #[default]
    None,
    /// Invert luminance but keep hue, so photographs stay recognisable.
    Night,
    /// Warm off-white, easier under low light than pure white.
    Sepia,
    /// Reduce the page's white point without inverting.
    Dim,
}

impl PageTint {
    pub fn label(self) -> &'static str {
        match self {
            PageTint::None => "Normal",
            PageTint::Night => "Night",
            PageTint::Sepia => "Sepia",
            PageTint::Dim => "Dim",
        }
    }

    pub const ALL: [PageTint; 4] = [
        PageTint::None,
        PageTint::Night,
        PageTint::Sepia,
        PageTint::Dim,
    ];
}

/// Which side panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SidePanel {
    #[default]
    None,
    Thumbnails,
    Bookmarks,
    Comments,
    Attachments,
    Layers,
    Signatures,
    Search,
    Fields,
}

impl SidePanel {
    pub fn label(self) -> &'static str {
        match self {
            SidePanel::None => "None",
            SidePanel::Thumbnails => "Page Thumbnails",
            SidePanel::Bookmarks => "Bookmarks",
            SidePanel::Comments => "Comments",
            SidePanel::Attachments => "Attachments",
            SidePanel::Layers => "Layers",
            SidePanel::Signatures => "Signatures",
            SidePanel::Search => "Search",
            SidePanel::Fields => "Form Fields",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub page_mode: PageMode,
    pub zoom: ZoomMode,
    pub tint: PageTint,
    pub side_panel: SidePanel,
    pub side_panel_width: f32,
    /// Width of the right-hand dock that holds comments and the assistant.
    pub dock_width: f32,
    /// Which assistant backend to use. The key that goes with it lives in the
    /// OS credential store, never here — this file is plaintext.
    pub ai_backend: String,
    pub ai_model: String,
    /// How much of the document to send. See `quark_ai::Scope`.
    pub ai_scope: String,
    /// Base URL for the OpenAI-compatible backend, so one client can serve
    /// OpenAI, Groq, OpenRouter and the rest.
    pub ai_base_url: String,
    pub show_toolbar: bool,
    pub show_status_bar: bool,
    pub tool_style: ToolStyle,
    /// Reopen the documents that were open at exit.
    pub restore_session: bool,
    /// Return to the page the reader left off on.
    pub remember_position: bool,
    /// Files in the Recent list.
    pub recent_limit: usize,
    pub recent: Vec<RecentFile>,
    /// Render pages at this multiple of the display scale, for sharper text
    /// when zoomed. Above 2.0 the memory cost stops paying for itself.
    pub render_supersample: f32,
    /// Pages kept rasterised outside the viewport, in each direction.
    pub prerender_pages: usize,
    /// Rasterised page cache budget, in megabytes.
    pub raster_cache_mb: usize,
    pub smooth_scroll: bool,
    /// Ask before applying an irreversible operation such as redaction.
    pub confirm_destructive: bool,
    pub default_save_optimized: bool,
    pub theme_dark: bool,
    pub ui_scale: f32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            page_mode: PageMode::Continuous,
            zoom: ZoomMode::FitWidth,
            tint: PageTint::None,
            side_panel: SidePanel::Thumbnails,
            side_panel_width: 240.0,
            dock_width: 266.0,
            // Local by default: the first question a user asks must not
            // silently ship their document to a third party.
            ai_backend: "ollama".into(),
            ai_model: "llama3.2".into(),
            ai_scope: "nearby".into(),
            ai_base_url: "https://api.openai.com/v1".into(),
            show_toolbar: true,
            show_status_bar: true,
            tool_style: ToolStyle::default(),
            restore_session: true,
            remember_position: true,
            recent_limit: 20,
            recent: Vec::new(),
            render_supersample: 1.0,
            prerender_pages: 2,
            raster_cache_mb: 384,
            smooth_scroll: true,
            confirm_destructive: true,
            default_save_optimized: false,
            theme_dark: true,
            ui_scale: 1.0,
        }
    }
}

/// An entry in the Recent Files list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentFile {
    pub path: String,
    /// Page the reader was on, for `remember_position`.
    pub page: usize,
    /// RFC 3339 timestamp of the last open.
    pub opened: String,
}

impl Prefs {
    /// Records a file as most-recently-used.
    ///
    /// Re-opening a file moves it to the top rather than duplicating it, and
    /// the list is trimmed to `recent_limit`.
    pub fn push_recent(&mut self, path: &str, page: usize) {
        self.recent.retain(|r| r.path != path);
        self.recent.insert(0, RecentFile {
            path: path.to_owned(),
            page,
            opened: crate::annot::now_rfc3339(),
        });
        self.recent.truncate(self.recent_limit.max(1));
    }

    /// The remembered page for a path, if position memory is on.
    pub fn remembered_page(&self, path: &str) -> Option<usize> {
        if !self.remember_position {
            return None;
        }
        self.recent.iter().find(|r| r.path == path).map(|r| r.page)
    }

    pub fn clear_recent(&mut self) {
        self.recent.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_move_to_the_top_without_duplicating() {
        let mut p = Prefs::default();
        p.push_recent("/a.pdf", 0);
        p.push_recent("/b.pdf", 0);
        p.push_recent("/a.pdf", 12);
        assert_eq!(p.recent.len(), 2);
        assert_eq!(p.recent[0].path, "/a.pdf");
        assert_eq!(p.recent[0].page, 12);
    }

    #[test]
    fn recent_list_is_trimmed_to_the_limit() {
        let mut p = Prefs {
            recent_limit: 3,
            ..Default::default()
        };
        for i in 0..10 {
            p.push_recent(&format!("/f{i}.pdf"), 0);
        }
        assert_eq!(p.recent.len(), 3);
        assert_eq!(p.recent[0].path, "/f9.pdf");
    }

    #[test]
    fn remembered_page_respects_the_setting() {
        let mut p = Prefs::default();
        p.push_recent("/a.pdf", 7);
        assert_eq!(p.remembered_page("/a.pdf"), Some(7));
        assert_eq!(p.remembered_page("/missing.pdf"), None);
        p.remember_position = false;
        assert_eq!(p.remembered_page("/a.pdf"), None);
    }

    #[test]
    fn prefs_round_trip_through_toml() {
        // Serialisation is what makes settings survive a restart; a field that
        // fails to round-trip silently resets every launch.
        let mut p = Prefs::default();
        p.page_mode = PageMode::TwoUpCoverContinuous;
        p.zoom = ZoomMode::Custom(1.75);
        p.tint = PageTint::Night;
        p.push_recent("/x.pdf", 3);

        let s = toml_like(&p);
        let back: Prefs = serde_json::from_str(&s).expect("round trip");
        assert_eq!(back.page_mode, PageMode::TwoUpCoverContinuous);
        assert_eq!(back.zoom, ZoomMode::Custom(1.75));
        assert_eq!(back.tint, PageTint::Night);
        assert_eq!(back.recent[0].path, "/x.pdf");
    }

    // JSON stands in for TOML here so quark-core keeps a single serde backend
    // in its dev-dependencies; both go through the same Serialize impls.
    fn toml_like(p: &Prefs) -> String {
        serde_json::to_string(p).unwrap()
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // Upgrading Quark must not wipe a user's settings file just because it
        // gained a field.
        let partial = r#"{"theme_dark": false}"#;
        let p: Prefs = serde_json::from_str(partial).expect("partial parse");
        assert!(!p.theme_dark);
        assert_eq!(p.page_mode, PageMode::Continuous);
        assert_eq!(p.recent_limit, 20);
    }
}
