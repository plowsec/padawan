//! The text editor widget: buffer, cursor, selection, undo, syntax colours, hint lines, find bar.
//! Draws only the visible lines. Columns are byte offsets in the line's UTF-8, lines are 0-based.
//!
//! PUBLIC API IS FROZEN (the shell compiles against it). Add items if needed; do not change these.
//!
//! ```ignore
//! let mut doc = Document::new("fn main() {}\n", Lang::Rust);
//! let mut st = EditorState::default();
//! let out = margin_editor::show(ui, &mut doc, &mut st, &EditorOptions::default(), &theme, &lenses);
//! if out.changed { /* buffer edited this frame */ }
//! ```
//!
//! | Module | What |
//! |---|---|
//! | `buffer` | lines, line endings, BOM, the edit primitive |
//! | `undo` · `doc` | edit groups, undo/redo, dirty tracking · `Document` internals |
//! | `lexcache` | lexer state per line start, lazy + budgeted |
//! | `text` · `display` | char classes, hazards · display string + column mapping |
//! | `layout` · `lens` | galleys, wrap rows · hint rows |
//! | `view` · `input` · `paint` | scroll/frame · keys, mouse, actions · drawing |
//! | `ops` · `indent` · `menu` | editing commands · auto-indent · context menu |
//! | `find` · `findbar` | search · find bar |

mod buffer;
mod display;
mod doc;
mod find;
mod findbar;
#[cfg(test)]
mod fuzz;
mod indent;
mod input;
mod layout;
mod lens;
mod lexcache;
mod menu;
mod ops;
mod paint;
#[cfg(test)]
mod testrng;
mod text;
mod undo;
mod view;

pub use buffer::Edit;
use margin_code::Lang;
use margin_theme::Theme;

/// A text buffer with undo history and cached lexer states. One per open file.
pub struct Document {
    buf: buffer::Buffer,
    lang: Lang,
    hist: undo::History,
    lex: lexcache::LexCache,
    revision: u64,
    saved_id: u64,
    /// Exact loaded text when it cannot be rebuilt from the lines (mixed line endings).
    pristine: Option<String>,
    pristine_id: u64,
    read_only: bool,
}

/// `(line, col)`: 0-based line, byte column on a char boundary.
pub type Pos = (u32, u32);

impl Document {
    /// `text` may use `\n` or `\r\n` (remembered, written back by `text()`); a UTF-8 BOM is kept the same way.
    pub fn new(text: &str, lang: Lang) -> Self {
        Document::from_buffer(buffer::Buffer::new(text), lang)
    }

    /// Whole buffer with its original line endings.
    pub fn text(&self) -> String {
        match &self.pristine {
            Some(p) if self.at_pristine() => p.clone(),
            _ => self.buf.text(),
        }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    pub fn set_lang(&mut self, lang: Lang) {
        if lang != self.lang {
            self.lang = lang;
            self.lex.reset();
            self.revision += 1;
        }
    }

    /// Number of lines (an empty buffer has 1).
    pub fn line_count(&self) -> u32 {
        self.buf.line_count()
    }

    /// Line `i` without its line break; `""` past the end.
    pub fn line(&self, i: u32) -> &str {
        self.buf.line(i)
    }

    pub fn len_bytes(&self) -> usize {
        match &self.pristine {
            Some(p) if self.at_pristine() => p.len(),
            _ => self.buf.len_bytes(),
        }
    }

    /// Changes on every edit (undo and redo included). For caches keyed on the content.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Edited since `mark_saved` (undo back to the saved state clears it).
    pub fn is_dirty(&self) -> bool {
        self.hist.state_id() != self.saved_id
    }

    pub fn mark_saved(&mut self) {
        self.saved_id = self.hist.state_id();
    }

    /// Read-only buffers ignore every edit (typing, paste, undo).
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn set_read_only(&mut self, on: bool) {
        self.read_only = on;
    }

    /// Replace the whole content (file changed on disk). Clears the undo history, keeps nothing dirty.
    pub fn reload(&mut self, text: &str) {
        let mut buf = buffer::Buffer::new(text);
        self.pristine = buf.original.take();
        self.buf = buf;
        self.hist.clear();
        self.saved_id = self.hist.state_id();
        self.pristine_id = self.saved_id;
        self.lex.reset();
        self.revision += 1;
    }

    // ---- Added (not in the frozen stub) ----

    /// Replace `start..end` (clamped, either order) with `text` as one undo step; returns the inverse edit.
    /// `None` when read-only. `\n` separates lines.
    pub fn replace(&mut self, start: Pos, end: Pos, text: &str) -> Option<Edit> {
        let (a, b) = buffer::order(self.clamp(start), self.clamp(end));
        let removed = self.buf.slice(a, b);
        let mut fin = a;
        self.group(undo::Kind::Other, f64::NEG_INFINITY, undo::Sel::caret(a), |tx| {
            fin = tx.edit(a, b, text);
            undo::Sel::caret(fin)
        });
        (!self.read_only).then_some(Edit { start: a, end: fin, text: removed })
    }

    pub fn can_undo(&self) -> bool {
        !self.read_only && self.hist.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        !self.read_only && self.hist.can_redo()
    }

    /// Lines are written back with `\r\n` (dominant ending of the loaded text).
    pub fn crlf(&self) -> bool {
        self.buf.crlf
    }

    /// The loaded text started with a UTF-8 BOM (written back by `text()`).
    pub fn has_bom(&self) -> bool {
        self.buf.bom
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
    /// Enter keeps the indentation of the line, and adds a level after an opening bracket or a block opener
    /// (`:` in Python, `then`/`do` in Lua/Ruby/Shell…); a closing bracket typed on a blank line dedents.
    /// Markdown: Enter continues a list item or a quote.
    pub auto_indent: bool,
    /// Right-click menu (Cut, Copy, Paste, Select All, Go to Definition, Find Callers…).
    pub context_menu: bool,
}

/// An edit command from outside the widget (menu bar, palette). Queue with `EditorState::run`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    ToggleComment,
    Indent,
    Outdent,
    DuplicateLine,
    DeleteLine,
    MoveLineUp,
    MoveLineDown,
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
            auto_indent: true,
            context_menu: true,
        }
    }
}

