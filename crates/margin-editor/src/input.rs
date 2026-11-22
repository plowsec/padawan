//! Keyboard, IME, clipboard and mouse. Only with focus (mouse: when the pointer is on the editor).
//! Handled events are removed from the input; everything else stays for the shell.
//!
//! | Keys (macOS / others) | Action |
//! |---|---|
//! | ←/→ · ⌥←/→ / Ctrl+←/→ · ⌘←/→ | char · word · line start/end |
//! | ↑/↓ · ⌘↑/↓ / Ctrl+Home/End | visual row · document start/end |
//! | Home / End | visual row first, then line (Home toggles first non-blank / column 0) |
//! | PageUp / PageDown | one view |
//! | + Shift | extend the selection |
//! | Backspace / Delete · ⌥ / Ctrl · ⌘ | char · word · to line start/end |
//! | Enter · Tab · Shift+Tab | newline + indent · indent or tab · outdent |
//! | ⌘/Ctrl+Z · ⌘/Ctrl+Shift+Z · Ctrl+Y | undo · redo · redo |
//! | ⌘/Ctrl+A · ⌘/Ctrl+/ · F12 | select all · toggle comment · go to definition |
//! | Esc | close the find bar (only when open) |
//!
//! | Mouse | Action |
//! |---|---|
//! | click · double · triple | caret · word · line |
//! | Shift+click · drag | extend (auto-scroll at the edges) |
//! | gutter click / drag | select lines |
//! | ⌘/Ctrl+hover · ⌘/Ctrl+click | underline identifier · go to definition |

use egui::{CursorIcon, Event, ImeEvent, Key, Modifiers, Pos2, Rect, Response, Sense, Vec2};

use crate::buffer::order;
use crate::layout::LineView;
use crate::ops::{self, Reach, Tabs};
use crate::text::{floor_boundary, indent_len, next_boundary, prev_boundary, word_left, word_range_at, word_right};
use crate::undo::{Kind, Sel};
use crate::view::{hit_test, Cx, DragMode, Hit, Reveal, Vis};
use crate::Pos;

impl Cx<'_, '_> {
    fn tabs(&self) -> Tabs {
        Tabs { width: self.opts.tab_width.clamp(1, 16), soft: self.opts.soft_tabs }
    }

    fn apply(&mut self, r: Option<Sel>) {
        if let Some(s) = r {
            self.v.set_sel(s);
        }
    }

    fn line_view(&mut self, l: u32) -> LineView {
        self.lay.line(self.doc, l, &mut self.toks)
    }

    /// Move the head to `p` (extend keeps the anchor).
    fn go(&mut self, p: Pos, extend: bool) {
        let p = self.doc.clamp(p);
        let anchor = if extend { self.v.sel.anchor } else { p };
        self.v.set_sel(Sel { anchor, head: p });
    }

    fn sel_range(&self) -> (Pos, Pos) {
        order(self.v.sel.anchor, self.v.sel.head)
    }

    fn char_step(&self, p: Pos, right: bool, word: bool) -> Pos {
        let (l, c) = p;
        let line = self.doc.line(l);
        let c = c as usize;
        if right {
            if c >= line.len() {
                if l + 1 < self.doc.line_count() {
                    return (l + 1, 0);
                }
                return p;
            }
            let n = if word { word_right(line, c) } else { next_boundary(line, c) };
            (l, n as u32)
        } else {
            if c == 0 {
                if l > 0 {
                    return (l - 1, self.doc.line(l - 1).len() as u32);
                }
                return p;
            }
            let n = if word { word_left(line, c) } else { prev_boundary(line, c) };
            (l, n as u32)
        }
    }

    /// Move by `rows` visual rows (negative = up), keeping the preferred x.
    pub fn vertical(&mut self, rows: i32, extend: bool) {
        let (mut l, c) = self.v.sel.head;
        let mut lv = self.line_view(l);
        let ch = lv.ch(self.doc.line(l), c);
        let mut r = lv.row_of(ch, self.v.up);
        let x = self.v.pref_x.unwrap_or_else(|| lv.x_in_row(r, ch));
        let n = self.doc.line_count();
        for _ in 0..rows.unsigned_abs() {
            if rows < 0 {
                if r > 0 {
                    r -= 1;
                } else if l > 0 {
                    l -= 1;
                    lv = self.line_view(l);
                    r = lv.rows.len().saturating_sub(1);
                } else {
                    self.go((0, 0), extend);
                    return;
                }
            } else if r + 1 < lv.rows.len() {
                r += 1;
            } else if l + 1 < n {
                l += 1;
                lv = self.line_view(l);
                r = 0;
            } else {
                let end = (l, self.doc.line(l).len() as u32);
                self.go(end, extend);
                return;
            }
        }
        let (ch, up) = lv.hit_row(r, x);
        let col = lv.col(self.doc.line(l), ch);
        self.go((l, col), extend);
        self.v.up = up;
        self.v.pref_x = Some(x);
    }

