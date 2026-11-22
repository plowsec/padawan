//! Find bar: state, per-frame step (counter scan, pending search), and the floating bar UI (top-right).
//!
//! | Key (in the bar) | Action |
//! |---|---|
//! | Enter / Shift+Enter | next / previous |
//! | Enter in the replace field | replace the current match, go to the next |
//! | Esc | close, focus back to the text |
//!
//! Typing in the query selects the first match at/after the cursor. The counter shows `…` while scanning.

use std::time::Duration;

use egui::text::{CCursor, CCursorRange};
use egui::{Align2, CornerRadius, Key, Pos2, Rect, Response, Sense, Shadow, Stroke, TextEdit, Vec2};
use margin_theme::Theme;

use crate::buffer::order;
use crate::find::{replace_all, search, Dir, Hit, Query, Resume, Scan};
use crate::undo::{Kind, Sel};
use crate::view::{Cx, Reveal, View, FIND_BUDGET};
use crate::{Document, EditorOutput, Pos};

/// Budget for a search started by the shell (`find_next` outside `show`).
const NAV_BUDGET: Duration = Duration::from_millis(30);

#[derive(Default)]
pub struct FindUi {
    pub open: bool,
    pub replace_open: bool,
    pub text: String,
    pub replace: String,
    pub case: bool,
    pub word: bool,
    pub scan: Scan,
    /// Current match (selected in the text).
    pub current: Option<(Pos, Pos)>,
    pending: Option<Resume>,
    focus_query: bool,
    last: Query,
}

impl FindUi {
    pub fn query(&self) -> Query {
        Query { text: self.text.clone(), case: self.case, word: self.word }
    }

    pub fn open(&mut self, seed: Option<&str>, replace: bool) {
        if let Some(s) = seed.filter(|s| !s.is_empty() && !s.contains('\n') && s.len() <= 1000) {
            self.text = s.to_owned();
        }
        self.open = true;
        self.replace_open = replace;
        self.focus_query = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.pending = None;
        self.current = None;
        self.scan.reset();
    }

    pub fn busy(&self) -> bool {
        self.open && (!self.scan.done || self.pending.is_some())
    }

    pub fn counter(&self) -> String {
        if self.text.is_empty() {
            return String::new();
        }
        let n = self.scan.matches.len();
        let idx = self.current.and_then(|c| self.scan.index_of(c.0));
        if !self.scan.done {
            return match idx {
                Some(i) => format!("{i} of {n}…"),
                None if n == 0 => "…".to_owned(),
                None => format!("{n}…"),
            };
        }
        if n == 0 {
            return "No results".to_owned();
        }
        let total = if self.scan.capped { format!("{n}+") } else { n.to_string() };
        match idx {
            Some(i) => format!("{i} of {total}"),
            None => format!("{total} results"),
        }
    }
}

/// Run a search and apply its result to the view.
fn run(doc: &Document, v: &mut View, r: Resume, budget: Duration) {
    let q = v.find.query();
    match search(doc, &q, r, budget) {
        Hit::Found(a, b) => {
            v.find.pending = None;
            v.find.current = Some((a, b));
            v.sel = Sel { anchor: a, head: b };
            v.up = false;
            v.pref_x = None;
            v.reveal = Reveal::Center;
        }
        Hit::None => {
            v.find.pending = None;
            v.find.current = None;
        }
        Hit::Pending(next) => v.find.pending = Some(next),
    }
}

/// Next / previous match from the selection (shell or bar).
pub fn nav(doc: &Document, v: &mut View, dir: Dir) {
    v.clamp_to(doc);
    let (a, b) = order(v.sel.anchor, v.sel.head);
    let from = match dir {
        Dir::Next => b,
        Dir::Prev => a,
    };
    v.find.last = v.find.query();
    run(doc, v, Resume { from, dir, done: 0 }, NAV_BUDGET);
}