/// Per-view state: cursor, selection, scroll position, find bar. One per open tab.
#[derive(Default)]
pub struct EditorState {
    _private: (),
    v: view::View,
    /// Actions queued by `run`, applied by the next `show`.
    queued: Vec<EditAction>,
}

impl EditorState {
    /// Queue an edit command; the next `show` applies it (and takes keyboard focus).
    pub fn run(&mut self, a: EditAction) {
        if self.queued.len() < 64 {
            self.queued.push(a);
        }
    }

    /// Cursor position.
    pub fn cursor(&self) -> Pos {
        self.v.sel.head
    }

    /// Selection as `(start, end)` in document order; `None` when empty.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let (a, b) = buffer::order(self.v.sel.anchor, self.v.sel.head);
        (a != b).then_some((a, b))
    }

    pub fn selected_text(&self, doc: &Document) -> String {
        let (a, b) = buffer::order(self.v.sel.anchor, self.v.sel.head);
        doc.buf.slice(a, b)
    }

    /// Move the cursor there, clear the selection, scroll it into view (centred if it was off screen).
    /// Out-of-range values are clamped.
    pub fn goto(&mut self, line: u32, col: u32) {
        self.v.set_sel(undo::Sel::caret((line, col)));
        self.v.reveal = view::Reveal::Center;
    }

    /// Select `len` bytes at `(line, col)` and scroll there (search hit, definition).
    pub fn select(&mut self, line: u32, col: u32, len: u32) {
        self.v.set_sel(undo::Sel { anchor: (line, col), head: (line, col.saturating_add(len)) });
        self.v.reveal = view::Reveal::Center;
    }

    /// Open the find bar; `seed`: initial query (the shell passes the selection). `replace`: with the replace row.
    pub fn open_find(&mut self, seed: Option<&str>, replace: bool) {
        self.v.find.open(seed, replace);
    }

    pub fn close_find(&mut self) {
        if self.v.find.open {
            self.v.find.close();
            self.v.focus_text = true;
        }
    }

    pub fn find_open(&self) -> bool {
        self.v.find.open
    }

    pub fn find_next(&mut self, doc: &Document) {
        findbar::nav(doc, &mut self.v, find::Dir::Next);
    }

    pub fn find_prev(&mut self, doc: &Document) {
        findbar::nav(doc, &mut self.v, find::Dir::Prev);
    }

    pub fn undo(&mut self, doc: &mut Document) {
        if let Some(s) = doc.undo_group() {
            self.v.set_sel(s);
        }
    }

    pub fn redo(&mut self, doc: &mut Document) {
        if let Some(s) = doc.redo_group() {
            self.v.set_sel(s);
        }
    }

    /// Comment / uncomment the selected lines with the language's line comment. No-op without one.
    pub fn toggle_comment(&mut self, doc: &mut Document) {
        let Some(prefix) = doc.lang().line_comment() else { return };
        self.v.clamp_to(doc);
        if let Some(s) = ops::toggle_comment(doc, self.v.sel, prefix, 0.0) {
            self.v.set_sel(s);
        }
    }

    /// Select the whole buffer.
    pub fn select_all(&mut self, doc: &Document) {
        self.v.sel = undo::Sel { anchor: (0, 0), head: doc.buf.end_pos() };
        self.v.up = false;
    }

    /// First visible line (to restore the view later with `set_top_line`).
    pub fn top_line(&self) -> u32 {
        self.v.top_line
    }

    pub fn set_top_line(&mut self, line: u32) {
        self.v.top_line = line;
        self.v.top_off = 0.0;
        self.v.reveal = view::Reveal::None;
    }

    // ---- Added (not in the frozen stub) ----

    /// Set the selection: `anchor` stays, `head` is the cursor. Clamped at the next `show`.
    pub fn set_selection(&mut self, anchor: Pos, head: Pos) {
        self.v.set_sel(undo::Sel { anchor, head });
    }

    /// Screen rect of the caret as drawn by the last `show` (`None` when off screen). For popups at the cursor.
    pub fn cursor_rect(&self) -> Option<egui::Rect> {
        self.v.caret
    }

    /// Current find query (empty when none).
    pub fn find_query(&self) -> &str {
        &self.v.find.text
    }

    /// Match counter as shown in the find bar (`3 of 17`, `No results`, `12…` while counting).
    pub fn find_status(&self) -> String {
        self.v.find.counter()
    }

    /// The context menu is open (it takes ↑/↓/Enter/Esc while open).
    pub fn context_menu_open(&self) -> bool {
        self.v.menu.is_some()
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
    /// "Find Callers" chosen in the context menu: the identifier.
    pub find_callers: Option<String>,
}

