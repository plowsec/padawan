//! Search tab body (one per `Search: …` tab). Streams `SearchBatch`…`SearchDone`; results sorted by path then
//! line, grouped by file (collapsible), virtualised. Every widget id is salted with the tab id.
//!
//! | Input | Does |
//! |---|---|
//! | typing in the pattern (≥ 2 chars) | search after 150 ms; the previous request is cancelled |
//! | Enter in the pattern / a filter | search now |
//! | `Aa` `W` `.*` | case, whole word, regex (search now) |
//! | `>` left of the pattern | show / hide the replace row |
//! | Down in the pattern | into the results |
//! | Up / Down (results focused) | move through hits; Enter opens |
//! | right-click a hit | Copy Line, Copy Path |
//! | `Replace All`, `Replace` on a file row | ask, then `Request::Replace` |

use std::collections::HashSet;

use egui::{pos2, vec2, CornerRadius, CursorIcon, Id, Key, Pos2, Rect, Sense, Stroke, Ui};
use margin_proto::limits::MAX_ITEMS;
use margin_proto::{RelPath, ReqId, SearchQuery, SearchStats, Site};
use margin_theme::Theme;

use crate::sanitize::{clean, sanitize};
use crate::widgets;

pub const DEBOUNCE: f64 = 0.15;
pub const MAX_HITS: u32 = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SRow {
    /// Index into `groups`.
    Header(usize),
    /// Index into `hits`.
    Hit(usize),
}

#[derive(Clone, Copy, Debug)]
struct Group {
    start: usize,
    len: usize,
}

#[derive(Default)]
pub struct SearchPanel {
    /// Tab id (widget ids).
    pub tab: u64,
    pub query: String,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    pub include: String,
    pub exclude: String,
    pub replace: String,
    pub show_replace: bool,
    pub req: Option<ReqId>,
    /// Query of the results on screen (or in flight). Replace uses it.
    pub last: Option<SearchQuery>,
    /// egui time of the last edit not yet searched.
    pub edited_at: Option<f64>,
    pub hits: Vec<Site>,
    unsorted: bool,
    groups: Vec<Group>,
    rows: Vec<SRow>,
    rows_dirty: bool,
    collapsed: HashSet<RelPath>,
    pub stats: Option<SearchStats>,
    pub error: Option<String>,
    /// After a replace: files left alone and why (sanitised, a few).
    pub note: Option<String>,
    pub running: bool,
    /// Selected row (index into `rows`).
    selected: Option<usize>,
    pub focus_query: bool,
    pub focus_replace: bool,
    reveal: Option<usize>,
    /// Hits dropped because of our own cap.
    capped: bool,
    /// A new request is out: its first message replaces the old results.
    fresh: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpenHit {
    pub path: RelPath,
    pub line: u32,
    pub col: u32,
    pub len: u32,
}

/// What the tab asks the app for this frame.
#[derive(Default)]
pub struct SearchOut {
    pub send: Option<SearchQuery>,
    pub open: Option<OpenHit>,
    /// Replace asked: `None` = every file, `Some(p)` = that file.
    pub replace: Option<Option<RelPath>>,
    /// Right-click on a hit: (line text, path, pointer).
    pub menu: Option<(String, RelPath, Pos2)>,
}

pub fn query_id(tab: u64) -> Id {
    Id::unique(("search-query", tab))
}

pub fn replace_id(tab: u64) -> Id {
    Id::unique(("search-replace", tab))
}

pub fn results_id(tab: u64) -> Id {
    Id::unique(("search-results", tab))
}

pub fn toggle_id(tab: u64, label: &str) -> Id {
    Id::unique(("search-toggle", tab, label))
}

pub fn replace_all_id(tab: u64) -> Id {
    Id::unique(("search-replace-all", tab))
}

pub fn replace_file_id(tab: u64, group: usize) -> Id {
    Id::unique(("search-replace-file", tab, group))
}

pub fn replace_toggle_id(tab: u64) -> Id {
    Id::unique(("search-replace-toggle", tab))
}

impl SearchPanel {
    pub fn new(tab: u64) -> SearchPanel {
        SearchPanel { tab, ..Default::default() }
    }