    fn home(&mut self, extend: bool) {
        let (l, c) = self.v.sel.head;
        let lv = self.line_view(l);
        let text = self.doc.line(l);
        let ch = lv.ch(text, c);
        let r = lv.row_of(ch, self.v.up);
        if r > 0 {
            let c0 = lv.rows[r].c0;
            if ch != c0 {
                let col = lv.col(text, c0);
                self.go((l, col), extend);
                return;
            }
        }
        let ind = indent_len(text) as u32;
        let target = if c == ind { 0 } else { ind };
        self.go((l, target), extend);
    }

    fn end(&mut self, extend: bool) {
        let (l, c) = self.v.sel.head;
        let lv = self.line_view(l);
        let text = self.doc.line(l);
        let ch = lv.ch(text, c);
        let r = lv.row_of(ch, self.v.up);
        if r + 1 < lv.rows.len() {
            let c1 = lv.rows[r].c1;
            if !(ch == c1 && self.v.up) {
                let col = lv.col(text, c1);
                self.go((l, col), extend);
                self.v.up = true;
                return;
            }
        }
        let len = text.len() as u32;
        self.go((l, len), extend);
    }

    fn page(&mut self, down: bool, extend: bool) {
        let rows = ((self.v.geo.view_h / self.lay.row_h).floor() as i32 - 1).max(1);
        let dy = rows as f32 * self.lay.row_h;
        let lay = self.lay;
        self.v.scroll_by(lay, self.doc, if down { dy } else { -dy }, &mut self.toks);
        self.vertical(if down { rows } else { -rows }, extend);
    }

    fn insert_text(&mut self, t: &str) {
        let t = t.replace("\r\n", "\n");
        if t.is_empty() {
            return;
        }
        let kind = if t.contains('\n') { Kind::Other } else { Kind::Type };
        let r = ops::insert(self.doc, self.v.sel, &t, kind, self.time);
        self.apply(r);
    }

    /// One key press. Returns true when handled.
    fn key(&mut self, key: Key, m: Modifiers) -> bool {
        let mac = self.mac;
        let cmd = m.command;
        let word_mod = if mac { m.alt } else { m.ctrl || m.command };
        let line_mod = mac && cmd;
        let shift = m.shift;
        let time = self.time;
        let tabs = self.tabs();
        match key {
            Key::ArrowLeft | Key::ArrowRight => {
                let right = key == Key::ArrowRight;
                if line_mod {
                    if right {
                        self.end(shift);
                    } else {
                        self.home(shift);
                    }
                } else if !shift && !word_mod && self.v.sel.anchor != self.v.sel.head {
                    let (a, b) = self.sel_range();
                    self.go(if right { b } else { a }, false);
                } else {
                    let p = self.char_step(self.v.sel.head, right, word_mod);
                    self.go(p, shift);
                }
            }
            Key::ArrowUp | Key::ArrowDown => {
                let down = key == Key::ArrowDown;
                if line_mod {
                    let p = if down { self.doc.buf.end_pos() } else { (0, 0) };
                    self.go(p, shift);
                } else if m.alt || (m.ctrl && !mac) {
                    return false; // line moves / scroll: not ours
                } else {
                    self.vertical(if down { 1 } else { -1 }, shift);
                }
            }
            Key::Home | Key::End => {
                let end = key == Key::End;
                if cmd && !mac {
                    let p = if end { self.doc.buf.end_pos() } else { (0, 0) };
                    self.go(p, shift);
                } else if end {
                    self.end(shift);
                } else {
                    self.home(shift);
                }
            }
            Key::PageUp | Key::PageDown => self.page(key == Key::PageDown, shift),
            Key::Backspace | Key::Delete => {
                let reach = if word_mod {
                    Reach::Word
                } else if line_mod {
                    Reach::Line
                } else {
                    Reach::Char
                };
                let r = if key == Key::Backspace { ops::backspace(self.doc, self.v.sel, reach, &tabs, time) } else { ops::delete(self.doc, self.v.sel, reach, time) };
                self.apply(r);
            }
            Key::Enter => {
                if cmd || m.alt || m.ctrl {
                    return false;
                }
                let r = ops::newline(self.doc, self.v.sel, &tabs, time);
                self.apply(r);
            }
            Key::Tab => {
                if cmd || m.alt || m.ctrl {
                    return false;
                }
                let r = if shift { ops::outdent(self.doc, self.v.sel, &tabs, time) } else { ops::tab(self.doc, self.v.sel, &tabs, time) };
                self.apply(r);
            }
            Key::Z if cmd => {
                let r = if shift { self.doc.redo_group() } else { self.doc.undo_group() };
                self.apply(r);
            }
            Key::Y if cmd && !mac => {
                let r = self.doc.redo_group();
                self.apply(r);
            }
            Key::A if cmd && !shift => {
                self.v.sel = Sel { anchor: (0, 0), head: self.doc.buf.end_pos() };
                self.v.up = false;
            }
            Key::Slash if cmd => {
                if let Some(p) = self.doc.lang().line_comment() {
                    let r = ops::toggle_comment(self.doc, self.v.sel, p, time);
                    self.apply(r);
                }
            }
            Key::F12 if !cmd && !shift => {
                let head = self.v.sel.head;
                self.goto_def(head);
            }
            Key::Escape if self.v.find.open && m.is_none() => {
                self.v.find.close();
            }
            _ => return false,
        }
        true
    }