/// Draw the editor in all the space `ui` has left and handle its input.
/// The widget owns: typing, IME, clipboard, selection (mouse + keyboard, word/line by double/triple click),
/// undo/redo (cmd/ctrl+z, shift+z / y), indent/outdent, auto-indent, line move/duplicate/delete, find bar (when open),
/// context menu, scrolling, `EditorState::run` actions (applied here, in order; they take keyboard focus).
/// It does not own: save, open, tabs, any shortcut not listed above (left in the input for the shell).
pub fn show(ui: &mut egui::Ui, doc: &mut Document, state: &mut EditorState, opts: &EditorOptions, theme: &Theme, lenses: &[Lens]) -> EditorOutput {
    let queued = std::mem::take(&mut state.queued);
    view::show(ui, doc, &mut state.v, &queued, opts, theme, lenses)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}

    #[test]
    fn document_and_state_are_send() {
        assert_send::<Document>();
        assert_send::<EditorState>();
    }

    #[test]
    fn dirty_tracking_and_undo() {
        let mut d = Document::new("abc", Lang::Plain);
        let mut st = EditorState::default();
        assert!(!d.is_dirty());
        d.replace((0, 3), (0, 3), "d");
        assert!(d.is_dirty());
        st.undo(&mut d);
        assert!(!d.is_dirty());
        assert_eq!(d.text(), "abc");
        st.redo(&mut d);
        d.mark_saved();
        assert!(!d.is_dirty());
        st.undo(&mut d);
        assert!(d.is_dirty());
        st.redo(&mut d);
        assert!(!d.is_dirty());
        let r0 = d.revision();
        d.set_read_only(true);
        assert!(d.replace((0, 0), (0, 0), "x").is_none());
        st.undo(&mut d);
        assert_eq!((d.text().as_str(), d.revision()), ("abcd", r0));
        d.set_read_only(false);
        d.reload("new\r\n");
        assert!(!d.is_dirty() && !d.can_undo() && d.crlf());
    }

    #[test]
    fn mixed_endings_roundtrip_until_edited() {
        let t = "a\r\nb\nc\r\n";
        let mut d = Document::new(t, Lang::Plain);
        assert_eq!(d.text(), t);
        let mut st = EditorState::default();
        d.replace((0, 1), (0, 1), "!");
        assert_eq!(d.text(), "a!\r\nb\r\nc\r\n");
        st.undo(&mut d);
        assert_eq!(d.text(), t, "undo back to the loaded state restores the exact bytes");
    }

    #[test]
    fn typing_coalesces_and_undo_restores_selection() {
        let mut d = Document::new("", Lang::Plain);
        let mut s = undo::Sel::caret((0, 0));
        for (i, ch) in "hello".chars().enumerate() {
            s = ops::insert(&mut d, s, &ch.to_string(), undo::Kind::Type, i as f64 * 0.1).unwrap();
        }
        s = indent::newline(&mut d, s, &ops::Tabs { width: 4, soft: true }, true, 0.6).unwrap();
        s = ops::insert(&mut d, s, "x", undo::Kind::Type, 0.7).unwrap();
        s = ops::insert(&mut d, s, "y", undo::Kind::Type, 5.0).unwrap(); // pause
        assert_eq!(s.head, (1, 2));
        let mut st = EditorState::default();
        st.undo(&mut d);
        assert_eq!((d.text().as_str(), st.cursor()), ("hello\nx", (1, 1)));
        st.undo(&mut d);
        assert_eq!(d.text(), "hello\n");
        st.undo(&mut d);
        assert_eq!(d.text(), "hello");
        st.undo(&mut d);
        assert_eq!((d.text().as_str(), st.cursor()), ("", (0, 0)));
        st.redo(&mut d);
        assert_eq!((d.text().as_str(), st.cursor()), ("hello", (0, 5)));
    }
}