    /// Tab title: `Search: <pattern>`.
    pub fn title(&self) -> String {
        let p = self.last.as_ref().map_or(self.query.as_str(), |q| q.pattern.as_str());
        if p.is_empty() {
            "Search".into()
        } else {
            format!("Search: {}", sanitize(p, 24))
        }
    }

    pub fn clear_results(&mut self) {
        self.hits.clear();
        self.groups.clear();
        self.rows.clear();
        self.stats = None;
        self.error = None;
        self.selected = None;
        self.capped = false;
        self.rows_dirty = false;
    }

    /// The query to send now, if any. `now`: egui time. Clears the debounce.
    pub fn due(&mut self, now: f64) -> Option<SearchQuery> {
        let at = self.edited_at?;
        if now - at < DEBOUNCE {
            return None;
        }
        self.edited_at = None;
        self.make_query()
    }

    /// Seconds until `due` fires (for `request_repaint_after`).
    pub fn wait_left(&self, now: f64) -> Option<f64> {
        self.edited_at.map(|at| (DEBOUNCE - (now - at)).max(0.0))
    }

    pub fn make_query(&self) -> Option<SearchQuery> {
        (self.query.chars().count() >= 2 || (self.regex && !self.query.is_empty())).then(|| SearchQuery {
            pattern: self.query.clone(),
            regex: self.regex,
            case_sensitive: self.case,
            whole_word: self.word,
            include: self.include.clone(),
            exclude: self.exclude.clone(),
            max_hits: MAX_HITS,
        })
    }

    /// A new request went out: old results stay until its first batch (no flicker).
    pub fn started(&mut self, id: ReqId, q: SearchQuery) {
        self.req = Some(id);
        self.last = Some(q);
        self.running = true;
        self.error = None;
        self.stats = None;
        self.fresh = true;
    }

    pub fn on_batch(&mut self, mut hits: Vec<Site>) {
        if std::mem::take(&mut self.fresh) {
            self.collapsed.clear();
            self.clear_results();
        }
        let room = MAX_ITEMS.saturating_sub(self.hits.len());
        if hits.len() > room {
            hits.truncate(room);
            self.capped = true;
        }
        self.hits.append(&mut hits);
        self.unsorted = true;
        self.rows_dirty = true;
    }

    pub fn on_done(&mut self, stats: SearchStats) {
        if std::mem::take(&mut self.fresh) {
            self.clear_results();
        }
        self.running = false;
        self.req = None;
        self.stats = Some(stats);
    }

    pub fn on_err(&mut self, msg: &str) {
        if std::mem::take(&mut self.fresh) {
            self.clear_results();
        }
        self.running = false;
        self.req = None;
        self.error = Some(sanitize(msg, 300));
    }

    fn rebuild(&mut self) {
        if std::mem::take(&mut self.unsorted) {
            // Adaptive merge sort: the sorted prefix + a sorted-ish batch merge in ~linear time.
            self.hits.sort_by(|a, b| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));
        }
        self.rows_dirty = false;
        self.groups.clear();
        let mut i = 0;
        while i < self.hits.len() {
            let mut j = i + 1;
            while j < self.hits.len() && self.hits[j].path == self.hits[i].path {
                j += 1;
            }
            self.groups.push(Group { start: i, len: j - i });
            i = j;
        }
        self.rows.clear();
        for (g, gr) in self.groups.iter().enumerate() {
            self.rows.push(SRow::Header(g));
            if !self.collapsed.contains(&self.hits[gr.start].path) {
                self.rows.extend((gr.start..gr.start + gr.len).map(SRow::Hit));
            }
        }
        if self.selected.is_some_and(|s| s >= self.rows.len()) {
            self.selected = None;
        }
    }

    fn fresh_rows(&mut self) {
        if self.rows_dirty || self.unsorted {
            self.rebuild();
        }
    }

    pub fn files(&self) -> usize {
        self.groups.len()
    }