    fn goto_def(&mut self, p: Pos) {
        let line = self.doc.line(p.0);
        if let Some((a, b)) = crate::text::ident_at(line, p.1 as usize, self.dollar) {
            self.out.goto_definition = Some((line[a..b].to_owned(), (p.0, a as u32)));
        }
    }
}

/// Keyboard, text, IME and clipboard events (editor focused).
pub fn keys(ui: &mut egui::Ui, cx: &mut Cx<'_, '_>) {
    let events = ui.input(|i| i.events.clone());
    let mut handled = vec![false; events.len()];
    for (k, ev) in events.iter().enumerate() {
        let h = match ev {
            Event::Text(t) => {
                if cx.v.preedit.is_none() {
                    let clean: String = t.chars().filter(|&c| c != '\r' && c != '\n').collect();
                    cx.insert_text(&clean);
                }
                true
            }
            Event::Ime(ime) => {
                match ime {
                    ImeEvent::Preedit { text, .. } => {
                        cx.v.preedit = (!text.is_empty()).then(|| text.clone());
                        cx.v.reveal = cx.v.reveal.max_near();
                    }
                    ImeEvent::Commit(t) => {
                        cx.v.preedit = None;
                        cx.insert_text(t);
                    }
                    _ => {}
                }
                true
            }
            Event::Copy => {
                let (a, b) = ops::copy_range(cx.doc, cx.v.sel);
                let mut t = cx.doc.buf.slice(a, b);
                if cx.v.sel.anchor == cx.v.sel.head && !t.ends_with('\n') {
                    t = format!("{}\n", t.trim_start_matches('\n'));
                }
                cx.copy = Some(t);
                true
            }
            Event::Cut => {
                if let Some((t, s)) = ops::cut(cx.doc, cx.v.sel, cx.time) {
                    cx.copy = Some(t);
                    cx.v.set_sel(s);
                }
                true
            }
            Event::Paste(t) => {
                cx.insert_text(t);
                true
            }
            Event::Key { key, pressed: true, modifiers, .. } => cx.key(*key, *modifiers),
            _ => false,
        };
        handled[k] = h;
    }
    if handled.iter().any(|&h| h) {
        ui.input_mut(|i| {
            if i.events.len() == handled.len() {
                let mut k = 0;
                i.events.retain(|_| {
                    let keep = !handled[k];
                    k += 1;
                    keep
                });
            }
        });
    }
}

