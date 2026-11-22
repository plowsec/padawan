//! Command palette (cmd+P / ctrl+P / cmd+shift+P) and Go to File (cmd+T): one overlay.
//!
//! | Query | Shows |
//! |---|---|
//! | text | `Actions` (fuzzy on titles) then `Files` (`FindFiles`, 30 ms debounce, old results kept) |
//! | empty | actions, recently used first, then recent files |
//! | `>text` | actions only |
//! | `:42` | go to line 42 of the active tab |
//! | `@name` | functions of the active file (outline), fuzzy |
//!
//! Keys: Up/Down/PageUp/PageDown move, Enter runs, Esc or a click outside closes.

use std::sync::Arc;

use egui::{pos2, vec2, Color32, CursorIcon, FontId, Id, Key, Order, Rect, Sense};
use margin_code::Outline;
use margin_proto::{FileMatch, RelPath, ReqId};
use margin_theme::Theme;

use crate::commands::{self, Cmd};
use crate::sanitize::{floor_char, is_hidden, MARK};
use crate::{fuzzy, widgets};

pub const FILE_DEBOUNCE: f64 = 0.03;
pub const FILE_LIMIT: u32 = 200;
const MAX_FILES: usize = 1000;
const VISIBLE: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    All,
    Files,
}

#[derive(Clone, Debug, PartialEq)]
enum PRow {
    Section(&'static str),
    Action(Cmd, Vec<usize>),
    File(usize),
    Line(u32),
    Symbol(usize, Vec<usize>),
    Note(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaletteOut {
    Run(Cmd),
    Open(RelPath),
    Line(u32),
    /// Function at (line, col) of the active file.
    Symbol(u32, u32),
    Close,
}

pub struct Palette {
    pub mode: Mode,
    pub query: String,
    selected: usize,
    fresh: bool,
    pub files: Vec<FileMatch>,
    pub files_total: u32,
    pub req: Option<ReqId>,
    /// File query whose results are on screen / in flight.
    pub files_for: Option<String>,
    edited_at: f64,
    rows: Vec<PRow>,
    rows_for: Option<(String, usize, bool)>,
    reveal: bool,
}

pub fn input_id() -> Id {
    Id::unique("palette-input")
}

/// Runs of `(text, highlighted)` for `s`, where `s` starts at byte `base` of the matched string.
/// Sanitised; at most `max` chars.
pub fn runs(s: &str, base: usize, positions: &[u32], max: usize) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for (n, (i, c)) in s.char_indices().enumerate() {
        if n + 1 >= max && n + 1 < s.chars().count() {
            out.push(("…".into(), false));
            break;
        }
        let hi = positions.binary_search(&((base + i) as u32)).is_ok();
        let c = if is_hidden(c) { MARK } else { c };
        match out.last_mut() {
            Some((t, h)) if *h == hi => t.push(c),
            _ => out.push((c.to_string(), hi)),
        }
    }
    out
}

impl Palette {
    pub fn new(mode: Mode, seed: &str) -> Palette {
        Palette {
            mode,
            query: seed.to_owned(),
            selected: 0,
            fresh: true,
            files: Vec::new(),
            files_total: 0,
            req: None,
            files_for: None,
            edited_at: -1.0,
            rows: Vec::new(),
            rows_for: None,
            reveal: false,
        }
    }

    /// Text sent to `FindFiles` for the current query, or `None` when files are not shown.
    fn file_query(&self) -> Option<String> {
        let q = self.query.trim();
        if q.starts_with('>') || q.starts_with(':') || q.starts_with('@') {
            return None;
        }
        Some(q.to_owned())
    }

    /// A `FindFiles` query to send now (debounced), if the shown results are for another query.
    pub fn file_due(&mut self, now: f64) -> Option<String> {
        let q = self.file_query()?;
        if self.files_for.as_deref() == Some(q.as_str()) || now - self.edited_at < FILE_DEBOUNCE {
            return None;
        }
        self.files_for = Some(q.clone());
        Some(q)
    }

    pub fn wait_left(&self, now: f64) -> Option<f64> {
        let q = self.file_query()?;
        (self.files_for.as_deref() != Some(q.as_str())).then(|| (FILE_DEBOUNCE - (now - self.edited_at)).max(0.0))
    }

    pub fn set_files(&mut self, mut items: Vec<FileMatch>, total: u32) {
        items.truncate(MAX_FILES);
        for it in &mut items {
            it.positions.sort_unstable();
            it.positions.truncate(4096);
        }
        self.files = items;
        self.files_total = total;
        self.rows_for = None;
    }

    fn rebuild(&mut self, mru: &[Cmd], outline: Option<&Outline>, has_tab: bool) {
        self.rows.clear();
        let q = self.query.trim().to_owned();
        if let Some(rest) = q.strip_prefix(':') {
            match rest.trim().parse::<u32>() {
                Ok(n) if has_tab && n > 0 => self.rows.push(PRow::Line(n)),
                _ if !has_tab => self.rows.push(PRow::Note("Open a file to go to a line".into())),
                _ => self.rows.push(PRow::Note("Type a line number".into())),
            }
            return;
        }
        if let Some(rest) = q.strip_prefix('@') {
            match outline {
                Some(o) if !o.fns.is_empty() => {
                    let mut v: Vec<(i32, usize, Vec<usize>)> = o.fns.iter().enumerate().filter_map(|(i, f)| fuzzy::score(rest, &f.name).map(|(s, p)| (s, i, p))).collect();
                    if !rest.trim().is_empty() {
                        v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                    }
                    v.truncate(2000);
                    self.rows.push(PRow::Section("Functions"));
                    self.rows.extend(v.into_iter().map(|(_, i, p)| PRow::Symbol(i, p)));
                }
                _ => self.rows.push(PRow::Note(if has_tab { "No functions in this file".into() } else { "Open a file to list its functions".into() })),
            }
            return;
        }
        let actions_only = q.starts_with('>');
        let aq = q.trim_start_matches('>').trim();
        if self.mode == Mode::All {
            let mut acts: Vec<(i32, usize, Cmd, Vec<usize>)> = commands::ALL
                .iter()
                .enumerate()
                .filter_map(|(i, d)| fuzzy::score(aq, d.title).map(|(s, p)| (s, i, d.cmd, p)))
                .collect();
            if aq.is_empty() {
                let rank = |c: Cmd| mru.iter().position(|m| *m == c).unwrap_or(usize::MAX);
                acts.sort_by_key(|a| (rank(a.2), a.1));
            } else {
                acts.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                acts.truncate(if actions_only { 100 } else { 8 });
            }
            if !acts.is_empty() {
                self.rows.push(PRow::Section("Actions"));
                self.rows.extend(acts.into_iter().map(|(_, _, c, p)| PRow::Action(c, p)));
            }
        }
        if !actions_only && !self.files.is_empty() {
            self.rows.push(PRow::Section("Files"));
            self.rows.extend((0..self.files.len()).map(PRow::File));
        }
        if self.rows.is_empty() {
            self.rows.push(PRow::Note(if self.req.is_some() { "Searching…".into() } else { "No matches".into() }));
        }
    }

    fn selectable(r: &PRow) -> bool {
        !matches!(r, PRow::Section(_) | PRow::Note(_))
    }

    fn fix_selection(&mut self, dir: isize) {
        let n = self.rows.len();
        if n == 0 {
            self.selected = 0;
            return;
        }
        let mut i = self.selected.min(n - 1) as isize;
        while i >= 0 && (i as usize) < n && !Self::selectable(&self.rows[i as usize]) {
            i += if dir == 0 { 1 } else { dir };
        }
        if i < 0 || i as usize >= n {
            i = self.rows.iter().position(Self::selectable).unwrap_or(0) as isize;
        }
        self.selected = i as usize;
    }

    /// Selected row, for tests: `"action:<title>"`, `"file:<path>"`, `"line:<n>"`, `"symbol:<i>"`.
    pub fn selected_desc(&self) -> Option<String> {
        Some(match self.rows.get(self.selected)? {
            PRow::Action(c, _) => format!("action:{}", commands::def(*c).title),
            PRow::File(f) => format!("file:{}", self.files.get(*f)?.path),
            PRow::Line(n) => format!("line:{n}"),
            PRow::Symbol(s, _) => format!("symbol:{s}"),
            _ => return None,
        })
    }

    fn pick(&self, i: usize, outline: Option<&Outline>) -> Option<PaletteOut> {
        Some(match self.rows.get(i)? {
            PRow::Action(c, _) => PaletteOut::Run(*c),
            PRow::File(f) => PaletteOut::Open(self.files.get(*f)?.path.clone()),
            PRow::Line(n) => PaletteOut::Line(n - 1),
            PRow::Symbol(s, _) => {
                let f = outline?.fns.get(*s)?;
                PaletteOut::Symbol(f.line, f.col)
            }
            _ => return None,
        })
    }

    /// Draw the overlay. `screen`: content rect.
    #[allow(clippy::too_many_arguments)]
    pub fn show(&mut self, ctx: &egui::Context, t: &Theme, mac: bool, mru: &[Cmd], outline: Option<Arc<Outline>>, has_tab: bool, now: f64) -> Option<PaletteOut> {
        let screen = ctx.content_rect();
        let mut out = None;
        // Scrim.
        let scrim = egui::Area::new(Id::unique("palette-scrim")).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            let r = ui.interact(screen, Id::unique("palette-scrim-hit"), Sense::click());
            ui.painter().rect_filled(screen, 0.0, t.ui.scrim);
            r
        });
        if scrim.inner.clicked() {
            out = Some(PaletteOut::Close);
        }
        let key = (self.query.clone(), self.files.len(), self.req.is_some());
        if self.rows_for.as_ref() != Some(&key) {
            let changed = self.rows_for.as_ref().is_none_or(|k| k.0 != key.0);
            self.rebuild(mru, outline.as_deref(), has_tab);
            self.rows_for = Some(key);
            if changed {
                self.selected = 0;
            }
            self.fix_selection(0);
        }
        // Keys before the text field sees them.
        let (up, down, pgup, pgdn, enter, esc) = ctx.input_mut(|i| {
            let m = egui::Modifiers::NONE;
            (i.consume_key(m, Key::ArrowUp), i.consume_key(m, Key::ArrowDown), i.consume_key(m, Key::PageUp), i.consume_key(m, Key::PageDown), i.consume_key(m, Key::Enter), i.consume_key(m, Key::Escape))
        });
        let n = self.rows.len();
        let mv = |s: &mut Self, d: isize| {
            if n == 0 {
                return;
            }
            let target = (s.selected as isize + d).clamp(0, n as isize - 1);
            s.selected = target as usize;
            s.fix_selection(if d < 0 { -1 } else { 1 });
            if !Self::selectable(&s.rows[s.selected]) {
                s.fix_selection(if d < 0 { 1 } else { -1 });
            }
            s.reveal = true;
        };
        if down {
            mv(self, 1);
        }
        if up {
            mv(self, -1);
        }
        if pgdn {
            mv(self, VISIBLE as isize);
        }
        if pgup {
            mv(self, -(VISIBLE as isize));
        }
        if esc {
            out = Some(PaletteOut::Close);
        }
        if enter {
            if let Some(o) = self.pick(self.selected, outline.as_deref()) {
                out = Some(o);
            }
        }

        let row_h = t.row_h();
        let w = 620.0f32.min(screen.width() - 32.0);
        let in_h = (row_h + 18.0).round();
        let list_rows = n.clamp(1, VISIBLE);
        let h = in_h + 1.0 + list_rows as f32 * row_h + 8.0;
        let pos = pos2((screen.center().x - w / 2.0).round(), (screen.top() + (screen.height() * 0.12).clamp(24.0, 96.0)).round());
        let rect = Rect::from_min_size(pos, vec2(w, h));
        egui::Area::new(Id::unique("palette")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            let p = ui.painter().clone();
            p.add(ctx.global_style().visuals.window_shadow.as_shape(rect, 6));
            p.rect_filled(rect, 6.0, t.ui.overlay_bg);
            p.rect_stroke(rect, 6.0, t.border(), egui::StrokeKind::Inside);
            // Swallow clicks on the panel so they don't reach the scrim.
            ui.interact(rect, Id::unique("palette-body"), Sense::click());
            let ir = Rect::from_min_size(rect.min + vec2(16.0, 6.0), vec2(w - 32.0, in_h - 12.0));
            let hint = match self.mode {
                Mode::All => "Type a command or a file name   ·   >actions   :line   @function",
                Mode::Files => "Go to file",
            };
            let before = self.query.clone();
            let resp = widgets::field_bare(ui, ir, input_id(), &mut self.query, hint, t, egui::FontId::proportional(t.look.ui_size + 1.0));
            if std::mem::take(&mut self.fresh) {
                widgets::cursor_to_end(ctx, input_id(), &self.query);
            }
            if !resp.has_focus() {
                resp.request_focus();
            }
            if self.query != before {
                self.edited_at = now;
            }
            let lr = Rect::from_min_max(pos2(rect.left(), rect.top() + in_h + 1.0), pos2(rect.right(), rect.bottom() - 4.0));
            p.hline(rect.x_range(), rect.top() + in_h + 0.5, t.border());
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lr));
            child.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let reveal = std::mem::take(&mut self.reveal).then_some(self.selected);
            let mut clicked = None;
            widgets::virtual_rows(&mut child, Id::unique("palette-rows"), n, row_h, reveal, |ui, i, r| {
                let row = &self.rows[i];
                let r = r.shrink2(vec2(4.0, 0.0));
                if !Self::selectable(row) {
                    let label = match row {
                        PRow::Section(s) => s.to_string(),
                        PRow::Note(s) => s.clone(),
                        _ => String::new(),
                    };
                    widgets::text_at(ui, r, r.left() + 8.0, &label, t.font_small(), if matches!(row, PRow::Note(_)) { t.ui.text_faint } else { t.ui.text_muted });
                    return;
                }
                let resp = ui.interact(r, Id::unique(("palette-row", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                let sel = i == self.selected;
                if sel {
                    ui.painter().rect_filled(r, 4.0, t.ui.row_selected);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, t.ui.row_hover);
                }
                if resp.clicked() {
                    clicked = Some(i);
                }
                let x = r.left() + 8.0;
                let hi = |b: bool, base: Color32| if b { t.ui.accent } else { base };
                match row {
                    PRow::Action(c, pos) => {
                        let title = commands::def(*c).title;
                        let k = commands::shortcut(*c, mac);
                        let kw = if k.is_empty() { 0.0 } else { widgets::key_hint(ui, r, r.right() - 8.0, &k, t) + 12.0 };
                        let pos: Vec<u32> = pos.iter().map(|&p| p as u32).collect();
                        let rs = runs(title, 0, &pos, 200);
                        let base = if sel { t.ui.text_strong } else { t.ui.text };
                        let parts: Vec<(&str, FontId, Color32)> = rs.iter().map(|(s, h)| (s.as_str(), t.font_ui(), hi(*h, base))).collect();
                        widgets::paint_job(ui, r.with_max_x(r.right() - kw - 8.0), x, &parts);
                    }
                    PRow::File(f) => {
                        let Some(m) = self.files.get(*f) else { return };
                        let path = m.path.as_str();
                        let name = m.path.name();
                        let nb = floor_char(path, path.len() - name.len());
                        let dir = path[..nb].trim_end_matches('/');
                        let mut parts_s = runs(name, nb, &m.positions, 120);
                        let name_n = parts_s.len();
                        if !dir.is_empty() {
                            parts_s.push(("   ".into(), false));
                            parts_s.extend(runs(dir, 0, &m.positions, 200));
                        }
                        let parts: Vec<(&str, FontId, Color32)> = parts_s
                            .iter()
                            .enumerate()
                            .map(|(k, (s, h))| if k < name_n { (s.as_str(), t.font_ui(), hi(*h, t.ui.text_strong)) } else { (s.as_str(), t.font_small(), hi(*h, t.ui.text_muted)) })
                            .collect();
                        widgets::paint_job(ui, r.with_max_x(r.right() - 8.0), x, &parts);
                    }
                    PRow::Line(n) => {
                        widgets::text_at(ui, r, x, &format!("Go to line {n}"), t.font_ui(), t.ui.text_strong);
                    }
                    PRow::Symbol(s, pos) => {
                        let Some(f) = outline.as_ref().and_then(|o| o.fns.get(*s)) else { return };
                        let pos: Vec<u32> = pos.iter().map(|&p| p as u32).collect();
                        let rs = runs(&f.name, 0, &pos, 120);
                        let mut parts: Vec<(&str, FontId, Color32)> = rs.iter().map(|(s, h)| (s.as_str(), t.font_mono_ui(), hi(*h, t.ui.text_strong))).collect();
                        let ln = format!("   {}", f.line + 1);
                        parts.push((&ln, t.font_small(), t.ui.text_faint));
                        widgets::paint_job(ui, r.with_max_x(r.right() - 8.0), x, &parts);
                    }
                    _ => {}
                }
            });
            if let Some(i) = clicked {
                out = self.pick(i, outline.as_deref()).or(out.clone());
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(p: &str) -> FileMatch {
        FileMatch { path: RelPath::new(p).unwrap(), score: 1, positions: vec![0] }
    }

    #[test]
    fn sections_prefixes_and_selection() {
        let mut p = Palette::new(Mode::All, "save");
        p.set_files(vec![fm("save.rs")], 1);
        p.rebuild(&[], None, true);
        assert_eq!(p.rows[0], PRow::Section("Actions"));
        assert!(matches!(p.rows[1], PRow::Action(Cmd::Save, _)));
        assert!(p.rows.contains(&PRow::Section("Files")));
        p.fix_selection(0);
        assert_eq!(p.selected, 1);
        assert_eq!(p.pick(1, None), Some(PaletteOut::Run(Cmd::Save)));

        p.query = ">save".into();
        p.rebuild(&[], None, true);
        assert!(!p.rows.iter().any(|r| matches!(r, PRow::File(_))));
        assert!(p.file_query().is_none());

        p.query = ":12".into();
        p.rebuild(&[], None, true);
        assert_eq!(p.pick(0, None), Some(PaletteOut::Line(11)));

        let mut f = Palette::new(Mode::Files, "");
        f.set_files(vec![fm("a.rs")], 1);
        f.rebuild(&[], None, true);
        assert_eq!(f.rows, vec![PRow::Section("Files"), PRow::File(0)]);

        // MRU first for an empty query.
        let mut e = Palette::new(Mode::All, "");
        e.rebuild(&[Cmd::ToggleTheme], None, false);
        assert!(matches!(e.rows[1], PRow::Action(Cmd::ToggleTheme, _)));
    }

    #[test]
    fn file_debounce_keeps_old_results() {
        let mut p = Palette::new(Mode::All, "");
        assert_eq!(p.file_due(0.0), Some(String::new()));
        p.set_files(vec![fm("x")], 1);
        p.query = "ab".into();
        p.edited_at = 1.0;
        assert_eq!(p.file_due(1.01), None);
        assert_eq!(p.files.len(), 1);
        assert_eq!(p.file_due(1.05), Some("ab".into()));
        assert_eq!(p.file_due(1.1), None);
    }

    #[test]
    fn runs_highlight_and_sanitise() {
        let r = runs("a\u{202e}bc", 0, &[0, 5], 50);
        assert_eq!(r, vec![("a".into(), true), ("·b".into(), false), ("c".into(), true)]);
        // Positions inside a multi-byte char never panic.
        let _ = runs("é中", 0, &[1, 2, 3, 99], 50);
    }
}
