//! The text editor widget: buffer, cursor, selection, undo, syntax colours, hint lines, find bar.
//! Draws only the visible lines. Columns are byte offsets in the line's UTF-8, lines are 0-based.
//!
//! STUB. PUBLIC API IS FROZEN (the shell compiles against it). Add items if needed; do not change these.
//!
//! ```ignore
//! let mut doc = Document::new("fn main() {}\n", Lang::Rust);
//! let mut st = EditorState::default();
//! let out = margin_editor::show(ui, &mut doc, &mut st, &EditorOptions::default(), &theme, &lenses);
//! if out.changed { /* buffer edited this frame */ }
//! ```

use margin_code::Lang;
use margin_theme::Theme;

/// A text buffer with undo history and cached lexer states. One per open file.
pub struct Document {
    text: String,
    lang: Lang,
}

/// `(line, col)`: 0-based line, byte column on a char boundary.
pub type Pos = (u32, u32);

impl Document {
    /// `text` may use `\n` or `\r\n` (remembered, written back by `text()`); a UTF-8 BOM is kept the same way.
    pub fn new(text: &str, lang: Lang) -> Self {
        Self { text: text.to_owned(), lang }
    }

    /// Whole buffer with its original line endings.
    pub fn text(&self) -> String {
        self.text.clone()
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    pub fn set_lang(&mut self, lang: Lang) {
        self.lang = lang;
    }

    /// Number of lines (an empty buffer has 1).
    pub fn line_count(&self) -> u32 {
        1
    }

    /// Line `i` without its line break; `""` past the end.
    pub fn line(&self, i: u32) -> &str {
        let _ = i;
        ""
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }

    /// Changes on every edit (undo and redo included). For caches keyed on the content.
    pub fn revision(&self) -> u64 {
        0
    }

    /// Edited since `mark_saved` (undo back to the saved state clears it).
    pub fn is_dirty(&self) -> bool {
        false
    }

    pub fn mark_saved(&mut self) {}

    /// Read-only buffers ignore every edit (typing, paste, undo).
    pub fn read_only(&self) -> bool {
        false
    }

    pub fn set_read_only(&mut self, on: bool) {
        let _ = on;
    }

    /// Replace the whole content (file changed on disk). Clears the undo history, keeps nothing dirty.
    pub fn reload(&mut self, text: &str) {
        self.text = text.to_owned();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LensTone {
    /// Counts: callers, callees.
    #[default]
    Normal,
    /// Sinks.
    Sink,
    /// Not clickable, dimmer (e.g. "indexing…").
    Faint,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LensSeg {
    pub text: String,
    pub tone: LensTone,
    /// Returned in `EditorOutput::lens_clicked`. Segments with `tone == Faint` are not clickable.
    pub id: u32,
    /// Tooltip; empty = none.
    pub hover: String,
}

/// A hint line drawn above `line` (an extra row, not part of the text): `3 callers · 5 callees · sinks: memcpy`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lens {
    pub line: u32,
    pub segs: Vec<LensSeg>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wrap {
    /// Wrap prose (`Lang::is_prose`), not code.
    Auto,
    On,
    Off,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditorOptions {
    pub tab_width: u32,
    /// Tab key inserts spaces.
    pub soft_tabs: bool,
    pub wrap: Wrap,
    pub line_numbers: bool,
    pub highlight_current_line: bool,
    /// Draw spaces/tabs as faint dots/arrows.
    pub show_whitespace: bool,
    /// Draw the hint lines passed to `show`.
    pub show_lenses: bool,
    /// Give the editor keyboard focus this frame (after opening a file, closing an overlay).
    pub request_focus: bool,
}

impl Default for EditorOptions {
    fn default() -> Self {
        Self {
            tab_width: 4,
            soft_tabs: true,
            wrap: Wrap::Auto,
            line_numbers: true,
            highlight_current_line: true,
            show_whitespace: false,
            show_lenses: true,
            request_focus: false,
        }
    }
}

/// Per-view state: cursor, selection, scroll position, find bar. One per open tab.
#[derive(Default)]
pub struct EditorState {
    _private: (),
}

impl EditorState {
    /// Cursor position.
    pub fn cursor(&self) -> Pos {
        (0, 0)
    }

    /// Selection as `(start, end)` in document order; `None` when empty.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        None
    }

    pub fn selected_text(&self, doc: &Document) -> String {
        let _ = doc;
        String::new()
    }

    /// Move the cursor there, clear the selection, scroll it into view (centred if it was off screen).
    /// Out-of-range values are clamped.
    pub fn goto(&mut self, line: u32, col: u32) {
        let _ = (line, col);
    }

    /// Select `len` bytes at `(line, col)` and scroll there (search hit, definition).
    pub fn select(&mut self, line: u32, col: u32, len: u32) {
        let _ = (line, col, len);
    }

    /// Open the find bar; `seed`: initial query (the shell passes the selection). `replace`: with the replace row.
    pub fn open_find(&mut self, seed: Option<&str>, replace: bool) {
        let _ = (seed, replace);
    }

    pub fn close_find(&mut self) {}

    pub fn find_open(&self) -> bool {
        false
    }

    pub fn find_next(&mut self, doc: &Document) {
        let _ = doc;
    }

    pub fn find_prev(&mut self, doc: &Document) {
        let _ = doc;
    }

    pub fn undo(&mut self, doc: &mut Document) {
        let _ = doc;
    }

    pub fn redo(&mut self, doc: &mut Document) {
        let _ = doc;
    }

    /// Comment / uncomment the selected lines with the language's line comment. No-op without one.
    pub fn toggle_comment(&mut self, doc: &mut Document) {
        let _ = doc;
    }

    /// Select the whole buffer.
    pub fn select_all(&mut self, doc: &Document) {
        let _ = doc;
    }

    /// First visible line (to restore the view later with `set_top_line`).
    pub fn top_line(&self) -> u32 {
        0
    }

    pub fn set_top_line(&mut self, line: u32) {
        let _ = line;
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorOutput {
    /// The buffer was edited this frame.
    pub changed: bool,
    /// The editor has keyboard focus.
    pub focused: bool,
    /// A hint segment was clicked: `(Lens::line, LensSeg::id)`.
    pub lens_clicked: Option<(u32, u32)>,
    /// Cmd/Ctrl+click or F12 on an identifier: the word and where it is.
    pub goto_definition: Option<(String, Pos)>,
    /// Identifier under the cursor (empty when none): the shell uses it for "find callers of…".
    pub word_at_cursor: String,
}

/// Draw the editor in all the space `ui` has left and handle its input.
/// The widget owns: typing, IME, clipboard, selection (mouse + keyboard, word/line by double/triple click),
/// undo/redo (cmd/ctrl+z, shift+z / y), indent/outdent, find bar (when open), scrolling.
/// It does not own: save, open, tabs, any shortcut not listed above (left in the input for the shell).
pub fn show(ui: &mut egui::Ui, doc: &mut Document, state: &mut EditorState, opts: &EditorOptions, theme: &Theme, lenses: &[Lens]) -> EditorOutput {
    let _ = (doc, state, opts, lenses);
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, theme.code.bg);
    ui.allocate_rect(rect, egui::Sense::click());
    EditorOutput::default()
}