/// Clickable hint segments: real egui widgets (hover, tooltip, click).
pub fn lens_widgets(ui: &mut egui::Ui, cx: &mut Cx<'_, '_>, vis: &[Vis], text: Rect, id: egui::Id) {
    let mut hover = None;
    let dx = text.left() - if cx.lay.wrap { 0.0 } else { cx.v.scroll_x };
    for v in vis {
        let Some(lens) = &v.view.lens else { continue };
        let y1 = v.y;
        let y0 = y1 - cx.lay.lens_h;
        for s in &lens.segs {
            if !s.clickable {
                continue;
            }
            let r = Rect::from_min_max(Pos2::new(dx + s.x0, y0), Pos2::new(dx + s.x1, y1)).intersect(text);
            if !r.is_positive() {
                continue;
            }
            let mut resp = ui.interact(r, id.with(("lens", v.view.line, s.idx)), Sense::click());
            if resp.hovered() {
                hover = Some((v.view.line, s.idx));
                resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                if !s.hover.is_empty() {
                    resp = resp.on_hover_text(s.hover.as_str());
                }
            }
            if resp.clicked() {
                cx.out.lens_clicked = Some((v.view.line, s.id));
            }
        }
    }
    cx.v.lens_hover = hover;
}

/// Mouse: wheel, press, drag, gutter, cmd+hover.
pub fn mouse(ui: &mut egui::Ui, cx: &mut Cx<'_, '_>, resp: &Response, vis: &[Vis], id: egui::Id) {
    let geo = cx.v.geo;
    let text = geo.text;
    let rect = geo.rect;
    let (pressed, down, pos, mods, wheel, dt) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos(), i.modifiers, i.smooth_scroll_delta, i.stable_dt.min(0.1)));
    // Modifiers of the press itself (more precise than the frame's).
    let press_mods = ui.input(|i| {
        i.events.iter().rev().find_map(|e| match e {
            Event::PointerButton { pressed: true, button: egui::PointerButton::Primary, modifiers, .. } => Some(*modifiers),
            _ => None,
        })
    });
    let lay = cx.lay;

    if wheel != Vec2::ZERO && ui.rect_contains_pointer(rect) {
        cx.v.scroll_by(lay, cx.doc, -wheel.y, &mut cx.toks);
        if !lay.wrap {
            cx.v.scroll_x = (cx.v.scroll_x - wheel.x).max(0.0);
        }
        ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }

    let hover = resp.hover_pos();
    cx.v.ctrl_word = None;
    if let Some(p) = hover {
        if text.contains(p) {
            if mods.command {
                if let Some(Hit::Text(pos, _)) = hit_test(vis, lay, cx.doc, text, cx.v.scroll_x, p) {
                    let line = cx.doc.line(pos.0);
                    if let Some((a, b)) = crate::text::ident_at(line, pos.1 as usize, cx.dollar) {
                        cx.v.ctrl_word = Some((pos.0, a as u32, b as u32));
                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    }
                }
            } else {
                ui.ctx().set_cursor_icon(CursorIcon::Text);
            }
        }
    }

    if pressed && resp.hovered() {
        if let Some(p) = pos.filter(|p| p.x < text.right()) {
            ui.memory_mut(|m| m.request_focus(id));
            press(cx, vis, p, press_mods.unwrap_or(mods));
        }
    }
    if !down {
        cx.v.click.drag = None;
        return;
    }
    let Some(mode) = cx.v.click.drag else { return };
    let Some(p) = pos else { return };
    // Auto-scroll when dragging past the edges.
    let mut dy = 0.0;
    if p.y < text.top() {
        dy = (p.y - text.top()) * 12.0 * dt;
    } else if p.y > text.bottom() {
        dy = (p.y - text.bottom()) * 12.0 * dt;
    }
    if dy != 0.0 {
        let dy = if dy.abs() < 1.0 { dy.signum() } else { dy };
        cx.v.scroll_by(lay, cx.doc, dy, &mut cx.toks);
        ui.ctx().request_repaint();
    }
    if !lay.wrap && mode != DragMode::Line {
        if p.x > text.right() {
            cx.v.scroll_x += (p.x - text.right()) * 12.0 * dt;
            ui.ctx().request_repaint();
        } else if p.x < text.left() && cx.v.scroll_x > 0.0 {
            cx.v.scroll_x = (cx.v.scroll_x - (text.left() - p.x) * 12.0 * dt).max(0.0);
            ui.ctx().request_repaint();
        }
    }
    let q = Pos2::new(p.x.clamp(text.left() - 1.0, text.right()), p.y.clamp(text.top(), text.bottom() - 1.0));
    let q = if mode == DragMode::Line { Pos2::new(text.left() + 1.0, q.y) } else { q };
    let Some(Hit::Text(at, up)) = hit_test(vis, lay, cx.doc, text, cx.v.scroll_x, q).or(Some(Hit::Text(cx.v.sel.head, false))) else { return };
    extend_drag(cx, mode, at, up);
}

