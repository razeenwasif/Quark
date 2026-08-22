//! Every user-invokable action, in one enum.
//!
//! Menus, the toolbar, keyboard shortcuts and the command palette all resolve
//! to a [`Command`], so a feature becomes reachable from all four the moment it
//! is added here. It also means the palette can list everything Quark can do
//! without a parallel table that drifts out of date.

use crate::layout::{PageMode, ZoomMode};
use crate::prefs::{PageTint, SidePanel};
use crate::tools::Tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    // --- file ---
    Open,
    OpenRecent(usize),
    Save,
    SaveAs,
    SaveACopy,
    Close,
    CloseAll,
    Revert,
    Print,
    Properties,
    NewFromBlank,
    NewFromImages,
    NewFromText,
    CombineFiles,
    Quit,

    // --- export ---
    ExportPagesAsImages,
    ExportText,
    ExtractPages,
    SplitDocument,
    Optimize,

    // --- edit ---
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
    Deselect,
    CopyAsImage,
    Preferences,

    // --- view ---
    ZoomIn,
    ZoomOut,
    SetZoom(ZoomMode),
    SetPageMode(PageMode),
    RotateViewCw,
    RotateViewCcw,
    SetTint(PageTint),
    TogglePanel(SidePanel),
    ToggleToolbar,
    ToggleStatusBar,
    FullScreen,
    ReadingMode,
    ToggleTheme,

    // --- navigation ---
    NextPage,
    PrevPage,
    FirstPage,
    LastPage,
    GoToPage(usize),
    GoBack,
    GoForward,
    ScrollUp,
    ScrollDown,

    // --- find ---
    Find,
    FindNext,
    FindPrev,
    AdvancedSearch,

    // --- tools ---
    SetTool(Tool),
    Comment,

    // --- pages ---
    InsertPagesFromFile,
    InsertBlankPage,
    DeletePages,
    RotatePagesCw,
    RotatePagesCcw,
    MovePages,
    CropPages,
    ResizePages,
    AddWatermark,
    RemoveWatermark,
    AddBackground,
    AddHeaderFooter,
    AddBatesNumbering,
    RemoveBatesNumbering,

    // --- forms ---
    PrepareForm,
    HighlightFields,
    FlattenForm,
    ExportFormData,
    ImportFormData,
    ClearForm,

    // --- protect ---
    EncryptWithPassword,
    RemoveSecurity,
    RestrictEditing,
    MarkForRedaction,
    ApplyRedactions,
    SanitizeDocument,
    RemoveHiddenInformation,

    // --- signatures ---
    SignDocument,
    CertifyDocument,
    ValidateSignatures,

    // --- accessibility & structure ---
    ShowTags,
    AutoTagDocument,
    ReadOutLoud,

    // --- comparison ---
    CompareDocuments,

    // --- ocr ---
    RecognizeText,

    // --- bookmarks ---
    AddBookmark,
    DeleteBookmark,
    RenameBookmark,

    // --- attachments ---
    AttachFile,
    ExtractAttachment,

    // --- window ---
    NextTab,
    PrevTab,
    CommandPalette,
    Help,
    About,
}

/// The menu a command is filed under, used to build the menu bar and to group
/// the command palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Menu {
    File,
    Edit,
    View,
    Navigate,
    Tools,
    Pages,
    Forms,
    Protect,
    Window,
    Help,
}

impl Menu {
    pub fn label(self) -> &'static str {
        match self {
            Menu::File => "File",
            Menu::Edit => "Edit",
            Menu::View => "View",
            Menu::Navigate => "Navigate",
            Menu::Tools => "Tools",
            Menu::Pages => "Pages",
            Menu::Forms => "Forms",
            Menu::Protect => "Protect",
            Menu::Window => "Window",
            Menu::Help => "Help",
        }
    }

    pub const ALL: [Menu; 10] = [
        Menu::File,
        Menu::Edit,
        Menu::View,
        Menu::Navigate,
        Menu::Tools,
        Menu::Pages,
        Menu::Forms,
        Menu::Protect,
        Menu::Window,
        Menu::Help,
    ];
}