    /// Hits in display order (tests).
    pub fn ordered(&mut self) -> &[Site] {
        self.fresh_rows();
        &self.hits
    }

    /// Files with hits, in display order.
    pub fn file_paths(&mut self) -> Vec<RelPath> {
        self.fresh_rows();
        self.groups.iter().map(|g| self.hits[g.start].path.clone()).collect()
    }

    /// Hits in one file.
    pub fn hits_in(&self, p: &RelPath) -> usize {
        self.groups.iter().find(|g| &self.hits[g.start].path == p).map_or(0, |g| g.len)
    }

    /// Selected hit (tests).
    pub fn selected_hit(&self) -> Option<&Site> {
        match self.selected.and_then(|s| self.rows.get(s))? {
            SRow::Hit(h) => self.hits.get(*h),
            SRow::Header(_) => None,
        }
    }

    pub fn summary(&self) -> String {
        if self.running && self.hits.is_empty() {
            return "Searching…".into();
        }
        let n = self.hits.len();
        let files = self.groups.len();
        if n == 0 && self.stats.is_some() {
            return "No results".into();
        }
        let mut s = format!("{} {} in {} {}", thousands(n as u64), if n == 1 { "result" } else { "results" }, thousands(files as u64), if files == 1 { "file" } else { "files" });
        if let Some(st) = self.stats {
            s += &format!(" · {} ms", st.elapsed_ms);
            if st.truncated || self.capped {
                s += &format!(" · first {} shown", thousands(n as u64));
            }
        } else if self.running {
            s += " · searching…";
        }
        s
    }

    fn keyboard(&mut self, ui: &Ui) -> Option<OpenHit> {
        let (up, down, enter) = ui.input_mut(|i| {
            (i.consume_key(egui::Modifiers::NONE, Key::ArrowUp), i.consume_key(egui::Modifiers::NONE, Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, Key::Enter))
        });
        if down {
            self.step(1);
        }
        if up {
            self.step(-1);
        }
        if enter {
            return self.selected_open();
        }
        None
    }

    fn selected_open(&self) -> Option<OpenHit> {
        let s = self.selected_hit()?;
        Some(OpenHit { path: s.path.clone(), line: s.line, col: s.col, len: s.len })
    }

