//! Sidebar top: folder name, one search field (`Aa` `W` `.*`), `Names | Content`, filters (include / exclude);
//! below it the file tree, or a flat list of matching files.
//!
//! | Mode | Field | Below |
//! |---|---|---|
//! | Names, empty query and filters | — | the tree |
//! | Names, query or filters | live `FindFiles` (80 ms debounce, stale replies dropped, old rows kept) | matching files |
//! | Content | Enter opens a new search tab | the tree, or files matching the filters |
//!
//! Keys in the field: Up / Down move in the list, Enter opens (Names) or searches (Content),
//! Esc clears the query, a second Esc returns to the editor.

use egui::{pos2, vec2, Color32, CursorIcon, FontId, Id, Key, Pos2, Rect, Sense, Ui};
use margin_proto::{FileMatch, RelPath, ReqId, SearchQuery};
use margin_theme::Theme;

use crate::palette::runs;
use crate::sanitize::floor_char;
use crate::search_panel::{thousands, MAX_HITS};
use crate::widgets;

pub const DEBOUNCE: f64 = 0.08;
pub const NAMES_LIMIT: u32 = 2000;
const MAX_POSITIONS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SideMode {
    #[default]
    Names,
    Content,
}

/// `(query, include, exclude)` of a `FindFiles`.
pub type NamesQuery = (String, String, String);

#[derive(Default)]
pub struct SideSearch {
    pub query: String,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    pub mode: SideMode,
    pub include: String,
    pub exclude: String,
    pub filters_open: bool,
    /// Focus the field next frame.
    pub focus: bool,
    pub edited_at: Option<f64>,
    pub req: Option<ReqId>,
    /// Query of the rows on screen (or in flight).
    pub sent: Option<NamesQuery>,
    pub items: Vec<FileMatch>,
    pub total: u32,
    /// A reply arrived for `sent`.
    pub got: bool,
    pub selected: usize,
    reveal: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SideOut {
    Open(RelPath),
    /// Enter in Content mode.
    Search(SearchQuery),
    /// Esc on an empty field.
    Leave,
    /// Right-click on a list row.
    Menu(RelPath, Pos2),
}

pub fn field_id() -> Id {
    Id::unique("side-query")
}

pub fn list_id() -> Id {
    Id::unique("side-list")
}

pub fn mode_id() -> Id {
    Id::unique("side-mode")
}

pub fn filters_id() -> Id {
    Id::unique("side-filters")
}

pub fn glob_id(i: usize) -> Id {
    Id::unique(("side-glob", i))
}

pub fn side_toggle_id(label: &str) -> Id {
    Id::unique(("side-toggle", label))
}

impl SideSearch {
    /// The `FindFiles` the list needs, or `None` when the tree shows.
    pub fn names_query(&self) -> Option<NamesQuery> {
        let q = if self.mode == SideMode::Names { self.query.trim().to_owned() } else { String::new() };
        let (i, e) = (self.include.trim().to_owned(), self.exclude.trim().to_owned());
        (!q.is_empty() || !i.is_empty() || !e.is_empty()).then_some((q, i, e))
    }

    pub fn list_active(&self) -> bool {
        self.names_query().is_some()
    }

    /// A query to send now (debounced, only when it differs from the one on screen).
    pub fn due(&mut self, now: f64) -> Option<NamesQuery> {
        let q = self.names_query()?;
        if self.sent.as_ref() == Some(&q) || self.edited_at.is_some_and(|at| now - at < DEBOUNCE) {
            return None;
        }
        self.edited_at = None;
        self.sent = Some(q.clone());
        self.got = false;
        Some(q)
    }

    pub fn wait_left(&self, now: f64) -> Option<f64> {
        let q = self.names_query()?;
        (self.sent.as_ref() != Some(&q)).then(|| self.edited_at.map_or(0.0, |at| (DEBOUNCE - (now - at)).max(0.0)))
    }

