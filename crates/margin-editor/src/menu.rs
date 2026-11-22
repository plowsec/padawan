//! Context menu (`EditorOptions::context_menu`): secondary click in the text area. Drawn on a foreground
//! `egui::Area`, so the editor rect never clips it; flipped / shifted to stay inside the window.
//!
//! | Rule | Behaviour |
//! |---|---|
//! | open | secondary press on the text; outside the selection: caret moves there first |
//! | items | Cut Copy Paste · Select All · Go to Definition, Find Callers (identifier + outline language) · Toggle Comment (line comment language) |
//! | disabled | Cut, Paste, Toggle Comment on read-only documents (shown greyed) |
//! | keys | ↑/↓ move, Enter choose, Esc close (focus back to the text); any other key closes and goes through |
//! | close | Esc, press outside, wheel, after choosing, option turned off |

use egui::{Align2, CornerRadius, Event, Id, Key, Order, Pos2, Rect, Response, Sense, Shadow, Stroke, Vec2};
use margin_theme::Theme;

use crate::buffer::order;
use crate::undo::Sel;
use crate::view::{hit_test, Cx, Hit, View, Vis};
use crate::{Document, EditAction, EditorOutput, Pos};

pub struct Menu {
    /// Screen point of the click.
    pub at: Pos2,
    /// Highlighted row (keyboard or hover).
    pub hi: Option<usize>,
    /// Screen rect as drawn (for "press outside").
    pub rect: Rect,
    /// Identifier at the cursor when opened, with its start.
    pub word: Option<(String, Pos)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Act(EditAction),
    GotoDef,
    FindCallers,
}

pub struct Row {
    pub item: Item,
    pub label: &'static str,
    pub keys: String,
    pub enabled: bool,
    /// Separator above this row.
    pub sep: bool,
}

/// Rows for the current document state. `sym`: the mono font has `⌘`.
pub fn rows(doc: &Document, word: bool, mac: bool, sym: bool) -> Vec<Row> {
    let ro = doc.read_only();
    let k = |c: &str| {
        if sym {
            format!("⌘{c}")
        } else if mac {
            format!("Cmd+{c}")
        } else {
            format!("Ctrl+{c}")
        }
    };
    let row = |item, label, keys, enabled, sep| Row { item, label, keys, enabled, sep };
    let mut v = vec![
        row(Item::Act(EditAction::Cut), "Cut", k("X"), !ro, false),
        row(Item::Act(EditAction::Copy), "Copy", k("C"), true, false),
        row(Item::Act(EditAction::Paste), "Paste", k("V"), !ro, false),
        row(Item::Act(EditAction::SelectAll), "Select All", k("A"), true, true),
    ];
    if word && doc.lang().has_outline() {
        v.push(row(Item::GotoDef, "Go to Definition", "F12".to_owned(), true, true));
        v.push(row(Item::FindCallers, "Find Callers", String::new(), true, false));
    }
    if doc.lang().line_comment().is_some() {
        v.push(row(Item::Act(EditAction::ToggleComment), "Toggle Comment", k("/"), !ro, true));
    }
    v
}

/// Next enabled row after `from` (wrapping), `down` or up.
fn step(rows: &[Row], from: Option<usize>, down: bool) -> Option<usize> {
    let n = rows.len();
    if n == 0 {
        return None;
    }
    let mut i = match from {
        Some(i) => i,
        None if down => n - 1,
        None => 0,
    };
    for _ in 0..n {
        i = if down { (i + 1) % n } else { (i + n - 1) % n };
        if rows.get(i).is_some_and(|r| r.enabled) {
            return Some(i);
        }
    }
    None
}

/// Run a chosen row now (inside the frame's `Cx`).
fn choose(cx: &mut Cx<'_, '_>, item: Item, word: Option<(String, Pos)>) {
    match item {
        Item::Act(a) => cx.action(a),
        Item::GotoDef => cx.out.goto_definition = word,
        Item::FindCallers => cx.out.find_callers = word.map(|w| w.0),
    }
}