    /// Move the selection to the next / previous hit row.
    fn step(&mut self, d: isize) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        let from = self.selected;
        let mut i = from.map_or(if d > 0 { -1 } else { n as isize }, |x| x as isize);
        loop {
            i += d;
            if i < 0 || i >= n as isize {
                break;
            }
            if matches!(self.rows[i as usize], SRow::Hit(_)) {
                self.selected = Some(i as usize);
                break;
            }
        }
        self.reveal = self.selected;
    }

    /// Draw in `rect` (the whole tab body).
    pub fn show(&mut self, ui: &mut Ui, rect: Rect, t: &Theme, now: f64) -> SearchOut {
        let mut out = SearchOut::default();
        let tab = self.tab;
        let row_h = t.row_h();
        ui.painter().rect_filled(rect, 0.0, t.code.bg);
        let pad = 16.0;
        let fh = (row_h + 2.0).round();
        let left = rect.left() + pad + 24.0;
        let fw = (rect.width() - 2.0 * pad - 24.0).clamp(120.0, 720.0);
        let mut y = rect.top() + 12.0;

        // Replace disclosure.
        let cr = Rect::from_min_size(pos2(rect.left() + pad, y + (fh - 20.0) / 2.0), vec2(20.0, 20.0));
        let c = ui.interact(cr, replace_toggle_id(tab), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text("Replace");
        if c.hovered() {
            ui.painter().rect_filled(cr, 3.0, t.ui.row_hover);
        }
        widgets::chevron(ui.painter(), cr.center(), self.show_replace, if c.hovered() { t.ui.text_strong } else { t.ui.text_muted });
        if c.clicked() {
            self.show_replace = !self.show_replace;
            self.focus_replace = self.show_replace;
        }

        // Pattern.
        let fr = Rect::from_min_size(pos2(left, y), vec2(fw, fh));
        let qid = query_id(tab);
        let before = self.query.clone();
        let has_focus = ui.memory(|m| m.has_focus(qid));
        if has_focus && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown)) {
            widgets::hold_focus(ui);
            ui.memory_mut(|m| m.request_focus(results_id(tab)));
            self.fresh_rows();
            self.selected = None;
            self.step(1);
        }
        let resp = widgets::field_pad(ui, fr, qid, &mut self.query, "Search the folder", t, t.font_ui(), 74.0);
        if std::mem::take(&mut self.focus_query) {
            resp.request_focus();
            widgets::cursor_to_end(ui.ctx(), qid, &self.query);
        }
        if self.query != before {
            self.edited_at = Some(now);
        }
        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            self.edited_at = None;
            out.send = self.make_query();
        }
        let tw = 22.0;
        let mut x = fr.right() - 4.0 - 3.0 * tw;
        let mut flip = false;
        for (label, on, tip) in [("Aa", &mut self.case, "Match case"), ("W", &mut self.word, "Whole word"), (".*", &mut self.regex, "Regular expression")] {
            let r = Rect::from_min_size(pos2(x, fr.top() + 3.0), vec2(tw - 2.0, fh - 6.0));
            flip |= widgets::text_toggle(ui, r, label, on, tip, t, toggle_id(tab, label));
            x += tw;
        }
        if flip {
            self.edited_at = None;
            out.send = self.make_query();
        }
        y = fr.bottom() + 6.0;

        // Replace row.
        if self.show_replace {
            let bw = 96.0;
            let rr = Rect::from_min_size(pos2(left, y), vec2(fw - bw - 8.0, fh));
            let hint = if self.regex { "Replace   ·   $1, ${name}" } else { "Replace" };
            let rid = replace_id(tab);
            let resp = widgets::field(ui, rr, rid, &mut self.replace, hint, t, t.font_ui());
            if std::mem::take(&mut self.focus_replace) {
                resp.request_focus();
                widgets::cursor_to_end(ui.ctx(), rid, &self.replace);
            }
            let br = Rect::from_min_size(pos2(rr.right() + 8.0, y), vec2(bw, fh));
            let can = !self.hits.is_empty() && !self.running && self.last.is_some();
            if small_button(ui, br, "Replace All", can, t, replace_all_id(tab)) {
                out.replace = Some(None);
            }
            y = rr.bottom() + 6.0;
        }

        // Filters: include | exclude.
        let half = ((fw - 8.0) / 2.0).floor();
        for (i, (hint, which)) in [("files to include, e.g. src/**, *.rs", 0), ("files to exclude, e.g. target/**", 1)].into_iter().enumerate() {
            let r = Rect::from_min_size(pos2(left + i as f32 * (half + 8.0), y), vec2(half, fh));
            let text = if which == 0 { &mut self.include } else { &mut self.exclude };
            let before = text.clone();
            let resp = widgets::field(ui, r, Id::unique(("search-glob", tab, i)), text, hint, t, t.font_small());
            if *text != before {
                self.edited_at = Some(now);
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                self.edited_at = None;
                out.send = self.make_query();
            }
        }
        y += fh + 8.0;
        self.fresh_rows();

        // Status (space always reserved: nothing jumps).
        let sr = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), row_h - 2.0));
        if let Some(e) = &self.error {
            widgets::text_at(ui, sr, left, e, t.font_small(), t.ui.error);
        } else if self.stats.is_some() || self.running || !self.hits.is_empty() {
            widgets::text_at(ui, sr, left, &self.summary(), t.font_small(), t.ui.text_muted);
        } else {
            let hint = if self.query.is_empty() { "Type a pattern, then Enter" } else if self.make_query().is_none() { "At least 2 characters" } else { "Enter to search" };
            widgets::text_at(ui, sr, left, hint, t.font_small(), t.ui.text_faint);
        }
        y = sr.bottom();
        if let Some(n) = &self.note {
            let nr = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), row_h - 2.0));
            widgets::text_at(ui, nr, left, n, t.font_small(), t.ui.warning);
            y = nr.bottom();
        }
        y += 6.0;
        ui.painter().hline(rect.x_range(), y + 0.5, t.border());
        y += 1.0;

        // Results.
        let lr = Rect::from_min_max(pos2(rect.left(), y), rect.max);
        let rid = results_id(tab);
        let bg = ui.interact(lr, rid, Sense::click());
        if bg.clicked() {
            ui.memory_mut(|m| m.request_focus(rid));
        }
        let focused = ui.memory(|m| m.has_focus(rid));
        if focused {
            ui.memory_mut(|m| m.set_focus_lock_filter(rid, egui::EventFilter { vertical_arrows: true, horizontal_arrows: false, tab: false, escape: false }));
            if let Some(o) = self.keyboard(ui) {
                out.open = Some(o);
            }
            widgets::hold_focus(ui);
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lr));
        child.set_clip_rect(lr);
        child.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let digits = self.hits.iter().map(|h| h.line.saturating_add(1)).max().unwrap_or(1).to_string().len().max(3);
        let num_w = (digits as f32) * t.font_mono_ui().size * 0.62 + 4.0;
        let mut clicked: Option<usize> = None;
        let mut toggle: Option<RelPath> = None;
        let mut menu: Option<(usize, Pos2)> = None;
        let reveal = self.reveal.take();
        let show_replace = self.show_replace && !self.running && self.last.is_some();
        let mut replace_file: Option<RelPath> = None;
        let inset = pad;
        widgets::virtual_rows(&mut child, Id::unique(("search-scroll", tab)), self.rows.len(), row_h, reveal, |ui, i, r| {
            let row = self.rows[i];
            let resp = ui.interact(r, rid.with(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            widgets::row_bg(ui.painter(), r, t, resp.hovered(), self.selected == Some(i));
            match row {
                SRow::Header(g) => {
                    let gr = self.groups[g];
                    let path = &self.hits[gr.start].path;
                    let open = !self.collapsed.contains(path);
                    widgets::chevron(ui.painter(), pos2(r.left() + inset + 4.0, r.center().y), open, t.ui.text_muted);
                    let count = thousands(gr.len as u64);
                    let mut right = r.right() - inset;
                    right -= widgets::text_right(ui, r, right, &count, t.font_mono_ui(), t.ui.text_muted) + 10.0;
                    if show_replace {
                        let br = Rect::from_min_max(pos2(right - 64.0, r.top() + 2.0), pos2(right, r.bottom() - 2.0));
                        if small_button(ui, br, "Replace", true, t, replace_file_id(tab, g)) {
                            replace_file = Some(path.clone());
                        }
                        right = br.left() - 8.0;
                    }
                    let dir = path.parent().map(|p| clean(p.as_str())).unwrap_or_default();
                    let name = clean(path.name());
                    widgets::paint_job(ui, r.with_max_x(right), r.left() + inset + 14.0, &[(&name, t.font_ui(), t.ui.text_strong), ("  ", t.font_small(), t.ui.text_muted), (&dir, t.font_small(), t.ui.text_muted)]);
                    if resp.clicked() && replace_file.is_none() {
                        toggle = Some(path.clone());
                    }
                }
                SRow::Hit(h) => {
                    let s = &self.hits[h];
                    let num = (u64::from(s.line) + 1).to_string();
                    let nx = r.left() + inset + 14.0 + num_w;
                    widgets::text_right(ui, r, nx, &num, t.font_mono_ui(), t.ui.text_faint);
                    widgets::snippet(ui, r.with_max_x(r.right() - inset), nx + 12.0, &s.text, s.text_col, s.len, t);
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                    if resp.secondary_clicked() {
                        menu = Some((i, resp.interact_pointer_pos().unwrap_or(r.center())));
                    }
                }
            }
        });
        if let Some(p) = toggle {
            if !self.collapsed.remove(&p) {
                self.collapsed.insert(p);
            }
            self.rows_dirty = true;
            ui.ctx().request_repaint();
        }
        if let Some(i) = clicked {
            self.selected = Some(i);
            ui.memory_mut(|m| m.request_focus(rid));
            out.open = self.selected_open();
        }
        if let Some((i, pos)) = menu {
            self.selected = Some(i);
            if let Some(s) = self.selected_hit() {
                out.menu = Some((s.text.clone(), s.path.clone(), pos));
            }
        }
        if let Some(p) = replace_file {
            out.replace = Some(Some(p));
        }
        out
    }
}

