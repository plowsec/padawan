//! Per-view state and the frame: geometry, scrolling, reveal, visible rows, orchestration of `show`.
//!
//! Scroll anchor: `(top_line, top_off)`; `top_off` = points from the first text row of `top_line` to the
//! viewport top (negative inside its hint row). Hint rows appearing/disappearing keep the text still.
//! Max scroll: the last line can rise to the middle of the view. The scrollbar is line-based (no total height).
//!
//! Frame order: metrics → hit-test views → mouse → keys → find → reveal → lex (budget) → views → paint → find bar.

use std::time::Duration;

use egui::{Pos2, Rect, Sense, Vec2};
use margin_code::Token;
use margin_theme::Theme;

use crate::findbar::FindUi;
use crate::layout::{Lay, LineView};
use crate::undo::Sel;
use crate::{Document, EditorOptions, EditorOutput, Lens, Pos, Wrap};

pub const LEX_BUDGET: Duration = Duration::from_millis(3);
pub const FIND_BUDGET: Duration = Duration::from_millis(3);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reveal {
    #[default]
    None,
    /// Keep the cursor visible with minimal scrolling.
    Near,
    /// Centre the cursor if it is off screen.
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragMode {
    Char,
    Word,
    Line,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Click {
    pub time: f64,
    pub pos: Pos2,
    pub count: u32,
    pub drag: Option<DragMode>,
    /// Selection made by the press (word / line), kept while dragging.
    pub origin: (Pos, Pos),
}

#[derive(Clone, Copy, Debug)]
pub struct Geo {
    pub rect: Rect,
    pub text: Rect,
    pub view_h: f32,
    pub char_w: f32,
}

impl Default for Geo {
    fn default() -> Self {
        let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        Geo { rect: r, text: r, view_h: 600.0, char_w: 8.0 }
    }
}

#[derive(Default)]
pub struct View {
    pub sel: Sel,
    /// Caret at a wrap boundary is drawn at the end of the upper row.
    pub up: bool,
    pub pref_x: Option<f32>,
    pub top_line: u32,
    pub top_off: f32,
    pub scroll_x: f32,
    pub widest: f32,
    pub reveal: Reveal,
    pub find: FindUi,
    pub click: Click,
    pub preedit: Option<String>,
    pub geo: Geo,
    pub focus_text: bool,
    pub sb_drag: Option<f32>,
    pub hsb_drag: Option<f32>,
    pub lens_hover: Option<(u32, usize)>,
    pub had_focus: bool,
    /// Identifier under the pointer with Cmd/Ctrl held: `(line, start, end)`.
    pub ctrl_word: Option<(u32, u32, u32)>,
    /// Caret rect drawn last frame (screen), when visible.
    pub caret: Option<Rect>,
}

/// A laid-out visible line: `y` = screen y of its first text row.
pub struct Vis {
    pub view: LineView,
    pub y: f32,
}

/// Everything input handlers need this frame.
pub struct Cx<'a, 'b> {
    pub doc: &'a mut Document,
    pub v: &'a mut View,
    pub lay: &'a Lay<'b>,
    pub opts: &'a EditorOptions,
    pub out: &'a mut EditorOutput,
    pub time: f64,
    pub mac: bool,
    pub toks: Vec<Token>,
    pub copy: Option<String>,
    pub dollar: bool,
}

pub fn snap(v: f32, ppp: f32) -> f32 {
    (v * ppp).round() / ppp
}

impl View {
    pub fn clamp_to(&mut self, doc: &Document) {
        self.sel.anchor = doc.clamp(self.sel.anchor);
        self.sel.head = doc.clamp(self.sel.head);
        let n = doc.line_count();
        if self.top_line >= n {
            self.top_line = n.saturating_sub(1);
            self.top_off = 0.0;
        }
        if !self.top_off.is_finite() {
            self.top_off = 0.0;
        }
        if !self.scroll_x.is_finite() || self.scroll_x < 0.0 {
            self.scroll_x = 0.0;
        }
    }

    pub fn set_sel(&mut self, sel: Sel) {
        self.sel = sel;
        self.up = false;
        self.pref_x = None;
        self.reveal = self.reveal.max_near();
    }

    /// Scroll by `dy` points (positive: view moves down the document).
    pub fn scroll_by(&mut self, lay: &Lay<'_>, doc: &Document, dy: f32, toks: &mut Vec<Token>) {
        if !dy.is_finite() {
            return;
        }
        let n = doc.line_count();
        let mut line = self.top_line.min(n.saturating_sub(1));
        let lens = |l: u32| if lay.has_lens(l) { lay.lens_h } else { 0.0 };
        let mut d = self.top_off + lens(line) + dy;
        // Long jumps without wrap: block = row (+ lens), so skip whole runs exactly.
        if !lay.wrap {
            loop {
                let k = ((d / (lay.row_h + lay.lens_h)).floor().max(0.0) as u32).min(n.saturating_sub(1) - line);
                if k < 8 {
                    break;
                }
                d -= k as f32 * lay.row_h + lenses_in(lay, line, line + k) as f32 * lay.lens_h;
                line += k;
            }
        }
        loop {
            let h = lay.block_h(doc, line, toks);
            if d >= h && line + 1 < n {
                d -= h;
                line += 1;
            } else {
                break;
            }
        }
        while d < 0.0 {
            if line == 0 {
                d = 0.0;
                break;
            }
            line -= 1;
            d += lay.block_h(doc, line, toks);
        }
        self.top_line = line;
        self.top_off = d - lens(line);
        self.clamp_end(lay, doc, toks);
    }

    /// Keep the scroll position at or above the max (last line in the middle of the view).
    pub fn clamp_end(&mut self, lay: &Lay<'_>, doc: &Document, toks: &mut Vec<Token>) {
        let n = doc.line_count();
        let view_h = self.geo.view_h.max(lay.row_h);
        let rows_in_view = (view_h / lay.row_h).ceil() as u32 + 2;
        if self.top_line.saturating_add(rows_in_view) < n {
            return;
        }
        let lens = |l: u32| if lay.has_lens(l) { lay.lens_h } else { 0.0 };
        let last = n.saturating_sub(1);
        let mut line = last;
        let mut d = lay.block_h(doc, last, toks) - view_h * 0.5;
        while d < 0.0 && line > 0 {
            line -= 1;
            d += lay.block_h(doc, line, toks);
        }
        let d = d.max(0.0);
        let cur = self.top_off + lens(self.top_line);
        if (self.top_line, cur) > (line, d) {
            self.top_line = line;
            self.top_off = d - lens(line);
        }
    }

    /// Screen-relative y (from the viewport top) of the first text row of `line`, if within `limit` lines.
    pub fn y_of_line(&self, lay: &Lay<'_>, doc: &Document, line: u32, limit: u32, toks: &mut Vec<Token>) -> Option<f32> {
        let lens = |l: u32| if lay.has_lens(l) { lay.lens_h } else { 0.0 };
        let top = self.top_line;
        if line >= top {
            if line - top > limit {
                return None;
            }
            let mut y = -self.top_off;
            for l in top..line {
                y += lay.text_h(doc, l, toks) + lens(l + 1);
            }
            Some(y)
        } else {
            if top - line > limit {
                return None;
            }
            let mut y = -self.top_off;
            for l in (line..top).rev() {
                y -= lens(l + 1) + lay.text_h(doc, l, toks);
            }
            Some(y)
        }
    }

    /// Scroll so the cursor is visible (vertical and, without wrap, horizontal).
    pub fn do_reveal(&mut self, lay: &Lay<'_>, doc: &Document, toks: &mut Vec<Token>) {
        let mode = std::mem::take(&mut self.reveal);
        if mode == Reveal::None {
            return;
        }
        let view_h = self.geo.view_h;
        let row_h = lay.row_h;
        let (l, c) = self.sel.head;
        let lv = lay.line(doc, l, toks);
        let ch = lv.ch(doc.line(l), c);
        let r = lv.row_of(ch, self.up);
        let rows_in_view = (view_h / row_h).ceil() as u32 + 2;
        let limit = rows_in_view.saturating_mul(2);
        let lens_above = if r == 0 { lv.lens_h(lay.lens_h) } else { 0.0 };
        match self.y_of_line(lay, doc, l, limit, toks) {
            Some(y0) => {
                let y = y0 + r as f32 * row_h;
                let off = y < 0.0 || y + row_h > view_h;
                if mode == Reveal::Center && off {
                    self.center_on(lay, doc, l, r, toks);
                } else if y - lens_above < 0.0 {
                    self.scroll_by(lay, doc, y - lens_above, toks);
                } else if y + row_h > view_h {
                    self.scroll_by(lay, doc, y + row_h - view_h, toks);
                }
            }
            None => self.center_on(lay, doc, l, r, toks),
        }
        if !lay.wrap {
            let x = lv.x_in_row(0, ch);
            let w = self.geo.text.width();
            let margin = (self.geo.char_w * 4.0).min(w * 0.25);
            self.widest = self.widest.max(lv.row_right(0));
            if x < self.scroll_x + margin {
                self.scroll_x = (x - margin).max(0.0);
            } else if x > self.scroll_x + w - margin {
                self.scroll_x = x - w + margin;
            }
        }
    }

    fn center_on(&mut self, lay: &Lay<'_>, doc: &Document, line: u32, row: usize, toks: &mut Vec<Token>) {
        self.top_line = line;
        self.top_off = row as f32 * lay.row_h;
        let dy = -(self.geo.view_h * 0.5 - lay.row_h * 0.5);
        self.scroll_by(lay, doc, dy, toks);
    }

    /// Lines laid out from the anchor down to the bottom of `text` (screen rect).
    pub fn visible(&self, lay: &Lay<'_>, doc: &Document, text: Rect, toks: &mut Vec<Token>) -> Vec<Vis> {
        let mut out = Vec::new();
        let n = doc.line_count();
        let mut line = self.top_line.min(n.saturating_sub(1));
        let mut y = text.top() - self.top_off;
        loop {
            let view = lay.line(doc, line, toks);
            let lens = view.lens_h(lay.lens_h);
            if y - lens > text.bottom() {
                break;
            }
            let h = view.text_rows_h(lay.row_h);
            out.push(Vis { view, y });
            y += h;
            line += 1;
            if line >= n {
                break;
            }
            y += if lay.has_lens(line) { lay.lens_h } else { 0.0 };
        }
        out
    }
}

fn lenses_in(lay: &Lay<'_>, a: u32, b: u32) -> usize {
    if !lay.show_lenses {
        return 0;
    }
    let i = lay.lenses.partition_point(|l| l.line < a);
    let j = lay.lenses.partition_point(|l| l.line < b);
    j.saturating_sub(i)
}

impl Reveal {
    pub fn max_near(self) -> Reveal {
        if self == Reveal::None {
            Reveal::Near
        } else {
            self
        }
    }
}

/// Result of hit-testing a screen point against the visible rows.
pub enum Hit {
    Text(Pos, bool),
    /// In a hint row (not selectable).
    Lens,
}

pub fn hit_test(vis: &[Vis], lay: &Lay<'_>, doc: &Document, text: Rect, scroll_x: f32, p: Pos2) -> Option<Hit> {
    let first = vis.first()?;
    let x = p.x - text.left() + if lay.wrap { 0.0 } else { scroll_x };
    let hit_in = |v: &Vis, r: usize| {
        let (ch, up) = v.view.hit_row(r, x);
        let line = v.view.line;
        (doc.clamp((line, v.view.col(doc.line(line), ch))), up)
    };
    if p.y < first.y - first.view.lens_h(lay.lens_h) {
        let (pos, up) = hit_in(first, 0);
        return Some(Hit::Text(pos, up));
    }
    for v in vis {
        let lens = v.view.lens_h(lay.lens_h);
        if p.y < v.y && p.y >= v.y - lens {
            return Some(Hit::Lens);
        }
        let h = v.view.text_rows_h(lay.row_h);
        if p.y >= v.y && p.y < v.y + h {
            let r = (((p.y - v.y) / lay.row_h).floor() as usize).min(v.view.rows.len().saturating_sub(1));
            let (pos, up) = hit_in(v, r);
            return Some(Hit::Text(pos, up));
        }
    }
    let last = vis.last()?;
    if last.view.line + 1 >= doc.line_count() {
        return Some(Hit::Text(doc.buf.end_pos(), false));
    }
    let r = last.view.rows.len().saturating_sub(1);
    let (pos, up) = hit_in(last, r);
    Some(Hit::Text(pos, up))
}

/// The whole frame.
pub fn show(ui: &mut egui::Ui, doc: &mut Document, v: &mut View, opts: &EditorOptions, theme: &Theme, lenses: &[Lens]) -> EditorOutput {
    let mut out = EditorOutput::default();
    let rect = ui.available_rect_before_wrap();
    let id = ui.make_persistent_id("margin-editor");
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    ui.advance_cursor_after_rect(rect);
    if opts.request_focus || std::mem::take(&mut v.focus_text) {
        ui.memory_mut(|m| m.request_focus(id));
    }
    v.clamp_to(doc);
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let mac = ctx.os() == egui::os::OperatingSystem::Mac;
    let time = ui.input(|i| i.time);

    // Metrics.
    let font = theme.font_code();
    let row_h = theme.code_row_h().max(4.0);
    let lens_h = (row_h * 0.85).round().max(4.0);
    let char_w = ctx.fonts_mut(|f| f.glyph_width(&font, '0')).max(1.0);
    let digits = digits(doc.line_count()).max(3);
    let gutter_w = if opts.line_numbers { snap(digits as f32 * char_w + char_w * 3.0, ppp) } else { snap(char_w * 1.0, ppp) };
    let sb_w = 10.0f32;
    let text_left = rect.left() + gutter_w;
    let text = Rect::from_min_max(Pos2::new(text_left, rect.top()), Pos2::new((rect.right() - sb_w).max(text_left + 1.0), rect.bottom()));
    let wrap = match opts.wrap {
        Wrap::Auto => doc.lang().is_prose(),
        Wrap::On => true,
        Wrap::Off => false,
    };
    v.geo = Geo { rect, text, view_h: rect.height(), char_w };
    let preedit_owned = v.preedit.clone();
    let mut toks: Vec<Token> = Vec::new();
    let focused_before = ui.memory(|m| m.has_focus(id));
    let lay = Lay {
        ctx: ctx.clone(),
        theme,
        font: font.clone(),
        small: egui::FontId::new((font.size * 0.72).max(6.0), font.family.clone()),
        lens_font: egui::FontId::new((font.size * 0.85).max(6.0), egui::FontFamily::Proportional),
        row_h,
        lens_h,
        tab: opts.tab_width.clamp(1, 16),
        show_ws: opts.show_whitespace,
        wrap,
        wrap_w: (text.width() - char_w * 2.0).max(char_w * 8.0),
        lenses,
        show_lenses: opts.show_lenses,
        preedit: preedit_owned.as_deref().filter(|_| focused_before).map(|t| (v.sel.head, t)),
        prose: doc.lang().is_prose(),
        lens_hover: v.lens_hover,
    };
    let dollar = matches!(doc.lang(), margin_code::Lang::JavaScript | margin_code::Lang::TypeScript | margin_code::Lang::Php | margin_code::Lang::Shell);

    // Views as drawn last frame, for hit testing.
    let vis0 = v.visible(&lay, doc, text, &mut toks);
    let rev0 = doc.revision();
    {
        let mut cx = Cx { doc, v, lay: &lay, opts, out: &mut out, time, mac, toks: std::mem::take(&mut toks), copy: None, dollar };
        crate::input::lens_widgets(ui, &mut cx, &vis0, text, id);
        crate::input::mouse(ui, &mut cx, &resp, &vis0, id);
        if ui.memory(|m| m.has_focus(id)) {
            ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false }));
            crate::input::keys(ui, &mut cx);
        }
        if let Some(t) = cx.copy.take() {
            ctx.copy_text(t);
        }
        crate::findbar::step(&mut cx);
        toks = std::mem::take(&mut cx.toks);
    }
    let focused = ui.memory(|m| m.has_focus(id));
    out.focused = focused;
    v.had_focus = focused;

    // Preedit may have changed: rebuild the layout context's view of it.
    let preedit_owned = v.preedit.clone();
    let lay = Lay { preedit: preedit_owned.as_deref().filter(|_| focused).map(|t| (v.sel.head, t)), lens_hover: v.lens_hover, ..lay };

    v.clamp_to(doc);
    v.clamp_end(&lay, doc, &mut toks);
    v.do_reveal(&lay, doc, &mut toks);

    // Lex up to the last visible line within the budget (lines past it render plain until reached).
    let approx_last = v.top_line.saturating_add((text.height() / row_h).ceil() as u32 + 2).min(doc.line_count().saturating_sub(1));
    let lexed = doc.lex_upto(approx_last, LEX_BUDGET);
    let vis = v.visible(&lay, doc, text, &mut toks);
    if !lay.wrap {
        for x in &vis {
            v.widest = v.widest.max(x.view.row_right(0));
        }
        let max_x = (v.widest - text.width() + char_w * 4.0).max(0.0);
        v.scroll_x = v.scroll_x.clamp(0.0, max_x);
    } else {
        v.scroll_x = 0.0;
    }

    out.changed = doc.revision() != rev0;
    out.word_at_cursor = word_at(doc, v.sel.head, dollar);

    crate::paint::paint(ui, doc, v, &lay, opts, &vis, focused, &resp, gutter_w, ppp, dollar);
    crate::paint::scrollbars(ui, doc, v, &lay, &resp, &mut toks, id);
    crate::findbar::ui(ui, doc, v, theme, rect, &mut out, id);

    v.caret = crate::paint::cursor_rect(v, &lay, doc, &vis, ppp).filter(|r| text.expand(1.0).contains(r.center()));
    if focused {
        let to_global = ctx.layer_transform_to_global(ui.layer_id()).unwrap_or_default();
        let cur = crate::paint::cursor_rect(v, &lay, doc, &vis, ppp).unwrap_or(Rect::from_min_size(text.min, Vec2::new(1.0, row_h)));
        ctx.output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput { purpose: egui::IMEPurpose::Normal, rect: to_global * text, cursor_rect: to_global * cur, should_interrupt_composition: false });
        });
    }
    if !lexed || v.find.busy() {
        ctx.request_repaint();
    }
    out
}

pub fn digits(n: u32) -> usize {
    let mut d = 1;
    let mut n = n / 10;
    while n > 0 {
        d += 1;
        n /= 10;
    }
    d
}

pub fn word_at(doc: &Document, p: Pos, dollar: bool) -> String {
    let line = doc.line(p.0);
    crate::text::ident_at(line, p.1 as usize, dollar).map(|(a, b)| line[a..b].to_owned()).unwrap_or_default()
}