/// Before mouse and keys: close conditions, menu keys, opening on a secondary press.
pub fn input(ui: &mut egui::Ui, cx: &mut Cx<'_, '_>, resp: &Response, vis: &[Vis], id: Id) {
    if !cx.opts.context_menu {
        cx.v.menu = None;
        return;
    }
    if let Some(m) = &cx.v.menu {
        let events = ui.input(|i| i.events.clone());
        let rows = rows(cx.doc, m.word.is_some(), cx.mac, false);
        let mut handled = vec![false; events.len()];
        let (mut close, mut refocus, mut hi, mut pick) = (false, false, m.hi, None);
        for (k, e) in events.iter().enumerate() {
            match e {
                Event::PointerButton { pos, pressed: true, .. } if !m.rect.contains(*pos) => close = true,
                Event::MouseWheel { .. } => close = true,
                Event::Key { key: Key::Escape, pressed, .. } => {
                    handled[k] = true;
                    if *pressed {
                        close = true;
                        refocus = true;
                    }
                }
                Event::Key { key: key @ (Key::ArrowUp | Key::ArrowDown), pressed, .. } => {
                    handled[k] = true;
                    if *pressed {
                        hi = step(&rows, hi, *key == Key::ArrowDown);
                    }
                }
                Event::Key { key: Key::Enter, pressed, .. } => {
                    handled[k] = true;
                    if *pressed {
                        pick = hi.and_then(|i| rows.get(i)).filter(|r| r.enabled).map(|r| r.item);
                    }
                }
                Event::Key { pressed: true, .. } | Event::Text(_) | Event::Paste(_) | Event::Copy | Event::Cut => close = true,
                _ => {}
            }
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
        let word = m.word.clone();
        if let Some(m) = cx.v.menu.as_mut() {
            m.hi = hi;
        }
        if let Some(item) = pick {
            cx.v.menu = None;
            choose(cx, item, word);
            refocus = true;
        } else if close {
            cx.v.menu = None;
        }
        if refocus {
            ui.memory_mut(|m| m.request_focus(id));
        }
        ui.ctx().request_repaint();
    }

    let (secondary, pos) = ui.input(|i| (i.pointer.secondary_pressed(), i.pointer.interact_pos()));
    let text = cx.v.geo.text;
    let Some(p) = pos.filter(|p| secondary && resp.hovered() && text.contains(*p)) else { return };
    let Some(Hit::Text(at, up)) = hit_test(vis, cx.lay, cx.doc, text, cx.v.scroll_x, p) else { return };
    let (a, b) = order(cx.v.sel.anchor, cx.v.sel.head);
    let inside = a != b && a <= at && at <= b;
    if !inside {
        cx.v.set_sel(Sel::caret(cx.doc.clamp(at)));
        cx.v.up = up;
    }
    cx.v.click.drag = None;
    let head = cx.v.sel.head;
    let line = cx.doc.line(head.0);
    let word = crate::text::ident_at(line, head.1 as usize, cx.dollar).map(|(s, e)| (line[s..e].to_owned(), (head.0, s as u32)));
    cx.v.menu = Some(Menu { at: p, hi: None, rect: Rect::from_min_size(p, Vec2::ZERO), word });
    ui.memory_mut(|m| m.request_focus(id));
    ui.ctx().request_repaint();
}

/// Where a `size` menu opened at `at` goes inside `screen`: right/below the point, flipped when it does not
/// fit, then shifted inside a 4 pt margin.
pub fn place(at: Pos2, size: Vec2, screen: Rect) -> Pos2 {
    let m = 4.0;
    let fit = |p: f32, len: f32, lo: f32, hi: f32| {
        let p = if p + len > hi - m && p - len >= lo + m { p - len } else { p };
        p.clamp(lo + m, (hi - m - len).max(lo + m))
    };
    Pos2::new(fit(at.x + 1.0, size.x, screen.left(), screen.right()), fit(at.y + 1.0, size.y, screen.top(), screen.bottom()))
}

/// After painting: draw the open menu, mouse hover and clicks.
pub fn ui(ui: &mut egui::Ui, doc: &Document, v: &mut View, th: &Theme, out: &mut EditorOutput, id: Id, mac: bool) {
    let Some(m) = v.menu.as_mut() else { return };
    let ctx = ui.ctx().clone();
    let font = th.font_ui();
    let mono = th.font_mono_ui();
    let sym = mac && ctx.fonts_mut(|f| f.has_glyphs(&mono, "⌘"));
    let rows = rows(doc, m.word.is_some(), mac, sym);
    let rh = th.row_h().max(16.0);
    let (inset, pad_x, gap, sep_h) = (4.0, 10.0, 28.0, 9.0);
    let (lw, kw) = ctx.fonts_mut(|f| {
        rows.iter().fold((0.0f32, 0.0f32), |(lw, kw), r| {
            let a = f.layout_no_wrap(r.label.to_owned(), font.clone(), th.ui.text).size().x;
            let b = if r.keys.is_empty() { 0.0 } else { f.layout_no_wrap(r.keys.clone(), mono.clone(), th.ui.text).size().x };
            (lw.max(a), kw.max(b))
        })
    });
    let seps = rows.iter().filter(|r| r.sep).count() as f32;
    let w = (inset * 2.0 + pad_x * 2.0 + lw + gap + kw).max(180.0).ceil();
    let h = inset * 2.0 + rows.len() as f32 * rh + seps * sep_h;
    let ppp = ctx.pixels_per_point();
    let min = place(m.at, Vec2::new(w, h), ctx.content_rect());
    let min = Pos2::new((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
    let rect = Rect::from_min_size(min, Vec2::new(w, h));
    m.rect = rect;
    let hi = m.hi;
    let mut hovered = None;
    let mut clicked = None;
    egui::Area::new(id.with("ctx-menu")).order(Order::Foreground).fixed_pos(rect.min).constrain(false).fade_in(false).show(&ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::click());
        let p = ui.painter().clone();
        let radius = CornerRadius::same(5);
        let shadow = Shadow { offset: [0, 2], blur: 8, spread: 0, color: egui::Color32::from_black_alpha(if th.mode == margin_theme::Mode::Dark { 70 } else { 22 }) };
        p.add(shadow.as_shape(r, radius));
        p.rect_filled(r, radius, th.ui.overlay_bg);
        p.rect_stroke(r, radius, Stroke::new(1.0, th.ui.border), egui::StrokeKind::Inside);
        let mut y = r.top() + inset;
        for (i, row) in rows.iter().enumerate() {
            if row.sep && i > 0 {
                let ly = ((y + sep_h * 0.5) * ppp).floor() / ppp + 0.5 / ppp;
                p.line_segment([Pos2::new(r.left() + inset + 1.0, ly), Pos2::new(r.right() - inset - 1.0, ly)], Stroke::new(1.0 / ppp, th.ui.border));
                y += sep_h;
            }
            let rr = Rect::from_min_size(Pos2::new(r.left() + inset, y), Vec2::new(r.width() - inset * 2.0, rh));
            let resp = ui.interact(rr, id.with(("ctx-menu-row", i)), Sense::click());
            if row.enabled && resp.hovered() {
                hovered = Some(i);
            }
            if row.enabled && resp.clicked() {
                clicked = Some(row.item);
            }
            let lit = row.enabled && (hovered == Some(i) || (hovered.is_none() && hi == Some(i)));
            if lit {
                p.rect_filled(rr, CornerRadius::same(3), th.ui.row_selected);
            }
            let (fg, kc) = if !row.enabled {
                (th.ui.text_faint, th.ui.text_faint)
            } else if lit {
                (th.ui.text_strong, th.ui.text_muted)
            } else {
                (th.ui.text, th.ui.text_muted)
            };
            p.text(Pos2::new(rr.left() + pad_x, rr.center().y), Align2::LEFT_CENTER, row.label, font.clone(), fg);
            if !row.keys.is_empty() {
                p.text(Pos2::new(rr.right() - pad_x, rr.center().y), Align2::RIGHT_CENTER, &row.keys, mono.clone(), kc);
            }
            y += rh;
        }
    });
    if hovered.is_some() {
        m.hi = hovered;
    }
    if let Some(item) = clicked {
        let word = m.word.clone();
        v.menu = None;
        match item {
            Item::Act(a) => v.picks.push(a),
            Item::GotoDef => out.goto_definition = word,
            Item::FindCallers => out.find_callers = word.map(|w| w.0),
        }
        v.focus_text = true;
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use margin_code::Lang;

    #[test]
    fn rows_follow_language_and_read_only() {
        let labels = |d: &Document, w| rows(d, w, false, false).iter().map(|r| (r.label, r.enabled)).collect::<Vec<_>>();
        let mut d = Document::new("x", Lang::Rust);
        assert_eq!(labels(&d, true).len(), 7);
        assert_eq!(labels(&d, false).len(), 5, "no identifier: no Go to Definition / Find Callers");
        d.set_read_only(true);
        let l = labels(&d, true);
        assert!(l.contains(&("Cut", false)) && l.contains(&("Paste", false)) && l.contains(&("Toggle Comment", false)));
        assert!(l.contains(&("Copy", true)) && l.contains(&("Select All", true)) && l.contains(&("Go to Definition", true)));
        let d = Document::new("x", Lang::Markdown);
        assert_eq!(labels(&d, true).iter().map(|x| x.0).collect::<Vec<_>>(), ["Cut", "Copy", "Paste", "Select All"]);
        assert_eq!(rows(&d, false, true, true)[0].keys, "⌘X");
        assert_eq!(rows(&d, false, true, false)[0].keys, "Cmd+X");
        assert_eq!(rows(&d, false, false, false)[1].keys, "Ctrl+C");
    }

    #[test]
    fn keyboard_skips_disabled() {
        let mut d = Document::new("x", Lang::Rust);
        d.set_read_only(true);
        let r = rows(&d, false, false, false); // Cut(off) Copy Paste(off) SelectAll ToggleComment(off)
        assert_eq!(step(&r, None, true), Some(1));
        assert_eq!(step(&r, Some(1), true), Some(3));
        assert_eq!(step(&r, Some(3), true), Some(1), "wraps");
        assert_eq!(step(&r, None, false), Some(3));
        assert_eq!(step(&[], None, true), None);
    }

    #[test]
    fn placement_flips_and_shifts() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let s = Vec2::new(200.0, 150.0);
        assert_eq!(place(Pos2::new(100.0, 100.0), s, screen), Pos2::new(101.0, 101.0));
        assert_eq!(place(Pos2::new(780.0, 590.0), s, screen), Pos2::new(581.0, 441.0), "flipped left and up");
        // Too big to flip: shifted inside.
        let tall = Vec2::new(200.0, 500.0);
        let p = place(Pos2::new(100.0, 300.0), tall, screen);
        assert!(p.y >= 4.0 && p.y + 500.0 <= 596.0, "{p:?}");
        // Bigger than the screen: pinned to the margin.
        assert_eq!(place(Pos2::new(10.0, 10.0), Vec2::new(900.0, 900.0), screen), Pos2::new(4.0, 4.0));
    }
}
