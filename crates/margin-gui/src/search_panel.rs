//! Search in folder (sidebar tab). Streams `SearchBatch`…`SearchDone`; results sorted by path then line,
//! grouped by file (collapsible), virtualised.
//!
//! | Input | Does |
//! |---|---|
//! | typing (≥ 2 chars) | search after 150 ms; the previous request is cancelled |
//! | Enter in the query | search now |
//! | `Aa` `W` `.*` | case, whole word, regex |
//! | Up / Down (results focused) | move through hits; Enter opens |

use std::collections::HashSet;

use egui::{pos2, vec2, CursorIcon, Id, Key, Rect, Sense, Ui};
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
    pub query: String,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    pub include: String,
    pub exclude: String,
    pub filters_open: bool,
    pub req: Option<ReqId>,
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
    pub running: bool,
    /// Selected row (index into `rows`).
    selected: Option<usize>,
    pub focus_query: bool,
    reveal: Option<usize>,
    /// Hits dropped because of our own cap.
    capped: bool,
    /// A new request is out: its first message replaces the old results.
    fresh: bool,
}

pub struct OpenHit {
    pub path: RelPath,
    pub line: u32,
    pub col: u32,
    pub len: u32,
}

pub fn query_id() -> Id {
    Id::unique("search-query")
}

fn results_id() -> Id {
    Id::unique("search-results")
}