/// A command's presentation: what to call it, where it lives, its shortcut.
#[derive(Debug, Clone)]
pub struct CommandInfo {
    pub command: Command,
    pub label: &'static str,
    pub menu: Menu,
    /// Accelerator in the conventional `Ctrl+Shift+K` spelling, or empty.
    pub shortcut: &'static str,
    /// Whether the command changes the document, so it can be greyed out on a
    /// read-only or unopened file.
    pub mutates: bool,
}

macro_rules! cmds {
    ($( $cmd:expr, $label:literal, $menu:ident, $short:literal, $mut:literal );* $(;)?) => {
        vec![$( CommandInfo {
            command: $cmd,
            label: $label,
            menu: Menu::$menu,
            shortcut: $short,
            mutates: $mut,
        } ),*]
    };
}

/// The full command table.
///
/// Built once and cached by the caller; it is a few hundred entries and
/// rebuilding it per frame would be wasteful.
pub fn command_table() -> Vec<CommandInfo> {
    use Command as C;
    cmds![
        // File
        C::Open, "Open…", File, "Ctrl+O", false;
        C::Save, "Save", File, "Ctrl+S", true;
        C::SaveAs, "Save As…", File, "Ctrl+Shift+S", true;
        C::SaveACopy, "Save a Copy…", File, "", false;
        C::Revert, "Revert to Saved", File, "", true;
        C::Close, "Close", File, "Ctrl+W", false;
        C::CloseAll, "Close All", File, "", false;
        C::Print, "Print…", File, "Ctrl+P", false;
        C::Properties, "Document Properties…", File, "Ctrl+D", false;
        C::NewFromBlank, "Create Blank PDF", File, "", false;
        C::NewFromImages, "Create PDF from Images…", File, "", false;
        C::NewFromText, "Create PDF from Text…", File, "", false;
        C::CombineFiles, "Combine Files into PDF…", File, "", false;
        C::ExportPagesAsImages, "Export Pages as Images…", File, "", false;
        C::ExportText, "Export Text…", File, "", false;
        C::ExtractPages, "Extract Pages…", File, "", false;
        C::SplitDocument, "Split Document…", File, "", false;
        C::Optimize, "Reduce File Size…", File, "", true;
        C::Quit, "Exit", File, "Alt+F4", false;

        // Edit
        C::Undo, "Undo", Edit, "Ctrl+Z", true;
        C::Redo, "Redo", Edit, "Ctrl+Y", true;
        C::Cut, "Cut", Edit, "Ctrl+X", true;
        C::Copy, "Copy", Edit, "Ctrl+C", false;
        C::Paste, "Paste", Edit, "Ctrl+V", true;
        C::Delete, "Delete", Edit, "Delete", true;
        C::SelectAll, "Select All", Edit, "Ctrl+A", false;
        C::Deselect, "Deselect All", Edit, "Ctrl+Shift+A", false;
        C::CopyAsImage, "Copy as Image", Edit, "", false;
        C::Find, "Find", Edit, "Ctrl+F", false;
        C::FindNext, "Find Next", Edit, "F3", false;
        C::FindPrev, "Find Previous", Edit, "Shift+F3", false;
        C::AdvancedSearch, "Advanced Search…", Edit, "Ctrl+Shift+F", false;
        C::Preferences, "Preferences…", Edit, "Ctrl+K", false;

        // View
        C::ZoomIn, "Zoom In", View, "Ctrl+=", false;
        C::ZoomOut, "Zoom Out", View, "Ctrl+-", false;
        C::SetZoom(ZoomMode::Actual), "Actual Size", View, "Ctrl+1", false;
        C::SetZoom(ZoomMode::FitPage), "Fit Page", View, "Ctrl+0", false;
        C::SetZoom(ZoomMode::FitWidth), "Fit Width", View, "Ctrl+2", false;
        C::SetZoom(ZoomMode::FitHeight), "Fit Height", View, "Ctrl+3", false;
        C::SetPageMode(PageMode::Single), "Single Page", View, "", false;
        C::SetPageMode(PageMode::Continuous), "Continuous", View, "", false;
        C::SetPageMode(PageMode::TwoUp), "Two Pages", View, "", false;
        C::SetPageMode(PageMode::TwoUpContinuous), "Two Pages Continuous", View, "", false;
        C::SetPageMode(PageMode::TwoUpCover), "Two Pages with Cover", View, "", false;
        C::RotateViewCw, "Rotate View Clockwise", View, "Ctrl+Shift+Plus", false;
        C::RotateViewCcw, "Rotate View Counterclockwise", View, "Ctrl+Shift+Minus", false;
        C::SetTint(PageTint::None), "Normal Colours", View, "", false;
        C::SetTint(PageTint::Night), "Night Mode", View, "Ctrl+Shift+N", false;
        C::SetTint(PageTint::Sepia), "Sepia", View, "", false;
        C::SetTint(PageTint::Dim), "Dim", View, "", false;
        C::TogglePanel(SidePanel::Thumbnails), "Page Thumbnails", View, "F4", false;
        C::TogglePanel(SidePanel::Bookmarks), "Bookmarks", View, "", false;
        C::TogglePanel(SidePanel::Comments), "Comments", View, "", false;
        C::TogglePanel(SidePanel::Attachments), "Attachments", View, "", false;
        C::TogglePanel(SidePanel::Layers), "Layers", View, "", false;
        C::TogglePanel(SidePanel::Signatures), "Signatures", View, "", false;
        C::TogglePanel(SidePanel::Fields), "Form Fields", View, "", false;
        C::ToggleToolbar, "Show Toolbar", View, "F8", false;
        C::ToggleStatusBar, "Show Status Bar", View, "", false;
        C::FullScreen, "Full Screen", View, "F11", false;
        C::ReadingMode, "Reading Mode", View, "Ctrl+H", false;
        C::ToggleTheme, "Toggle Light/Dark Theme", View, "", false;

        // Navigate
        C::NextPage, "Next Page", Navigate, "Page Down", false;
        C::PrevPage, "Previous Page", Navigate, "Page Up", false;
        C::FirstPage, "First Page", Navigate, "Ctrl+Home", false;
        C::LastPage, "Last Page", Navigate, "Ctrl+End", false;
        C::GoToPage(0), "Go to Page…", Navigate, "Ctrl+G", false;
        C::GoBack, "Previous View", Navigate, "Alt+Left", false;
        C::GoForward, "Next View", Navigate, "Alt+Right", false;

        // Tools
        C::SetTool(Tool::Select), "Select Tool", Tools, "V", false;
        C::SetTool(Tool::Pan), "Hand Tool", Tools, "H", false;
        C::SetTool(Tool::ZoomArea), "Marquee Zoom", Tools, "", false;
        C::SetTool(Tool::Snapshot), "Snapshot", Tools, "", false;
        C::SetTool(Tool::Highlight), "Highlight Text", Tools, "Ctrl+Shift+H", true;
        C::SetTool(Tool::Underline), "Underline Text", Tools, "Ctrl+Shift+U", true;
        C::SetTool(Tool::StrikeOut), "Strike Through Text", Tools, "Ctrl+Shift+K", true;
        C::SetTool(Tool::Squiggly), "Squiggly Underline", Tools, "", true;
        C::SetTool(Tool::Note), "Add Sticky Note", Tools, "Ctrl+Shift+M", true;
        C::SetTool(Tool::FreeText), "Add Text Box", Tools, "", true;
        C::SetTool(Tool::Callout), "Add Callout", Tools, "", true;
        C::SetTool(Tool::Ink), "Draw Freehand", Tools, "Ctrl+Shift+D", true;
        C::SetTool(Tool::Eraser), "Eraser", Tools, "", true;
        C::SetTool(Tool::Line), "Draw Line", Tools, "", true;
        C::SetTool(Tool::Arrow), "Draw Arrow", Tools, "", true;
        C::SetTool(Tool::Rectangle), "Draw Rectangle", Tools, "", true;
        C::SetTool(Tool::Ellipse), "Draw Ellipse", Tools, "", true;
        C::SetTool(Tool::Polygon), "Draw Polygon", Tools, "", true;
        C::SetTool(Tool::Polyline), "Draw Polyline", Tools, "", true;
        C::SetTool(Tool::Stamp), "Add Stamp", Tools, "", true;
        C::SetTool(Tool::Signature), "Fill & Sign", Tools, "", true;
        C::SetTool(Tool::MeasureDistance), "Measure Distance", Tools, "", true;
        C::SetTool(Tool::MeasurePerimeter), "Measure Perimeter", Tools, "", true;
        C::SetTool(Tool::MeasureArea), "Measure Area", Tools, "", true;
        C::SetTool(Tool::ContentEdit), "Edit Text & Images", Tools, "", true;
        C::SetTool(Tool::Link), "Add Link", Tools, "", true;
        C::Comment, "Comment", Tools, "", true;
        C::RecognizeText, "Recognise Text (OCR)…", Tools, "", true;
        C::CompareDocuments, "Compare Documents…", Tools, "", false;
        C::ReadOutLoud, "Read Out Loud", Tools, "", false;
        C::ShowTags, "Accessibility Tags", Tools, "", false;
        C::AutoTagDocument, "Auto-Tag Document", Tools, "", true;

        // Pages
        C::InsertPagesFromFile, "Insert Pages from File…", Pages, "Ctrl+Shift+I", true;
        C::InsertBlankPage, "Insert Blank Page", Pages, "", true;
        C::DeletePages, "Delete Pages…", Pages, "Ctrl+Shift+Delete", true;
        C::RotatePagesCw, "Rotate Pages Clockwise", Pages, "", true;
        C::RotatePagesCcw, "Rotate Pages Counterclockwise", Pages, "", true;
        C::MovePages, "Move Pages…", Pages, "", true;
        C::CropPages, "Crop Pages…", Pages, "", true;
        C::ResizePages, "Resize Pages…", Pages, "", true;
        C::AddWatermark, "Add Watermark…", Pages, "", true;
        C::RemoveWatermark, "Remove Watermark", Pages, "", true;
        C::AddBackground, "Add Background…", Pages, "", true;
        C::AddHeaderFooter, "Add Header & Footer…", Pages, "", true;
        C::AddBatesNumbering, "Add Bates Numbering…", Pages, "", true;
        C::RemoveBatesNumbering, "Remove Bates Numbering", Pages, "", true;
        C::AddBookmark, "Add Bookmark", Pages, "Ctrl+B", true;
        C::AttachFile, "Attach a File…", Pages, "", true;

        // Forms
        C::PrepareForm, "Prepare Form", Forms, "", true;
        C::HighlightFields, "Highlight Existing Fields", Forms, "", false;
        C::FlattenForm, "Flatten Form Fields", Forms, "", true;
        C::ExportFormData, "Export Form Data…", Forms, "", false;
        C::ImportFormData, "Import Form Data…", Forms, "", true;
        C::ClearForm, "Clear Form", Forms, "", true;

        // Protect
        C::EncryptWithPassword, "Encrypt with Password…", Protect, "", true;
        C::RemoveSecurity, "Remove Security", Protect, "", true;
        C::RestrictEditing, "Restrict Editing…", Protect, "", true;
        C::MarkForRedaction, "Mark for Redaction", Protect, "", true;
        C::ApplyRedactions, "Apply Redactions", Protect, "", true;
        C::SanitizeDocument, "Sanitise Document", Protect, "", true;
        C::RemoveHiddenInformation, "Remove Hidden Information…", Protect, "", true;
        C::SignDocument, "Sign Document…", Protect, "", true;
        C::CertifyDocument, "Certify Document…", Protect, "", true;
        C::ValidateSignatures, "Validate All Signatures", Protect, "", false;

        // Window
        C::NextTab, "Next Tab", Window, "Ctrl+Tab", false;
        C::PrevTab, "Previous Tab", Window, "Ctrl+Shift+Tab", false;
        C::CommandPalette, "Command Palette", Window, "Ctrl+Shift+P", false;

        // Help
        C::Help, "Quark Help", Help, "F1", false;
        C::About, "About Quark", Help, "", false;
    ]
}