fn press(cx: &mut Cx<'_, '_>, vis: &[Vis], p: Pos2, mods: Modifiers) {
    let text = cx.v.geo.text;
    let lay = cx.lay;
    // Click count.
    let c = &mut cx.v.click;
    if cx.time - c.time < 0.45 && (p - c.pos).length() < 6.0 {
        c.count = c.count % 3 + 1;
    } else {
        c.count = 1;
    }
    c.time = cx.time;
    c.pos = p;
    let count = c.count;
    if p.x < text.left() {
        // Gutter: select the line.
        let q = Pos2::new(text.left() + 1.0, p.y);
        if let Some(Hit::Text(at, _)) = hit_test(vis, lay, cx.doc, text, cx.v.scroll_x, q) {
            let (a, b) = line_range(cx, at.0);
            let anchor = if mods.shift { cx.v.sel.anchor } else { a };
            cx.v.set_sel(Sel { anchor, head: b });
            cx.v.click.origin = (a, b);
            cx.v.click.drag = Some(DragMode::Line);
        }
        return;
    }
    let hit = hit_test(vis, lay, cx.doc, text, cx.v.scroll_x, p);
    let Some(Hit::Text(at, up)) = hit else {
        cx.v.click.drag = None;
        return;
    };
    if mods.command && !mods.shift {
        cx.go(at, false);
        cx.goto_def(at);
        cx.v.click.drag = None;
        return;
    }
    if mods.shift {
        cx.go(at, true);
        cx.v.up = up;
        let a = cx.v.sel.anchor;
        cx.v.click.origin = (a, a);
        cx.v.click.drag = Some(DragMode::Char);
        return;
    }
    match count {
        2 => {
            let line = cx.doc.line(at.0);
            let (a, b) = word_range_at(line, at.1 as usize);
            let (a, b) = ((at.0, a as u32), (at.0, b as u32));
            cx.v.set_sel(Sel { anchor: a, head: b });
            cx.v.click.origin = (a, b);
            cx.v.click.drag = Some(DragMode::Word);
        }
        3 => {
            let (a, b) = line_range(cx, at.0);
            cx.v.set_sel(Sel { anchor: a, head: b });
            cx.v.click.origin = (a, b);
            cx.v.click.drag = Some(DragMode::Line);
        }
        _ => {
            cx.go(at, false);
            cx.v.up = up;
            cx.v.click.origin = (at, at);
            cx.v.click.drag = Some(DragMode::Char);
        }
    }
    cx.v.reveal = Reveal::None;
}

fn line_range(cx: &Cx<'_, '_>, l: u32) -> (Pos, Pos) {
    let a = (l, 0);
    let b = if l + 1 < cx.doc.line_count() { (l + 1, 0) } else { (l, cx.doc.line(l).len() as u32) };
    (a, b)
}

fn extend_drag(cx: &mut Cx<'_, '_>, mode: DragMode, at: Pos, up: bool) {
    let (oa, ob) = cx.v.click.origin;
    let (anchor, head) = match mode {
        DragMode::Char => (cx.v.sel.anchor, at),
        DragMode::Word => {
            let line = cx.doc.line(at.0);
            let (a, b) = word_range_at(line, floor_boundary(line, at.1 as usize));
            let (a, b) = ((at.0, a as u32), (at.0, b as u32));
            if at < oa {
                (ob, a)
            } else {
                (oa, b.max(ob))
            }
        }
        DragMode::Line => {
            let (a, b) = line_range(cx, at.0);
            if at < oa {
                (ob, a)
            } else {
                (oa, b.max(ob))
            }
        }
    };
    let sel = Sel { anchor, head };
    if sel != cx.v.sel {
        cx.v.sel = sel;
        cx.v.pref_x = None;
        cx.v.reveal = Reveal::None;
    }
    cx.v.up = up && mode == DragMode::Char;
}