/// Small bordered text button at `r` (explicit id). Disabled: greyed, no click.
pub fn small_button(ui: &mut Ui, r: Rect, label: &str, enabled: bool, t: &Theme, id: Id) -> bool {
    let resp = ui.interact(r, id, if enabled { Sense::click() } else { Sense::hover() });
    let resp = if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
    let p = ui.painter();
    let hov = enabled && resp.hovered();
    p.rect_filled(r, CornerRadius::same(4), if hov { t.ui.row_selected } else { t.ui.row_hover });
    p.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, t.ui.border), egui::StrokeKind::Inside);
    p.text(r.center(), egui::Align2::CENTER_CENTER, label, t.font_small(), if enabled { t.ui.text_strong } else { t.ui.text_faint });
    enabled && resp.clicked()
}

/// `1234567` → `1,234,567`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(p: &str, line: u32) -> Site {
        Site { path: RelPath::new(p).unwrap(), line, col: 0, len: 1, text: "x".into(), text_col: 0, context: String::new() }
    }

    #[test]
    fn results_sorted_and_grouped_as_batches_arrive() {
        let mut s = SearchPanel::default();
        s.started(7, SearchQuery { pattern: "x".into(), ..Default::default() });
        s.on_batch(vec![site("b.rs", 9), site("a.rs", 3)]);
        s.on_batch(vec![site("b.rs", 1), site("a.rs", 1), site("c/d.rs", 0)]);
        let order: Vec<(String, u32)> = s.ordered().iter().map(|h| (h.path.to_string(), h.line)).collect();
        assert_eq!(order, vec![("a.rs".into(), 1), ("a.rs".into(), 3), ("b.rs".into(), 1), ("b.rs".into(), 9), ("c/d.rs".into(), 0)]);
        assert_eq!(s.files(), 3);
        assert_eq!(s.rows.len(), 3 + 5);
        assert_eq!(s.hits_in(&RelPath::new("b.rs").unwrap()), 2);
        s.on_done(SearchStats { files_scanned: 9, files_matched: 3, hits: 5, truncated: true, elapsed_ms: 38 });
        assert_eq!(s.summary(), "5 results in 3 files · 38 ms · first 5 shown");
        assert_eq!(s.title(), "Search: x");
        // A new search keeps the old rows until its first message.
        s.started(8, SearchQuery::default());
        assert_eq!(s.hits.len(), 5);
        s.on_err("regex parse error:\n bad");
        assert!(s.hits.is_empty());
        assert_eq!(s.error.as_deref(), Some("regex parse error:· bad"));
    }

    #[test]
    fn debounce_min_length_and_keys() {
        let mut s = SearchPanel { query: "a".into(), edited_at: Some(0.0), ..Default::default() };
        assert!(s.due(1.0).is_none(), "one char");
        s.query = "ab".into();
        s.edited_at = Some(1.0);
        assert!(s.due(1.1).is_none());
        assert!(s.due(1.2).is_some());
        assert!(s.due(1.3).is_none(), "fires once");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(12), "12");
        s.started(1, SearchQuery::default());
        s.on_batch(vec![site("a", 1), site("a", 2), site("b", 0)]);
        s.fresh_rows();
        s.step(1);
        assert_eq!(s.selected_hit().map(|h| h.line), Some(1));
        s.step(1);
        s.step(1);
        assert_eq!(s.selected_hit().map(|h| (h.path.to_string(), h.line)), Some(("b".into(), 0)), "skips the header");
        s.step(1);
        assert_eq!(s.selected_hit().map(|h| h.line), Some(0), "stops at the end");
    }
}