impl SearchPanel {
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
    pub fn started(&mut self, id: ReqId) {
        self.req = Some(id);
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

    pub fn files(&self) -> usize {
        self.groups.len()
    }

    /// Hits in display order (tests).
    pub fn ordered(&mut self) -> &[Site] {
        if self.rows_dirty || self.unsorted {
            self.rebuild();
        }
        &self.hits
    }

    fn summary(&self) -> String {
        let n = self.hits.len();
        let files = self.groups.len();
        let mut s = format!("{} {} in {} {}", thousands(n as u64), if n == 1 { "result" } else { "results" }, thousands(files as u64), if files == 1 { "file" } else { "files" });
        if let Some(st) = self.stats {
            s += &format!(" · {} ms", st.elapsed_ms);
            if st.truncated || self.capped {
                s += " · limit reached";
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
        let n = self.rows.len();
        if n == 0 {
            return None;
        }
        let step = |from: Option<usize>, d: isize| -> Option<usize> {
            let mut i = from.map_or(if d > 0 { -1 } else { n as isize }, |x| x as isize);
            loop {
                i += d;
                if i < 0 || i >= n as isize {
                    return from;
                }
                if matches!(self.rows[i as usize], SRow::Hit(_)) {
                    return Some(i as usize);
                }
            }
        };
        let mut moved = false;
        if down {
            self.selected = step(self.selected, 1);
            moved = true;
        }
        if up {
            self.selected = step(self.selected, -1);
            moved = true;
        }
        if moved {
            self.reveal = self.selected;
        }
        if moved || enter {
            if let Some(SRow::Hit(h)) = self.selected.map(|s| self.rows[s]) {
                // Moving previews the hit; Enter also lands there.
                let s = &self.hits[h];
                return Some(OpenHit { path: s.path.clone(), line: s.line, col: s.col, len: s.len });
            }
        }
        None
    }

    /// Draw in `rect`. Returns a query to send now (Enter / toggles) and a hit to open.
    pub fn show(&mut self, ui: &mut Ui, rect: Rect, t: &Theme, now: f64) -> (Option<SearchQuery>, Option<OpenHit>) {
        let mut send = None;
        let mut open = None;
        let row_h = t.row_h();
        let pad = 12.0;
        let fh = (row_h + 2.0).round();
        let mut y = rect.top() + 8.0;
        let fr = Rect::from_min_size(pos2(rect.left() + pad, y), vec2(rect.width() - 2.0 * pad, fh));
        let qid = query_id();
        let before = self.query.clone();
        let resp = widgets::field_pad(ui, fr, qid, &mut self.query, "Search", t, t.font_ui(), 74.0);
        if std::mem::take(&mut self.focus_query) {
            resp.request_focus();
            widgets::cursor_to_end(ui.ctx(), qid, &self.query);
        }
        if self.query != before {
            self.edited_at = Some(now);
        }
        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            self.edited_at = None;
            send = self.make_query();
        }
        let tw = 22.0;
        let mut x = fr.right() - 4.0 - 3.0 * tw;
        let mut flip = false;
        for (label, on, tip) in [("Aa", &mut self.case, "Match case"), ("W", &mut self.word, "Whole word"), (".*", &mut self.regex, "Regular expression")] {
            let r = Rect::from_min_size(pos2(x, fr.top() + 3.0), vec2(tw - 2.0, fh - 6.0));
            flip |= widgets::text_toggle(ui, r, label, on, tip, t, Id::unique(("search-toggle", label)));
            x += tw;
        }
        if flip {
            self.edited_at = None;
            send = self.make_query();
        }
        y = fr.bottom() + 4.0;
        // Disclosure: files to include / exclude.
        let dr = Rect::from_min_size(pos2(rect.left() + pad - 4.0, y), vec2(rect.width() - 2.0 * pad + 8.0, row_h - 4.0));
        let d = ui.interact(dr, Id::unique("search-filters"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if d.clicked() {
            self.filters_open = !self.filters_open;
        }
        let col = if d.hovered() { t.ui.text } else { t.ui.text_muted };
        widgets::chevron(ui.painter(), pos2(dr.left() + 8.0, dr.center().y), self.filters_open, col);
        let filt = if self.include.is_empty() && self.exclude.is_empty() { "Files to include / exclude".to_owned() } else { "Files to include / exclude ·  set".to_owned() };
        widgets::text_at(ui, dr, dr.left() + 18.0, &filt, t.font_small(), col);
        y = dr.bottom() + 2.0;
        if self.filters_open {
            for (i, (hint, which)) in [("files to include, e.g. src/**, *.rs", 0), ("files to exclude, e.g. target/**", 1)].into_iter().enumerate() {
                let r = Rect::from_min_size(pos2(rect.left() + pad, y), vec2(rect.width() - 2.0 * pad, fh));
                let text = if which == 0 { &mut self.include } else { &mut self.exclude };
                let before = text.clone();
                let resp = widgets::field(ui, r, Id::unique(("search-glob", i)), text, hint, t, t.font_small());
                if *text != before {
                    self.edited_at = Some(now);
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    self.edited_at = None;
                    send = self.make_query();
                }
                y = r.bottom() + 4.0;
            }
        }
        if self.rows_dirty || self.unsorted {
            self.rebuild();
        }
        // Summary / error line (space always reserved: nothing jumps).
        let sr = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), row_h - 2.0));
        if let Some(e) = &self.error {
            widgets::text_at(ui, sr, sr.left() + pad, e, t.font_small(), t.ui.error);
        } else if self.stats.is_some() || self.running || !self.hits.is_empty() {
            let s = if self.hits.is_empty() && self.stats.is_some() { "No results".to_owned() } else { self.summary() };
            widgets::text_at(ui, sr, sr.left() + pad, &s, t.font_small(), t.ui.text_muted);
        }
        y = sr.bottom() + 2.0;
        let lr = Rect::from_min_max(pos2(rect.left(), y), rect.max);
        let rid = results_id();
        let bg = ui.interact(lr, rid, Sense::click());
        if bg.clicked() {
            ui.memory_mut(|m| m.request_focus(rid));
        }
        let focused = ui.memory(|m| m.has_focus(rid));
        if focused {
            ui.memory_mut(|m| m.set_focus_lock_filter(rid, egui::EventFilter { vertical_arrows: true, horizontal_arrows: false, tab: false, escape: false }));
            open = self.keyboard(ui);
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lr));
        child.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let digits = self.hits.iter().map(|h| h.line + 1).max().unwrap_or(1).to_string().len().max(3);
        let num_w = (digits as f32) * t.font_mono_ui().size * 0.62 + 4.0;
        let mut clicked: Option<usize> = None;
        let mut toggle: Option<RelPath> = None;
        let reveal = self.reveal.take();
        widgets::virtual_rows(&mut child, Id::unique("search-scroll"), self.rows.len(), row_h, reveal, |ui, i, r| {
            let row = self.rows[i];
            let resp = ui.interact(r, rid.with(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            widgets::row_bg(ui.painter(), r, t, resp.hovered(), self.selected == Some(i));
            match row {
                SRow::Header(g) => {
                    let gr = self.groups[g];
                    let path = &self.hits[gr.start].path;
                    let open = !self.collapsed.contains(path);
                    widgets::chevron(ui.painter(), pos2(r.left() + 14.0, r.center().y), open, t.ui.text_muted);
                    let count = thousands(gr.len as u64);
                    let cw = widgets::text_right(ui, r, r.right() - pad, &count, t.font_mono_ui(), t.ui.text_muted);
                    let dir = path.parent().map(|p| clean(p.as_str())).unwrap_or_default();
                    let name = clean(path.name());
                    widgets::paint_job(ui, r.with_max_x(r.right() - pad - cw - 8.0), r.left() + 24.0, &[(&name, t.font_ui(), t.ui.text_strong), ("  ", t.font_small(), t.ui.text_muted), (&dir, t.font_small(), t.ui.text_muted)]);
                    if resp.clicked() {
                        toggle = Some(path.clone());
                    }
                }
                SRow::Hit(h) => {
                    let s = &self.hits[h];
                    let num = (u64::from(s.line) + 1).to_string();
                    let nx = r.left() + 24.0 + num_w;
                    widgets::text_right(ui, r, nx, &num, t.font_mono_ui(), t.ui.text_faint);
                    widgets::snippet(ui, r.with_max_x(r.right() - 8.0), nx + 10.0, &s.text, s.text_col, s.len, t);
                    if resp.clicked() {
                        clicked = Some(i);
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
            if let SRow::Hit(h) = self.rows[i] {
                let s = &self.hits[h];
                open = Some(OpenHit { path: s.path.clone(), line: s.line, col: s.col, len: s.len });
            }
        }
        if self.hits.is_empty() && self.stats.is_none() && !self.running && self.error.is_none() {
            let hint = if self.query.is_empty() { "Type to search the folder" } else if self.make_query().is_none() { "At least 2 characters" } else { "" };
            let r = Rect::from_min_size(pos2(lr.left(), lr.top() + 4.0), vec2(lr.width(), row_h));
            widgets::text_at(ui, r, r.left() + pad, hint, t.font_small(), t.ui.text_faint);
        }
        (send, open)
    }
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
        s.started(7);
        s.on_batch(vec![site("b.rs", 9), site("a.rs", 3)]);
        s.on_batch(vec![site("b.rs", 1), site("a.rs", 1), site("c/d.rs", 0)]);
        let order: Vec<(String, u32)> = s.ordered().iter().map(|h| (h.path.to_string(), h.line)).collect();
        assert_eq!(order, vec![("a.rs".into(), 1), ("a.rs".into(), 3), ("b.rs".into(), 1), ("b.rs".into(), 9), ("c/d.rs".into(), 0)]);
        assert_eq!(s.files(), 3);
        assert_eq!(s.rows.len(), 3 + 5);
        s.on_done(SearchStats { files_scanned: 9, files_matched: 3, hits: 5, truncated: true, elapsed_ms: 38 });
        assert_eq!(s.summary(), "5 results in 3 files · 38 ms · limit reached");
        // A new search keeps the old rows until its first message.
        s.started(8);
        assert_eq!(s.hits.len(), 5);
        s.on_err("regex parse error:\n bad");
        assert!(s.hits.is_empty());
        assert_eq!(s.error.as_deref(), Some("regex parse error:· bad"));
    }

    #[test]
    fn debounce_and_min_length() {
        let mut s = SearchPanel { query: "a".into(), edited_at: Some(0.0), ..Default::default() };
        assert!(s.due(1.0).is_none(), "one char");
        s.query = "ab".into();
        s.edited_at = Some(1.0);
        assert!(s.due(1.1).is_none());
        assert!(s.due(1.2).is_some());
        assert!(s.due(1.3).is_none(), "fires once");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(12), "12");
    }
}
