//! Open tabs: file tabs (document + view state + disk metadata) and the Settings tab; the tab strip.
//!
//! Strip: flat; active tab = editor background + 2 px accent line on top; dirty = dot in the close slot;
//! × on hover; middle-click closes; same names get a muted parent-directory suffix; scrolls horizontally.

use egui::{pos2, vec2, CursorIcon, Id, Rect, Sense, Ui};
use margin_editor::{Document, EditorState, Lens};
use margin_proto::{Meta, RelPath, ReqId};
use margin_theme::Theme;

use crate::sanitize::clean;
use crate::widgets;

pub enum View {
    Loading,
    Text(Box<Document>),
    Binary(u64),
    Message(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Banner {
    /// Save refused: the file changed on disk since we read it.
    Conflict,
    /// Changed on disk while we have unsaved edits.
    ChangedDirty,
}

/// Outline + hint lines of one tab.
#[derive(Default)]
pub struct HintState {
    /// Revision the outline was computed from.
    pub outline_rev: Option<u64>,
    pub outline: Option<std::sync::Arc<margin_code::Outline>>,
    /// Revision of an outline job in flight.
    pub computing: Option<u64>,
    /// When the revision last changed (egui time), for the debounce.
    pub changed_at: f64,
    pub seen_rev: Option<u64>,
    pub lenses: Vec<Lens>,
    /// Outline revision a `Lens` request went out for (`None`: ask again).
    pub lens_sent: Option<u64>,
}

pub struct FileTab {
    pub path: RelPath,
    pub view: View,
    pub st: EditorState,
    pub meta: Option<Meta>,
    pub banner: Option<Banner>,
    /// Not valid UTF-8: shown lossy, read-only.
    pub bad_utf8: bool,
    pub saving: Option<ReqId>,
    pub hints: HintState,
    /// Cursor/selection to apply when the content arrives.
    pub pending_select: Option<(u32, u32, u32)>,
    pub focus_editor: bool,
}

impl FileTab {
    pub fn new(path: RelPath) -> FileTab {
        FileTab {
            path,
            view: View::Loading,
            st: EditorState::default(),
            meta: None,
            banner: None,
            bad_utf8: false,
            saving: None,
            hints: HintState::default(),
            pending_select: None,
            focus_editor: true,
        }
    }

    pub fn doc(&self) -> Option<&Document> {
        match &self.view {
            View::Text(d) => Some(d),
            _ => None,
        }
    }

    pub fn dirty(&self) -> bool {
        self.doc().is_some_and(Document::is_dirty)
    }
}

pub enum TabKind {
    File(Box<FileTab>),
    Settings,
}

pub struct Tab {
    pub id: u64,
    pub kind: TabKind,
}

impl Tab {
    pub fn file(&self) -> Option<&FileTab> {
        match &self.kind {
            TabKind::File(f) => Some(f),
            TabKind::Settings => None,
        }
    }

    pub fn file_mut(&mut self) -> Option<&mut FileTab> {
        match &mut self.kind {
            TabKind::File(f) => Some(f),
            TabKind::Settings => None,
        }
    }

    pub fn dirty(&self) -> bool {
        self.file().is_some_and(FileTab::dirty)
    }

    pub fn title(&self) -> String {
        match &self.kind {
            TabKind::File(f) => clean(f.path.name()),
            TabKind::Settings => "Settings".into(),
        }
    }
}

#[derive(Default)]
pub struct Tabs {
    pub list: Vec<Tab>,
    pub active: Option<usize>,
    next_id: u64,
    pub reveal_active: bool,
}

pub enum StripAction {
    Activate(usize),
    Close(usize),
}

impl Tabs {
    pub fn add(&mut self, kind: TabKind) -> usize {
        self.next_id += 1;
        let at = self.active.map_or(self.list.len(), |a| a + 1);
        self.list.insert(at, Tab { id: self.next_id, kind });
        self.activate(at);
        at
    }

    pub fn activate(&mut self, i: usize) {
        if i < self.list.len() {
            self.active = Some(i);
            self.reveal_active = true;
        }
    }

    pub fn remove(&mut self, i: usize) -> Option<Tab> {
        if i >= self.list.len() {
            return None;
        }
        let t = self.list.remove(i);
        self.active = match self.active {
            _ if self.list.is_empty() => None,
            Some(a) if a > i => Some(a - 1),
            Some(a) if a == i => Some(i.min(self.list.len() - 1)),
            a => a,
        };
        self.reveal_active = true;
        Some(t)
    }

    pub fn find_file(&self, p: &RelPath) -> Option<usize> {
        self.list.iter().position(|t| t.file().is_some_and(|f| &f.path == p))
    }

    pub fn by_id(&self, id: u64) -> Option<usize> {
        self.list.iter().position(|t| t.id == id)
    }

    pub fn file_by_id(&mut self, id: u64) -> Option<&mut FileTab> {
        self.list.iter_mut().find(|t| t.id == id).and_then(Tab::file_mut)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|a| self.list.get(a))
    }

    pub fn active_file(&self) -> Option<&FileTab> {
        self.active_tab().and_then(Tab::file)
    }

    pub fn active_file_mut(&mut self) -> Option<&mut FileTab> {
        let a = self.active?;
        self.list.get_mut(a).and_then(Tab::file_mut)
    }

    pub fn cycle(&mut self, d: isize) {
        let n = self.list.len() as isize;
        if let (Some(a), true) = (self.active, n > 0) {
            self.activate((a as isize + d).rem_euclid(n) as usize);
        }
    }

    /// Muted suffix (parent directory name) for tabs whose names collide.
    fn suffix(&self, i: usize) -> Option<String> {
        let f = self.list[i].file()?;
        let dup = self.list.iter().enumerate().any(|(j, t)| j != i && t.file().is_some_and(|g| g.path.name() == f.path.name()));
        if !dup {
            return None;
        }
        let parent = f.path.parent().unwrap_or_default();
        Some(if parent.is_root() { "./".into() } else { clean(parent.name()) })
    }

    /// Draw the strip in `rect`.
    pub fn strip(&mut self, ui: &mut Ui, rect: Rect, t: &Theme) -> Option<StripAction> {
        let p = ui.painter().clone();
        p.rect_filled(rect, 0.0, t.ui.panel_bg);
        // Under everything: the active tab's fill covers it, joining the editor.
        p.hline(rect.x_range(), rect.bottom() - 0.5, t.border());
        let mut act = None;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let reveal = std::mem::take(&mut self.reveal_active);
        egui::ScrollArea::horizontal().id_salt("tab-strip").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).auto_shrink([false, false]).show(&mut child, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.horizontal(|ui| {
                for i in 0..self.list.len() {
                    let title = self.list[i].title();
                    let suffix = self.suffix(i);
                    let active = self.active == Some(i);
                    let dirty = self.list[i].dirty();
                    let name_w = ui.painter().layout_no_wrap(title.clone(), t.font_ui(), t.ui.text).size().x.min(220.0);
                    let suf_w = suffix.as_ref().map_or(0.0, |s| ui.painter().layout_no_wrap(s.clone(), t.font_small(), t.ui.text).size().x.min(120.0) + 6.0);
                    let w = (12.0 + name_w + suf_w + 8.0 + 16.0 + 8.0).round();
                    let (r, resp) = ui.allocate_exact_size(vec2(w, rect.height()), Sense::click());
                    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                    if active && reveal {
                        ui.scroll_to_rect(r, None);
                    }
                    let painter = ui.painter();
                    if active {
                        painter.rect_filled(r, 0.0, t.code.bg);
                        painter.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 2.0)), 0.0, t.ui.accent);
                    } else if resp.hovered() {
                        painter.rect_filled(r, 0.0, t.ui.row_hover);
                    }
                    painter.vline(r.right() - 0.5, r.y_range(), t.border());
                    let color = if active { t.ui.text_strong } else { t.ui.text_muted };
                    let x = r.left() + 12.0;
                    let tw = widgets::text_at(ui, r.with_max_x(r.right() - 28.0), x, &title, t.font_ui(), color);
                    if let Some(s) = &suffix {
                        widgets::text_at(ui, r.with_max_x(r.right() - 28.0), x + tw + 6.0, s, t.font_small(), t.ui.text_faint);
                    }
                    let c = pos2(r.right() - 16.0, r.center().y);
                    let cr = Rect::from_center_size(c, vec2(18.0, 18.0));
                    let close = ui.interact(cr, Id::unique(("tab-close", self.list[i].id)), Sense::click());
                    let painter = ui.painter();
                    if close.hovered() {
                        painter.rect_filled(cr, 3.0, t.ui.row_selected);
                    }
                    if close.hovered() || (resp.hovered() && !dirty) || (active && !dirty) {
                        let col = if close.hovered() { t.ui.text_strong } else { t.ui.text_muted };
                        widgets::close_x(painter, c, 3.5, col);
                    } else if dirty {
                        widgets::dot(painter, c, 3.5, if active { t.ui.text } else { t.ui.text_muted });
                    }
                    if close.clicked() || resp.middle_clicked() {
                        act = Some(StripAction::Close(i));
                    } else if resp.clicked() {
                        act = Some(StripAction::Activate(i));
                    }
                }
            });
        });
        act
    }
}