    /// Back to the tree: drop rows. Returns the request to cancel.
    pub fn reset_list(&mut self) -> Option<ReqId> {
        self.items.clear();
        self.sent = None;
        self.got = false;
        self.selected = 0;
        self.req.take()
    }

    pub fn set_items(&mut self, mut items: Vec<FileMatch>, total: u32) {
        items.truncate(NAMES_LIMIT as usize);
        for it in &mut items {
            it.positions.truncate(MAX_POSITIONS);
            it.positions.sort_unstable();
        }
        self.items = items;
        self.total = total;
        self.got = true;
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    /// Content search for Enter.
    pub fn content_query(&self) -> Option<SearchQuery> {
        let p = self.query.clone();
        (p.chars().count() >= 2 || (self.regex && !p.is_empty())).then(|| SearchQuery {
            pattern: p,
            regex: self.regex,
            case_sensitive: self.case,
            whole_word: self.word,
            include: self.include.trim().to_owned(),
            exclude: self.exclude.trim().to_owned(),
            max_hits: MAX_HITS,
        })
    }

    pub fn count_line(&self) -> String {
        if !self.got {
            return if self.items.is_empty() { "Searching…".into() } else { String::new() };
        }
        let n = self.items.len();
        if n == 0 {
            "No matching files".into()
        } else if n >= NAMES_LIMIT as usize {
            format!("first {} matches of {} files", thousands(n as u64), thousands(u64::from(self.total)))
        } else {
            format!("{} {}", thousands(n as u64), if n == 1 { "file" } else { "files" })
        }
    }

    /// Header, field, mode and filters. Returns the rect left for the tree / list.
    pub fn head(&mut self, ui: &mut Ui, rect: Rect, t: &Theme, now: f64, name: &str, out: &mut Vec<SideOut>) -> Rect {
        let row_h = t.row_h();
        let pad = 12.0;
        let head_h = (row_h + 8.0).round();
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), head_h));
        widgets::text_at(ui, head.with_max_x(head.right() - pad), head.left() + 16.0, name, t.font_ui(), t.ui.text_strong);
        let fh = (row_h + 2.0).round();
        let mut y = head.bottom();
        let fr = Rect::from_min_size(pos2(rect.left() + pad, y), vec2(rect.width() - 2.0 * pad, fh));
        let fid = field_id();
        // Keys the field would otherwise take.
        if ui.memory(|m| m.has_focus(fid)) {
            let m = egui::Modifiers::NONE;
            let (up, down, enter, esc) = ui.input_mut(|i| (i.consume_key(m, Key::ArrowUp), i.consume_key(m, Key::ArrowDown), i.consume_key(m, Key::Enter), i.consume_key(m, Key::Escape)));
            if up || down || esc {
                widgets::hold_focus(ui);
            }
            let n = self.items.len();
            if self.list_active() && n > 0 {
                if down {
                    self.selected = (self.selected + 1).min(n - 1);
                    self.reveal = true;
                }
                if up {
                    self.selected = self.selected.saturating_sub(1);
                    self.reveal = true;
                }
            }
            if enter {
                match self.mode {
                    SideMode::Names => {
                        if let Some(it) = self.items.get(self.selected).filter(|_| self.list_active()) {
                            out.push(SideOut::Open(it.path.clone()));
                        }
                    }
                    SideMode::Content => {
                        if let Some(q) = self.content_query() {
                            out.push(SideOut::Search(q));
                        }
                    }
                }
            }
            if esc {
                if self.query.is_empty() {
                    ui.memory_mut(|m| m.surrender_focus(fid));
                    out.push(SideOut::Leave);
                } else {
                    self.query.clear();
                    self.edited_at = Some(now - DEBOUNCE);
                    self.selected = 0;
                }
            }
        }
        let names = self.mode == SideMode::Names;
        let hint = if names { "name, or glob: *.md, src/**/*.c" } else { "Search in files, then Enter" };
        let before = self.query.clone();
        let resp = widgets::field_pad(ui, fr, fid, &mut self.query, hint, t, t.font_ui(), 74.0);
        // Keep Esc / arrows for us (egui drops focus on Esc before we see it). After the TextEdit: last filter wins.
        ui.memory_mut(|m| m.set_focus_lock_filter(fid, egui::EventFilter { vertical_arrows: true, horizontal_arrows: true, tab: false, escape: true }));
        if std::mem::take(&mut self.focus) {
            resp.request_focus();
            widgets::cursor_to_end(ui.ctx(), fid, &self.query);
        }
        if self.query != before {
            self.edited_at = Some(now);
            self.selected = 0;
        }
        let tw = 22.0;
        let mut x = fr.right() - 4.0 - 3.0 * tw;
        for (label, on, tip) in [("Aa", &mut self.case, "Match case"), ("W", &mut self.word, "Whole word"), (".*", &mut self.regex, "Regular expression")] {
            let r = Rect::from_min_size(pos2(x, fr.top() + 3.0), vec2(tw - 2.0, fh - 6.0));
            if names {
                widgets::text_toggle_off(ui, r, label, t);
            } else {
                widgets::text_toggle(ui, r, label, on, tip, t, side_toggle_id(label));
            }
            x += tw;
        }
        y = fr.bottom() + 6.0;
        // Mode + filters disclosure.
        let sh = (row_h - 4.0).round();
        let seg = Rect::from_min_size(pos2(rect.left() + pad, y), vec2(136.0f32.min(rect.width() * 0.6), sh));
        if let Some(i) = widgets::segmented(ui, seg, &["Names", "Content"], usize::from(!names), t, mode_id()) {
            let m = if i == 1 { SideMode::Content } else { SideMode::Names };
            if m != self.mode {
                self.mode = m;
                self.edited_at = Some(now - DEBOUNCE);
                self.selected = 0;
            }
            self.focus = true;
        }
        let set = !self.include.trim().is_empty() || !self.exclude.trim().is_empty();
        let label = if set { "Filters · on" } else { "Filters" };
        let lw = ui.painter().layout_no_wrap(label.to_owned(), t.font_small(), Color32::PLACEHOLDER).size().x;
        let dr = Rect::from_min_max(pos2(rect.right() - pad - lw - 18.0, y), pos2(rect.right() - pad + 2.0, y + sh));
        let d = ui.interact(dr, filters_id(), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if d.clicked() {
            self.filters_open = !self.filters_open;
        }
        let col = if d.hovered() || set { t.ui.text } else { t.ui.text_muted };
        widgets::chevron(ui.painter(), pos2(dr.left() + 6.0, dr.center().y), self.filters_open, col);
        widgets::text_at(ui, dr, dr.left() + 16.0, label, t.font_small(), col);
        y = seg.bottom() + 6.0;
        if self.filters_open {
            for (i, hint) in ["include, e.g. *.rs, src/**", "exclude, e.g. *.min.js, target/**"].into_iter().enumerate() {
                let r = Rect::from_min_size(pos2(rect.left() + pad, y), vec2(rect.width() - 2.0 * pad, fh));
                let text = if i == 0 { &mut self.include } else { &mut self.exclude };
                let before = text.clone();
                widgets::field(ui, r, glob_id(i), text, hint, t, t.font_small());
                if *text != before {
                    self.edited_at = Some(now);
                    self.selected = 0;
                }
                y = r.bottom() + 4.0;
            }
            y += 2.0;
        }
        ui.painter().hline(rect.x_range(), y.round() + 0.5, t.border());
        Rect::from_min_max(pos2(rect.left(), y.round() + 1.0), rect.max)
    }

