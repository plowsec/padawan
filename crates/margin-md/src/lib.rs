//! Markdown → blocks → egui. Safe Rust, no dependency beyond egui and this workspace. Input is hostile.
//!
//! | Item | What |
//! |---|---|
//! | `parse` | text → `MdDoc`. Never panics, no recursion on input depth, linear time, capped output |
//! | `show` | draw an `MdDoc` in the space `ui` has left (scrolls, lays out visible blocks only) |
//!
//! Never fetched or opened: images, URLs. A clicked link is returned to the caller.
//!
//! PUBLIC API IS FROZEN (margin-gui compiles against it). Add items if needed; do not change these.

use margin_code as _;
use margin_theme::Theme;

/// A parsed document. `Send`: parsed on a worker thread for big files.
#[derive(Clone, Debug, Default)]
pub struct MdDoc {
    _private: (),
}

impl MdDoc {
    /// Headings in order: `(level 1–6, plain text, source line 0-based)`.
    pub fn headings(&self) -> Vec<(u8, String, u32)> {
        Vec::new()
    }

    /// Source line of the heading a `#fragment` link points to (GitHub-style slug).
    pub fn anchor_line(&self, _fragment: &str) -> Option<u32> {
        None
    }
}

/// Parse `text`. Inputs over `MAX_SOURCE` bytes are cut there (at a line start).
pub fn parse(_text: &str) -> MdDoc {
    MdDoc::default()
}

/// Source bytes parsed at most.
pub const MAX_SOURCE: usize = 8 << 20;

/// Scroll position and layout cache of one preview.
#[derive(Clone, Debug, Default)]
pub struct PreviewState {
    _private: (),
}

impl PreviewState {
    /// Scroll so that source line `line` (0-based) is at the top, on the next `show`.
    pub fn scroll_to_line(&mut self, _line: u32) {}
}

#[derive(Clone, Debug, Default)]
pub struct PreviewOutput {
    /// A link was clicked: its destination, exactly as written in the source (hostile text).
    pub link_clicked: Option<String>,
    /// A task-list checkbox was clicked: source line (0-based) of the item.
    pub task_toggled: Option<u32>,
    /// Source line (0-based) of the first visible block.
    pub top_line: u32,
}

/// Draw the preview in all the space `ui` has left.
pub fn show(_ui: &mut egui::Ui, _doc: &MdDoc, _state: &mut PreviewState, _theme: &Theme) -> PreviewOutput {
    PreviewOutput::default()
}
