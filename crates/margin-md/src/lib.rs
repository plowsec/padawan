//! Markdown → blocks → egui. Safe Rust, no dependency beyond egui and this workspace. Input is hostile.
//!
//! | Item | What |
//! |---|---|
//! | `parse` | text → `MdDoc`. Never panics, no recursion on input depth, linear time, capped output |
//! | `show` | draw an `MdDoc` in the space `ui` has left (scrolls, lays out visible blocks only) |
//!
//! Never fetched or opened: images, URLs. A clicked link is returned to the caller.
//!
//! | Module | What |
//! |---|---|
//! | `block` | line-based block parser (container stack) |
//! | `inline` | emphasis / links / code spans → spans |
//! | `build` | raw blocks → leaves (chunks, footnotes, gaps, hashes, slugs) |
//! | `doc` | document types, debug dump |
//! | `lay` | layout + paint of one leaf |
//! | `view` | scrolling, virtualization, input |
//!
//! PUBLIC API IS FROZEN (margin-gui compiles against it). Add items if needed; do not change these.

use std::sync::Arc;

use margin_theme::Theme;

mod block;
mod build;
mod doc;
mod inline;
mod lay;
mod text;
mod view;

#[cfg(test)]
mod tests;

pub use view::Hit;

/// A parsed document. `Send`: parsed on a worker thread for big files. Cheap to clone (shared).
#[derive(Clone, Default)]
pub struct MdDoc {
    inner: Arc<doc::Inner>,
}

impl std::fmt::Debug for MdDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MdDoc").field("id", &self.inner.id).field("blocks", &self.inner.leaves.len()).field("truncated", &self.inner.truncated).finish()
    }
}

impl MdDoc {
    /// Headings in order: `(level 1–6, plain text, source line 0-based)`.
    pub fn headings(&self) -> Vec<(u8, String, u32)> {
        self.inner.headings.clone()
    }

    /// Source line of the heading a `#fragment` link points to (GitHub-style slug).
    pub fn anchor_line(&self, fragment: &str) -> Option<u32> {
        let f = fragment.strip_prefix('#').unwrap_or(fragment);
        let f = text::percent_decode(f);
        let s = &self.inner.slugs;
        s.iter().find(|(x, _)| *x == f).or_else(|| s.iter().find(|(x, _)| *x == f.to_lowercase())).map(|&(_, l)| l)
    }

    /// Number of drawable blocks (long code blocks and tables count once per chunk).
    pub fn block_count(&self) -> usize {
        self.inner.leaves.len()
    }

    /// Input was cut (over `MAX_SOURCE`, or a block/container cap was hit).
    pub fn truncated(&self) -> bool {
        self.inner.truncated
    }

    /// Compact block-tree dump, one block per line (tests, debugging).
    pub fn dump(&self) -> String {
        self.inner.dump()
    }
}

/// Parse `text`. Inputs over `MAX_SOURCE` bytes are cut there (at a line start).
pub fn parse(text: &str) -> MdDoc {
    MdDoc { inner: Arc::new(build::build(text)) }
}

/// Source bytes parsed at most.
pub const MAX_SOURCE: usize = 8 << 20;

/// Scroll position and layout cache of one preview.
#[derive(Clone, Default)]
pub struct PreviewState {
    v: view::View,
}

impl std::fmt::Debug for PreviewState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.v.fmt(f)
    }
}

impl PreviewState {
    /// Scroll so that source line `line` (0-based) is at the top, on the next `show`.
    pub fn scroll_to_line(&mut self, line: u32) {
        self.v.pending = Some(line);
    }

    /// Clickable areas drawn in the last frame (screen rects). Tests, automation.
    pub fn hits(&self) -> &[(egui::Rect, Hit)] {
        &self.v.hits
    }
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
pub fn show(ui: &mut egui::Ui, doc: &MdDoc, state: &mut PreviewState, theme: &Theme) -> PreviewOutput {
    view::show(ui, &doc.inner, &mut state.v, theme)
}