/// Splits an accelerator such as `Ctrl+Shift+K` into modifiers and a key name.
///
/// Returns `(ctrl, shift, alt, key)`. The key is returned as written; matching
/// it to a physical key is the UI layer's job.
pub fn parse_shortcut(s: &str) -> Option<(bool, bool, bool, String)> {
    if s.is_empty() {
        return None;
    }
    let mut ctrl = false;
    let mut shift = false;
    let mut alt = false;
    let mut key = None;
    for part in s.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "alt" => alt = true,
            "" => {
                // A trailing '+' means the key itself is '+', as in "Ctrl++".
                key = Some("+".to_owned());
            }
            _ => key = Some(part.trim().to_owned()),
        }
    }
    key.map(|k| (ctrl, shift, alt, k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn the_table_is_not_empty_and_every_entry_is_labelled() {
        let t = command_table();
        assert!(t.len() > 100, "expected a full command set, got {}", t.len());
        for c in &t {
            assert!(!c.label.is_empty(), "{:?} has no label", c.command);
        }
    }

    #[test]
    fn no_command_appears_twice() {
        // A duplicate would make the palette show the same action twice and
        // make shortcut dispatch ambiguous.
        let t = command_table();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for c in &t {
            *seen.entry(format!("{:?}", c.command)).or_insert(0) += 1;
        }
        let dupes: Vec<_> = seen.iter().filter(|&(_, &n)| n > 1).collect();
        assert!(dupes.is_empty(), "duplicate commands: {dupes:?}");
    }

    #[test]
    fn no_shortcut_is_bound_to_two_commands() {
        let t = command_table();
        let mut seen: HashMap<&str, Vec<&str>> = HashMap::new();
        for c in &t {
            if !c.shortcut.is_empty() {
                seen.entry(c.shortcut).or_default().push(c.label);
            }
        }
        let clashes: Vec<_> = seen.iter().filter(|(_, v)| v.len() > 1).collect();
        assert!(clashes.is_empty(), "shortcut clashes: {clashes:?}");
    }

    #[test]
    fn every_menu_has_at_least_one_command() {
        let t = command_table();
        for m in Menu::ALL {
            assert!(
                t.iter().any(|c| c.menu == m),
                "{} menu would render empty",
                m.label()
            );
        }
    }

    #[test]
    fn shortcuts_parse_into_modifiers_and_a_key() {
        assert_eq!(
            parse_shortcut("Ctrl+Shift+K"),
            Some((true, true, false, "K".into()))
        );
        assert_eq!(parse_shortcut("F3"), Some((false, false, false, "F3".into())));
        assert_eq!(
            parse_shortcut("Alt+Left"),
            Some((false, false, true, "Left".into()))
        );
        assert_eq!(parse_shortcut(""), None);
    }

    #[test]
    fn a_trailing_plus_is_read_as_the_plus_key() {
        // "Ctrl++" is a real accelerator and naive splitting loses the key.
        assert_eq!(parse_shortcut("Ctrl++"), Some((true, false, false, "+".into())));
    }

    #[test]
    fn every_shortcut_in_the_table_parses() {
        for c in command_table() {
            if !c.shortcut.is_empty() {
                assert!(
                    parse_shortcut(c.shortcut).is_some(),
                    "unparsable shortcut {:?} on {}",
                    c.shortcut,
                    c.label
                );
            }
        }
    }

    #[test]
    fn read_only_commands_are_not_marked_as_mutating() {
        let t = command_table();
        for label in ["Open…", "Copy", "Find", "Zoom In", "Next Page"] {
            let c = t.iter().find(|c| c.label == label).unwrap();
            assert!(!c.mutates, "{label} should not be a mutating command");
        }
        for label in ["Undo", "Flatten Form Fields", "Apply Redactions"] {
            let c = t.iter().find(|c| c.label == label).unwrap();
            assert!(c.mutates, "{label} should be a mutating command");
        }
    }
}