/// Per frame (inside `show`, after input).
pub fn step(cx: &mut Cx<'_, '_>) {
    let doc: &Document = cx.doc;
    let v = &mut *cx.v;
    if !v.find.open {
        return;
    }
    let q = v.find.query();
    if q != v.find.last {
        v.find.last = q.clone();
        v.find.pending = None;
        if q.text.is_empty() {
            v.find.current = None;
        } else {
            let (a, _) = order(v.sel.anchor, v.sel.head);
            run(doc, v, Resume { from: a, dir: Dir::Next, done: 0 }, FIND_BUDGET);
        }
    } else if let Some(r) = v.find.pending {
        run(doc, v, r, FIND_BUDGET);
    }
    // The current match is only current while it is the selection.
    if let Some(c) = v.find.current {
        if order(v.sel.anchor, v.sel.head) != c {
            v.find.current = None;
        }
    }
    v.find.scan.step(doc, &q, FIND_BUDGET);
}

fn replace_one(doc: &mut Document, v: &mut View, time: f64) {
    let q = v.find.query();
    let sel = order(v.sel.anchor, v.sel.head);
    let is_match = v.find.current == Some(sel) || {
        let m = crate::find::Matcher::new(&q);
        let mut fold = Default::default();
        sel.0 .0 == sel.1 .0 && m.is_some_and(|m| m.line_matches(doc.line(sel.0 .0), &mut fold).contains(&(sel.0 .1, sel.1 .1)))
    };
    if is_match {
        let with = v.find.replace.replace("\r\n", "\n");
        if let Some(s) = doc.group(Kind::Other, time, v.sel, |tx| Sel::caret(tx.edit(sel.0, sel.1, &with))) {
            v.sel = s;
        }
    }
    nav(doc, v, Dir::Next);
}

#[derive(Clone, Copy)]
enum Glyph {
    Text(&'static str),
    Up,
    Down,
    Close,
    Chevron(bool),
}

fn button(ui: &mut egui::Ui, th: &Theme, r: Rect, id: egui::Id, g: Glyph, on: bool, tip: &str) -> Response {
    let resp = ui.interact(r, id, Sense::click()).on_hover_text(tip);
    let u = &th.ui;
    let p = ui.painter();
    if on {
        p.rect_filled(r, CornerRadius::same(3), u.row_selected);
    } else if resp.hovered() {
        p.rect_filled(r, CornerRadius::same(3), u.row_hover);
    }
    let col = if on { u.text_strong } else if resp.hovered() { u.text } else { u.text_muted };
    let s = Stroke::new(1.2, col);
    let c = r.center();
    let k = (r.height() * 0.18).round().max(3.0);
    match g {
        Glyph::Text(t) => {
            p.text(c, Align2::CENTER_CENTER, t, th.font_small(), col);
        }
        Glyph::Up => {
            p.line(vec![Pos2::new(c.x - k, c.y + k * 0.5), Pos2::new(c.x, c.y - k * 0.5), Pos2::new(c.x + k, c.y + k * 0.5)], s);
        }
        Glyph::Down => {
            p.line(vec![Pos2::new(c.x - k, c.y - k * 0.5), Pos2::new(c.x, c.y + k * 0.5), Pos2::new(c.x + k, c.y - k * 0.5)], s);
        }
        Glyph::Chevron(open) => {
            if open {
                p.line(vec![Pos2::new(c.x - k * 0.7, c.y - k * 0.35), Pos2::new(c.x, c.y + k * 0.35), Pos2::new(c.x + k * 0.7, c.y - k * 0.35)], s);
            } else {
                p.line(vec![Pos2::new(c.x - k * 0.35, c.y - k * 0.7), Pos2::new(c.x + k * 0.35, c.y), Pos2::new(c.x - k * 0.35, c.y + k * 0.7)], s);
            }
        }
        Glyph::Close => {
            let k = k * 0.8;
            p.line_segment([Pos2::new(c.x - k, c.y - k), Pos2::new(c.x + k, c.y + k)], s);
            p.line_segment([Pos2::new(c.x - k, c.y + k), Pos2::new(c.x + k, c.y - k)], s);
        }
    }
    resp
}

fn field(ui: &mut egui::Ui, th: &Theme, r: Rect, id: egui::Id, text: &mut String, hint: &str) -> Response {
    let focused = ui.memory(|m| m.has_focus(id));
    ui.painter().rect_filled(r, CornerRadius::same(3), th.ui.input_bg);
    ui.painter().rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, if focused { th.ui.accent } else { th.ui.border }), egui::StrokeKind::Inside);
    let font_h = ui.fonts_mut(|f| f.row_height(&th.font_ui()));
    let te = TextEdit::singleline(text)
        .id(id)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::symmetric(6, 0))
        .font(th.font_ui())
        .text_color(th.ui.text_strong)
        .hint_text(hint)
        .return_key(None)
        .desired_width(r.width() - 12.0);
    // Text block centred in the field (TextEdit itself lays out from the top).
    let inner = Rect::from_center_size(r.center(), Vec2::new(r.width(), font_h));
    ui.put(inner, te)
}