    /// Flat list of matching files.
    pub fn list(&mut self, ui: &mut Ui, rect: Rect, t: &Theme, out: &mut Vec<SideOut>) {
        let row_h = t.row_h();
        let cr = Rect::from_min_size(rect.min + vec2(0.0, 2.0), vec2(rect.width(), row_h - 2.0));
        widgets::text_at(ui, cr, cr.left() + 16.0, &self.count_line(), t.font_small(), t.ui.text_muted);
        let lr = Rect::from_min_max(pos2(rect.left(), cr.bottom() + 2.0), rect.max);
        let id = list_id();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lr));
        child.set_clip_rect(lr);
        child.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let reveal = std::mem::take(&mut self.reveal).then_some(self.selected);
        let mut clicked = None;
        let mut menu = None;
        let sel = self.selected;
        widgets::virtual_rows(&mut child, Id::unique("side-list-scroll"), self.items.len(), row_h, reveal, |ui, i, r| {
            let Some(m) = self.items.get(i) else { return };
            let resp = ui.interact(r, id.with(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            widgets::row_bg(ui.painter(), r, t, resp.hovered(), i == sel);
            let path = m.path.as_str();
            let name = m.path.name();
            let nb = floor_char(path, path.len() - name.len());
            let dir = path[..nb].trim_end_matches('/');
            let mut parts_s = runs(name, nb, &m.positions, 120);
            let name_n = parts_s.len();
            if !dir.is_empty() {
                parts_s.push(("  ".into(), false));
                parts_s.extend(runs(dir, 0, &m.positions, 200));
            }
            let hi = |h: bool, base: Color32| if h { t.ui.accent } else { base };
            let parts: Vec<(&str, FontId, Color32)> = parts_s
                .iter()
                .enumerate()
                .map(|(k, (s, h))| if k < name_n { (s.as_str(), t.font_ui(), hi(*h, t.ui.text)) } else { (s.as_str(), t.font_small(), hi(*h, t.ui.text_muted)) })
                .collect();
            widgets::paint_job(ui, r.with_max_x(r.right() - 8.0), r.left() + 16.0, &parts);
            if resp.clicked() {
                clicked = Some(i);
            }
            if resp.secondary_clicked() {
                menu = Some((i, resp.interact_pointer_pos().unwrap_or(r.center())));
            }
        });
        if let Some(i) = clicked {
            self.selected = i;
            if let Some(m) = self.items.get(i) {
                out.push(SideOut::Open(m.path.clone()));
            }
        }
        if let Some((i, pos)) = menu {
            self.selected = i;
            if let Some(m) = self.items.get(i) {
                out.push(SideOut::Menu(m.path.clone(), pos));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(p: &str) -> FileMatch {
        FileMatch { path: RelPath::new(p).unwrap(), score: 0, positions: vec![9, 0, 3] }
    }

    #[test]
    fn names_query_debounce_and_modes() {
        let mut s = SideSearch::default();
        assert!(s.names_query().is_none(), "empty: the tree");
        s.query = " *.md ".into();
        s.edited_at = Some(1.0);
        assert!(s.due(1.05).is_none(), "debounced");
        assert_eq!(s.due(1.09), Some(("*.md".into(), String::new(), String::new())));
        assert!(s.due(2.0).is_none(), "same query: not again");
        // Content mode: the query does not filter names; filters do.
        s.mode = SideMode::Content;
        assert!(s.names_query().is_none());
        s.include = "src/**".into();
        assert_eq!(s.names_query(), Some((String::new(), "src/**".into(), String::new())));
        s.query = "x".into();
        assert!(s.content_query().is_none(), "one char");
        s.query = "xy".into();
        assert_eq!(s.content_query().unwrap().include, "src/**");
    }

    #[test]
    fn count_line_and_caps() {
        let mut s = SideSearch { sent: Some(Default::default()), ..Default::default() };
        assert_eq!(s.count_line(), "Searching…");
        s.set_items(vec![fm("a.md"), fm("b/c.md")], 10);
        assert_eq!(s.count_line(), "2 files");
        assert_eq!(s.items[0].positions, vec![0, 3, 9]);
        let many: Vec<FileMatch> = (0..2100).map(|i| fm(&format!("f{i}"))).collect();
        s.set_items(many, 48_211);
        assert_eq!(s.items.len(), 2000);
        assert_eq!(s.count_line(), "first 2,000 matches of 48,211 files");
        s.set_items(vec![], 1);
        assert_eq!(s.count_line(), "No matching files");
    }
}
