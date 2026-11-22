//! Results panel (bottom of the editor area): callers, definitions, callees, sinks, reach chains.
//! Rows: `path:line` (muted mono) + snippet with the name highlighted + context (muted, right).
//! Click / Enter jumps; Up / Down move; Esc or × closes; the top hairline drags to resize.

use egui::{pos2, vec2, CursorIcon, Id, Key, Rect, Sense, Ui};
use margin_proto::limits::MAX_ITEMS;
use margin_proto::Site;
use margin_theme::Theme;

use crate::sanitize::{clean, sanitize};
use crate::widgets;

pub struct ResultsPanel {
    pub title: String,
    pub sites: Vec<Site>,
    pub truncated: bool,
    pub loading: bool,
    pub selected: Option<usize>,
    pub height: f32,
    pub focus: bool,
    reveal: Option<usize>,
}

pub enum ResultsAction {
    Jump(Site),
    Close,
}

pub fn panel_id() -> Id {
    Id::unique("results-panel")
}

impl ResultsPanel {
    pub fn new(title: String, height: f32) -> ResultsPanel {
        ResultsPanel { title, sites: Vec::new(), truncated: false, loading: true, selected: None, height, focus: false, reveal: None }
    }

    pub fn set(&mut self, mut sites: Vec<Site>, truncated: bool) {
        let truncated = truncated || sites.len() > MAX_ITEMS;
        sites.truncate(MAX_ITEMS);
        self.sites = sites;
        self.truncated = truncated;
        self.loading = false;
        self.selected = (!self.sites.is_empty()).then_some(0);
    }

    pub fn header(&self) -> String {
        let n = crate::search_panel::thousands(self.sites.len() as u64);
        let mut s = format!("{} · {n}", self.title);
        if self.truncated {
            s += "+";
        }
        if self.loading {
            s = format!("{} · …", self.title);
        }
        s
    }

    /// Draw in `rect` (includes the header row). `y` of the top edge is the resize handle.
    pub fn show(&mut self, ui: &mut Ui, rect: Rect, t: &Theme) -> Option<ResultsAction> {
        let mut act = None;
        let row_h = t.row_h();
        let p = ui.painter().clone();
        p.rect_filled(rect, 0.0, t.ui.panel_bg);
        p.hline(rect.x_range(), rect.top() + 0.5, t.border());
        let id = panel_id();
        let bg = ui.interact(rect, id, Sense::click());
        if bg.clicked() || std::mem::take(&mut self.focus) {
            ui.memory_mut(|m| m.request_focus(id));
        }
        let focused = ui.memory(|m| m.has_focus(id));
        if focused {
            ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { vertical_arrows: true, horizontal_arrows: false, tab: false, escape: true }));
            let (up, down, enter, esc) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(egui::Modifiers::NONE, Key::Enter),
                    i.consume_key(egui::Modifiers::NONE, Key::Escape),
                )
            });
            let n = self.sites.len();
            if n > 0 {
                if down {
                    self.selected = Some(self.selected.map_or(0, |s| (s + 1).min(n - 1)));
                    self.reveal = self.selected;
                }
                if up {
                    self.selected = Some(self.selected.map_or(0, |s| s.saturating_sub(1)));
                    self.reveal = self.selected;
                }
                if enter {
                    if let Some(s) = self.selected.and_then(|i| self.sites.get(i)) {
                        act = Some(ResultsAction::Jump(s.clone()));
                    }
                }
            }
            if esc {
                act = Some(ResultsAction::Close);
            }
        }
        // Resize handle on the top hairline.
        let hr = Rect::from_min_max(pos2(rect.left(), rect.top() - 3.0), pos2(rect.right(), rect.top() + 3.0));
        let h = ui.interact(hr, id.with("resize"), Sense::drag()).on_hover_cursor(CursorIcon::ResizeVertical);
        if h.dragged() {
            self.height = (self.height - h.drag_delta().y).clamp(80.0, 900.0);
        }
        // Header.
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), row_h + 2.0));
        widgets::text_at(ui, head.with_max_x(head.right() - 36.0), head.left() + 12.0, &self.header(), t.font_small(), t.ui.text_muted);
        let cr = Rect::from_center_size(pos2(head.right() - 18.0, head.center().y), vec2(20.0, 20.0));
        let close = ui.interact(cr, id.with("close"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Close (Esc)");
        if close.hovered() {
            ui.painter().rect_filled(cr, 3.0, t.ui.row_selected);
        }
        widgets::close_x(ui.painter(), cr.center(), 3.5, if close.hovered() { t.ui.text_strong } else { t.ui.text_muted });
        if close.clicked() {
            act = Some(ResultsAction::Close);
        }
        let list = Rect::from_min_max(pos2(rect.left(), head.bottom()), rect.max);
        if self.sites.is_empty() && !self.loading {
            let r = Rect::from_min_size(list.min, vec2(list.width(), row_h));
            widgets::text_at(ui, r, r.left() + 12.0, "Nothing found", t.font_small(), t.ui.text_faint);
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list));
        child.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let loc_w = 230.0f32.min(list.width() * 0.35);
        let mut clicked = None;
        let reveal = self.reveal.take();
        widgets::virtual_rows(&mut child, id.with("scroll"), self.sites.len(), row_h, reveal, |ui, i, r| {
            let s = &self.sites[i];
            let resp = ui.interact(r, id.with(("row", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            widgets::row_bg(ui.painter(), r, t, resp.hovered(), self.selected == Some(i));
            let loc = format!("{}:{}", clean(s.path.as_str()), u64::from(s.line) + 1);
            widgets::text_at(ui, r.with_max_x(r.left() + 12.0 + loc_w - 12.0), r.left() + 12.0, &loc, t.font_mono_ui(), t.ui.text_muted);
            let ctx_w = if s.context.is_empty() { 0.0 } else { widgets::text_right(ui, r.with_min_x(r.right() - 260.0), r.right() - 12.0, &sanitize(&s.context, 80), t.font_small(), t.ui.text_faint) + 16.0 };
            widgets::snippet(ui, r.with_max_x(r.right() - 12.0 - ctx_w), r.left() + 12.0 + loc_w, &s.text, s.text_col, s.len, t);
            if resp.clicked() {
                clicked = Some(i);
            }
        });
        if let Some(i) = clicked {
            self.selected = Some(i);
            ui.memory_mut(|m| m.request_focus(id));
            act = Some(ResultsAction::Jump(self.sites[i].clone()));
        }
        act
    }
}