/// Draw the bar (when open) and handle its widgets.
pub fn ui(ui: &mut egui::Ui, doc: &mut Document, v: &mut View, th: &Theme, area: Rect, _out: &mut EditorOutput, id: egui::Id) {
    if !v.find.open {
        return;
    }
    let time = ui.input(|i| i.time);
    let rh = th.row_h().max(18.0);
    let pad = 5.0;
    let btn = rh - 4.0;
    let w = (area.width() - 32.0).clamp(160.0, 470.0);
    let rows = if v.find.replace_open { 2.0 } else { 1.0 };
    let h = pad * 2.0 + rows * rh + (rows - 1.0) * 4.0;
    let bar = Rect::from_min_size(Pos2::new((area.right() - 18.0 - w).round(), area.top() + 6.0), Vec2::new(w, h));
    let p = ui.painter();
    let shadow = Shadow { offset: [0, 2], blur: 8, spread: 0, color: egui::Color32::from_black_alpha(if th.mode == margin_theme::Mode::Dark { 70 } else { 22 }) };
    p.add(shadow.as_shape(bar, CornerRadius::same(5)));
    p.rect_filled(bar, CornerRadius::same(5), th.ui.overlay_bg);
    p.rect_stroke(bar, CornerRadius::same(5), Stroke::new(1.0, th.ui.border), egui::StrokeKind::Inside);
    // Swallow clicks on the bar background.
    let _ = ui.interact(bar, id.with("findbar-bg"), Sense::click());

    let fid = id.with("find-query");
    let rid = id.with("find-replace");
    let mut x = bar.left() + pad;
    let y0 = bar.top() + pad;
    let row1 = |x0: f32, x1: f32| Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y0 + rh));
    let center_btn = |x: f32, y: f32| Rect::from_min_size(Pos2::new(x, y + (rh - btn) * 0.5), Vec2::new(btn, btn));

    let chev = button(ui, th, center_btn(x, y0), id.with("find-chev"), Glyph::Chevron(v.find.replace_open), false, "Toggle replace");
    if chev.clicked() {
        v.find.replace_open = !v.find.replace_open;
    }
    x += btn + 3.0;
    let right_w = 4.0 * (btn + 2.0) + btn + 6.0;
    let counter_w = 64.0;
    let field_r = row1(x, bar.right() - pad - right_w - counter_w);

    if std::mem::take(&mut v.find.focus_query) {
        ui.memory_mut(|m| m.request_focus(fid));
        let mut st = TextEdit::load_state(ui.ctx(), fid).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(v.find.text.chars().count()))));
        TextEdit::store_state(ui.ctx(), fid, st);
    }
    let qr = field(ui, th, field_r, fid, &mut v.find.text, "Find");
    let (enter, shift, esc) = ui.input(|i| {
        let m = i.events.iter().find_map(|e| match e {
            egui::Event::Key { key: Key::Enter, pressed: true, modifiers, .. } => Some(*modifiers),
            _ => None,
        });
        (m.is_some(), m.is_some_and(|m| m.shift), i.key_pressed(Key::Escape))
    });
    let in_query = qr.has_focus() || qr.lost_focus();
    if in_query && enter {
        ui.input_mut(|i| i.consume_key(i.modifiers, Key::Enter));
        nav(doc, v, if shift { Dir::Prev } else { Dir::Next });
        ui.memory_mut(|m| m.request_focus(fid));
    }
    let counter = v.find.counter();
    let cr = Rect::from_min_max(Pos2::new(field_r.right() + 6.0, y0), Pos2::new(field_r.right() + 6.0 + counter_w, y0 + rh));
    let ccol = if counter == "No results" { th.ui.error } else { th.ui.text_muted };
    ui.painter().text(Pos2::new(cr.left(), cr.center().y), Align2::LEFT_CENTER, counter, th.font_small(), ccol);

    let mut bx = cr.right();
    if button(ui, th, center_btn(bx, y0), id.with("find-case"), Glyph::Text("Aa"), v.find.case, "Match case").clicked() {
        v.find.case = !v.find.case;
    }
    bx += btn + 2.0;
    if button(ui, th, center_btn(bx, y0), id.with("find-word"), Glyph::Text("W"), v.find.word, "Whole word").clicked() {
        v.find.word = !v.find.word;
    }
    bx += btn + 4.0;
    if button(ui, th, center_btn(bx, y0), id.with("find-prev"), Glyph::Up, false, "Previous (Shift+Enter)").clicked() {
        nav(doc, v, Dir::Prev);
    }
    bx += btn + 2.0;
    if button(ui, th, center_btn(bx, y0), id.with("find-next"), Glyph::Down, false, "Next (Enter)").clicked() {
        nav(doc, v, Dir::Next);
    }
    bx += btn + 2.0;
    let close = button(ui, th, center_btn(bx, y0), id.with("find-close"), Glyph::Close, false, "Close (Esc)").clicked();

    let mut in_replace = false;
    if v.find.replace_open {
        let y1 = y0 + rh + 4.0;
        let fr = Rect::from_min_max(Pos2::new(field_r.left(), y1), Pos2::new(field_r.right(), y1 + rh));
        let rr = field(ui, th, fr, rid, &mut v.find.replace, "Replace");
        in_replace = rr.has_focus() || rr.lost_focus();
        if in_replace && enter {
            ui.input_mut(|i| i.consume_key(i.modifiers, Key::Enter));
            replace_one(doc, v, time);
            ui.memory_mut(|m| m.request_focus(rid));
        }
        let font = th.font_small();
        let tw = |s: &str| ui.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font.clone(), th.ui.text).size().x) + 14.0;
        let w1 = tw("Replace");
        let w2 = tw("All");
        let b1 = Rect::from_min_size(Pos2::new(fr.right() + 6.0, y1 + (rh - btn) * 0.5), Vec2::new(w1, btn));
        let b2 = Rect::from_min_size(Pos2::new(b1.right() + 4.0, b1.top()), Vec2::new(w2, btn));
        if button(ui, th, b1, id.with("find-rep1"), Glyph::Text("Replace"), false, "Replace (Enter)").clicked() {
            replace_one(doc, v, time);
        }
        if button(ui, th, b2, id.with("find-repall"), Glyph::Text("All"), false, "Replace all").clicked() {
            let q = v.find.query();
            let with = v.find.replace.clone();
            if replace_all(doc, &q, &with, v.sel, time) > 0 {
                v.sel = Sel::caret(doc.clamp(v.sel.head));
                v.find.current = None;
            }
        }
    }
    if close || (esc && (in_query || in_replace)) {
        v.find.close();
        v.focus_text = true;
        ui.ctx().request_repaint();
    }
}
