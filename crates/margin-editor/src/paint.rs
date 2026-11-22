//! Painting: background, current line, selection, matches, word/bracket highlights, guides, text, hint rows,
//! hazard boxes, cursor, gutter, scrollbars. Visible rows only. Rows snapped to the pixel grid.
//!
//! Layer order (bottom → top): bg · current line · selection · find matches · word highlight · hazard boxes ·
//! indent guides · text · bracket boxes · cmd-hover underline · hint row · cursor · gutter · scrollbars.

use egui::{CornerRadius, Pos2, Rect, Response, Sense, Stroke, Vec2};
use margin_code::Token;
use margin_theme::mix;

use crate::buffer::order;
use crate::display::SpanKind;
use crate::find::{Fold, Matcher};
use crate::layout::{Lay, LineView};
use crate::view::{snap, View, Vis};
use crate::{Document, EditorOptions, Pos};

/// Where a line's rows land on screen.
#[derive(Clone, Copy)]
struct RowGeo {
    /// Screen y of the first text row.
    y: f32,
    row_h: f32,
    /// Screen x of text-area x = 0.
    dx: f32,
    /// Width of the line-break sliver of a multi-line selection.
    eol_w: f32,
    ppp: f32,
}

/// Rects covering display chars `a..b` of a line, one per visual row.
/// `eol`: the selection continues past the line end (adds a sliver for the line break).
fn span_rects(lv: &LineView, a: u32, b: u32, eol: bool, g: RowGeo, out: &mut Vec<Rect>) {
    let RowGeo { y, row_h, dx, eol_w, ppp } = g;
    let n = lv.rows.len();
    for (r, row) in lv.rows.iter().enumerate() {
        let last = r + 1 == n;
        let lo = a.max(row.c0);
        let hi = b.min(row.c1);
        if lo > hi || (lo == hi && !(last && eol)) {
            continue;
        }
        let x0 = dx + lv.x_in_row(r, lo);
        let mut x1 = dx + lv.x_in_row(r, hi);
        if last && eol {
            x1 += eol_w;
        }
        let ry = snap(y + r as f32 * row_h, ppp);
        out.push(Rect::from_min_max(Pos2::new(snap(x0, ppp), ry), Pos2::new(snap(x1, ppp), snap(y + (r + 1) as f32 * row_h, ppp))));
    }
}

/// Bracket at/before the cursor and its partner (bounded scan).
pub fn match_bracket(doc: &Document, p: Pos) -> Option<(Pos, Pos)> {
    const MAX_CHARS: usize = 20_000;
    let line = doc.line(p.0);
    let c = p.1 as usize;
    let at = line[c.min(line.len())..].chars().next().map(|ch| (c, ch));
    let before = line[..c.min(line.len())].char_indices().next_back();
    let pick = |x: Option<(usize, char)>| x.filter(|(_, ch)| "()[]{}".contains(*ch));
    let (col, ch) = pick(before).or(pick(at))?;
    let (open, close, fwd) = match ch {
        '(' => ('(', ')', true),
        '[' => ('[', ']', true),
        '{' => ('{', '}', true),
        ')' => ('(', ')', false),
        ']' => ('[', ']', false),
        _ => ('{', '}', false),
    };
    let start = (p.0, col as u32);
    let mut depth = 0i32;
    let mut seen = 0usize;
    let n = doc.line_count();
    if fwd {
        let mut l = p.0;
        let mut from = col;
        while l < n {
            for (o, c2) in doc.line(l)[from..].char_indices() {
                seen += 1;
                if seen > MAX_CHARS {
                    return None;
                }
                if c2 == open {
                    depth += 1;
                } else if c2 == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some((start, (l, (from + o) as u32)));
                    }
                }
            }
            l += 1;
            from = 0;
        }
    } else {
        let mut l = p.0;
        let mut to = col + 1;
        loop {
            let s = doc.line(l);
            for (o, c2) in s[..to.min(s.len())].char_indices().rev() {
                seen += 1;
                if seen > MAX_CHARS {
                    return None;
                }
                if c2 == close {
                    depth += 1;
                } else if c2 == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some((start, (l, o as u32)));
                    }
                }
            }
            if l == 0 {
                break;
            }
            l -= 1;
            to = usize::MAX;
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub fn paint(ui: &egui::Ui, doc: &Document, v: &View, lay: &Lay<'_>, opts: &EditorOptions, vis: &[Vis], focused: bool, _resp: &Response, gutter_w: f32, ppp: f32, dollar: bool) {
    let th = lay.theme;
    let c = &th.code;
    let geo = v.geo;
    let rect = geo.rect;
    let text = geo.text;
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, CornerRadius::ZERO, c.bg);
    let tp = painter.with_clip_rect(text.intersect(painter.clip_rect()));
    let dx = text.left() - if lay.wrap { 0.0 } else { v.scroll_x };
    let row_h = lay.row_h;
    let (sa, sb) = order(v.sel.anchor, v.sel.head);
    let has_sel = sa != sb;
    let sel_color = if focused { c.selection } else { c.selection_inactive };
    let eol_w = (geo.char_w * 0.5).max(3.0);
    let matcher = if v.find.open { Matcher::new(&v.find.query()) } else { None };
    let current = v.find.current;
    let mut fold = Fold::default();
    let word = if !has_sel { crate::view::word_at(doc, v.sel.head, dollar) } else { String::new() };
    let word_m = (!word.is_empty() && word.len() <= 200).then(|| Matcher::new(&crate::find::Query { text: word.clone(), case: true, word: true })).flatten();
    let brackets = if focused && !has_sel { match_bracket(doc, v.sel.head) } else { None };
    let mut rects = Vec::new();
    let hair = 1.0 / ppp;

    for x in vis {
        let lv = &x.view;
        let line = lv.line;
        let ltext = doc.line(line);
        let y = snap(x.y, ppp);
        let rows_h = lv.text_rows_h(row_h);
        let g = RowGeo { y, row_h, dx, eol_w, ppp };

        if opts.highlight_current_line && line == v.sel.head.0 && !has_sel {
            painter.rect_filled(Rect::from_min_max(Pos2::new(rect.left(), y), Pos2::new(text.right(), snap(y + rows_h, ppp))), CornerRadius::ZERO, c.current_line);
        }
        // Selection.
        if has_sel && line >= sa.0 && line <= sb.0 {
            let a = if line == sa.0 { lv.ch(ltext, sa.1) } else { 0 };
            let b = if line == sb.0 { lv.ch(ltext, sb.1) } else { lv.disp.mapped_chars };
            rects.clear();
            span_rects(lv, a, b, line < sb.0, g, &mut rects);
            for r in &rects {
                tp.rect_filled(*r, CornerRadius::ZERO, sel_color);
            }
        }
        // Find matches.
        if let Some(m) = &matcher {
            let mut n = 0;
            m.each(ltext, &mut fold, |ma, mb| {
                n += 1;
                if ma >= lv.disp.shown {
                    return false;
                }
                let is_cur = current == Some(((line, ma), (line, mb)));
                rects.clear();
                span_rects(lv, lv.ch(ltext, ma), lv.ch(ltext, mb), false, g, &mut rects);
                for r in &rects {
                    tp.rect_filled(*r, CornerRadius::same(2), if is_cur { c.find_current } else { c.find_match });
                }
                n < 2000
            });
        }
        // Other occurrences of the word under the cursor.
        if let Some(m) = &word_m {
            let mut n = 0;
            m.each(ltext, &mut fold, |ma, mb| {
                n += 1;
                if ma >= lv.disp.shown {
                    return false;
                }
                rects.clear();
                span_rects(lv, lv.ch(ltext, ma), lv.ch(ltext, mb), false, g, &mut rects);
                for r in &rects {
                    tp.rect_filled(*r, CornerRadius::same(2), c.word_highlight);
                }
                n < 2000
            });
        }
        // Hazard boxes.
        for s in lv.disp.spans.iter().filter(|s| s.kind == SpanKind::Hazard) {
            let r = lv.row_of(s.ch, false);
            let x0 = dx + lv.x_in_row(r, s.ch + 1) - 2.0;
            let x1 = dx + lv.x_in_row(r, s.ch + s.ch_len - 1) + 2.0;
            let ry = y + r as f32 * row_h;
            let h = (lay.small.size + 4.0).min(row_h - 2.0);
            let top = snap(ry + (row_h - h) * 0.5, ppp);
            tp.rect_filled(Rect::from_min_max(Pos2::new(snap(x0, ppp), top), Pos2::new(snap(x1, ppp), snap(top + h, ppp))), CornerRadius::same(2), c.hazard_bg);
        }
        // Indent guides in leading whitespace.
        let glyphs = lv.galley.rows.first().map(|r| &r.glyphs[..]).unwrap_or(&[]);
        let lead = glyphs.iter().take_while(|g| matches!(g.chr, ' ' | '·' | '→')).count() as u32;
        if lead > 0 && (lead as usize) < glyphs.len() {
            let mut k = 0;
            while k < lead {
                let gx = snap(dx + lv.gx(k), ppp);
                tp.rect_filled(Rect::from_min_size(Pos2::new(gx, y), Vec2::new(hair, row_h)), CornerRadius::ZERO, c.guide);
                k += lay.tab;
            }
        }
        // Text, one clip per visual row.
        let gh = lv.galley.rows.first().map_or(lv.galley.size().y, |r| r.height());
        for (r, row) in lv.rows.iter().enumerate() {
            let ry = snap(y + r as f32 * row_h, ppp);
            let ind = if r > 0 { lv.indent_x } else { 0.0 };
            let ox = dx + ind - row.x0;
            let ty = snap(ry + (row_h - gh) * 0.5, ppp);
            let clip = if lv.rows.len() > 1 {
                let x0 = dx + ind - 1.0;
                let x1 = if r + 1 == lv.rows.len() { text.right() } else { dx + lv.x_in_row(r, row.c1) };
                Rect::from_min_max(Pos2::new(x0.max(text.left()), ry), Pos2::new(x1.min(text.right()), ry + row_h))
            } else {
                Rect::from_min_max(Pos2::new(text.left(), ry), Pos2::new(text.right(), ry + row_h))
            };
            tp.with_clip_rect(clip.intersect(tp.clip_rect())).galley(Pos2::new(snap(ox, ppp), ty), lv.galley.clone(), c.text);
        }
        // Bracket pair.
        if let Some((p1, p2)) = brackets {
            for p in [p1, p2] {
                if p.0 == line {
                    let a = lv.ch(ltext, p.1);
                    let b = lv.ch(ltext, crate::text::next_boundary(ltext, p.1 as usize) as u32);
                    rects.clear();
                    span_rects(lv, a, b, false, g, &mut rects);
                    for r in &rects {
                        tp.rect_stroke(r.shrink(hair * 0.5), CornerRadius::same(2), Stroke::new(hair, c.gutter_text), egui::StrokeKind::Inside);
                    }
                }
            }
        }
        // Cmd/Ctrl+hover underline.
        if let Some((wl, wa, wb)) = v.ctrl_word {
            if wl == line {
                rects.clear();
                span_rects(lv, lv.ch(ltext, wa), lv.ch(ltext, wb), false, g, &mut rects);
                for r in &rects {
                    let uy = snap(r.center().y + gh * 0.5, ppp);
                    tp.rect_filled(Rect::from_min_max(Pos2::new(r.left(), uy), Pos2::new(r.right(), uy + hair.max(1.0))), CornerRadius::ZERO, th.syn.link);
                }
            }
        }
        // Hint row.
        if let Some(lens) = &lv.lens {
            let ly = y - lay.lens_h;
            let lgh = lens.galley.size().y;
            let ty = snap(ly + (lay.lens_h - lgh) * 0.5 + 1.0, ppp);
            tp.galley(Pos2::new(snap(dx + lens.x, ppp), ty), lens.galley.clone(), c.lens_text);
        }
        // Gutter number.
        if opts.line_numbers {
            let col = if line == v.sel.head.0 { c.gutter_text_active } else { c.gutter_text };
            let g = painter.layout_no_wrap(format!("{}", u64::from(line) + 1), lay.font.clone(), col);
            let right = rect.left() + gutter_w - geo.char_w * 1.5;
            let ty = snap(y + (row_h - g.size().y) * 0.5, ppp);
            let gp = painter.with_clip_rect(Rect::from_min_max(rect.min, Pos2::new(rect.left() + gutter_w, rect.bottom())).intersect(painter.clip_rect()));
            gp.galley(Pos2::new(snap(right - g.size().x, ppp), ty), g, col);
        }
    }
    if focused {
        if let Some(r) = cursor_rect(v, lay, doc, vis, ppp) {
            if text.expand(1.0).contains(r.center()) {
                tp.rect_filled(r, CornerRadius::ZERO, c.cursor);
            }
        }
    }
}

/// Caret rect (screen), when its line is visible.
pub fn cursor_rect(v: &View, lay: &Lay<'_>, doc: &Document, vis: &[Vis], ppp: f32) -> Option<Rect> {
    let (l, c) = v.sel.head;
    let x = vis.iter().find(|x| x.view.line == l)?;
    let lv = &x.view;
    let ltext = doc.line(l);
    let mut ch = lv.ch(ltext, c);
    // With a preedit the caret sits after it (col_to_char already skips it); keep it inside the mapped text.
    ch = ch.min(lv.disp.mapped_chars);
    let r = lv.row_of(ch, v.up);
    let dx = v.geo.text.left() - if lay.wrap { 0.0 } else { v.scroll_x };
    let cx = snap(dx + lv.x_in_row(r, ch), ppp);
    let w = ((1.5 * ppp).round().max(1.0)) / ppp;
    let gh = lv.galley.rows.first().map_or(lay.row_h, |r| r.height());
    let h = (gh + 2.0).min(lay.row_h);
    let ry = snap(x.y, ppp) + r as f32 * lay.row_h;
    let top = snap(ry + (lay.row_h - h) * 0.5, ppp);
    Some(Rect::from_min_size(Pos2::new(cx - snap(w * 0.5, ppp), top), Vec2::new(w, snap(h, ppp))))
}

/// Vertical (line-based) and horizontal scrollbars: thin, draggable, click on the track pages.
pub fn scrollbars(ui: &mut egui::Ui, doc: &Document, v: &mut View, lay: &Lay<'_>, _resp: &Response, toks: &mut Vec<Token>, id: egui::Id) {
    let c = &lay.theme.code;
    let geo = v.geo;
    let rect = geo.rect;
    let n = doc.line_count();
    let view_lines = (geo.view_h / lay.row_h).max(1.0);
    let max_pos = n.saturating_sub(1) as f32;
    let track = Rect::from_min_max(Pos2::new(geo.text.right(), rect.top()), rect.max);
    if (n as f32) > view_lines * 0.5 || v.top_line > 0 {
        let th = lay.text_h(doc, v.top_line, toks).max(1.0);
        let pos = v.top_line as f32 + (v.top_off / th).clamp(0.0, 1.0);
        let thumb_h = (track.height() * view_lines / (max_pos + view_lines)).clamp(24.0_f32.min(track.height()), track.height());
        let free = (track.height() - thumb_h).max(0.0);
        let t = if max_pos > 0.0 { (pos / max_pos).clamp(0.0, 1.0) } else { 0.0 };
        let thumb = Rect::from_min_size(Pos2::new(track.left(), track.top() + free * t), Vec2::new(track.width(), thumb_h));
        let r = ui.interact(track, id.with("vsb"), Sense::click_and_drag());
        let p = r.interact_pointer_pos();
        if r.drag_started() {
            if let Some(p) = p {
                v.sb_drag = Some(if thumb.contains(p) { p.y - thumb.top() } else { thumb_h * 0.5 });
            }
        }
        if r.dragged() {
            if let (Some(grab), Some(p)) = (v.sb_drag, p) {
                let t = if free > 0.0 { ((p.y - grab - track.top()) / free).clamp(0.0, 1.0) } else { 0.0 };
                let pos = t * max_pos;
                let line = (pos.floor() as u32).min(n.saturating_sub(1));
                v.top_line = line;
                v.top_off = pos.fract() * lay.text_h(doc, line, toks);
                v.clamp_end(lay, doc, toks);
            }
        } else if r.clicked() {
            if let Some(p) = p {
                if !thumb.contains(p) {
                    let dy = if p.y < thumb.top() { -geo.view_h } else { geo.view_h } * 0.9;
                    v.scroll_by(lay, doc, dy, toks);
                }
            }
        }
        if !r.dragged() {
            v.sb_drag = None;
        }
        let hot = r.hovered() || r.dragged();
        let w = if hot { 8.0 } else { 6.0 };
        let col = if hot { mix(c.scrollbar, c.gutter_text_active, 0.25) } else { c.scrollbar };
        let tr = Rect::from_min_max(Pos2::new(track.right() - w - 2.0, thumb.top() + 2.0), Pos2::new(track.right() - 2.0, thumb.bottom() - 2.0));
        ui.painter().rect_filled(tr, CornerRadius::same(3), col);
    }
    // Horizontal.
    let tw = geo.text.width();
    if !lay.wrap && v.widest > tw {
        let track = Rect::from_min_max(Pos2::new(geo.text.left(), rect.bottom() - 8.0), Pos2::new(geo.text.right(), rect.bottom()));
        let total = v.widest + geo.char_w * 4.0;
        let thumb_w = (track.width() * tw / total).clamp(24.0_f32.min(track.width()), track.width());
        let free = (track.width() - thumb_w).max(0.0);
        let max_x = (total - tw).max(1.0);
        let t = (v.scroll_x / max_x).clamp(0.0, 1.0);
        let thumb = Rect::from_min_size(Pos2::new(track.left() + free * t, track.top()), Vec2::new(thumb_w, track.height()));
        let r = ui.interact(track, id.with("hsb"), Sense::click_and_drag());
        let p = r.interact_pointer_pos();
        if r.drag_started() {
            if let Some(p) = p {
                v.hsb_drag = Some(if thumb.contains(p) { p.x - thumb.left() } else { thumb_w * 0.5 });
            }
        }
        if r.dragged() {
            if let (Some(grab), Some(p)) = (v.hsb_drag, p) {
                let t = if free > 0.0 { ((p.x - grab - track.left()) / free).clamp(0.0, 1.0) } else { 0.0 };
                v.scroll_x = t * max_x;
            }
        } else {
            v.hsb_drag = None;
        }
        let hot = r.hovered() || r.dragged();
        let col = if hot { mix(c.scrollbar, c.gutter_text_active, 0.25) } else { c.scrollbar };
        let h = if hot { 7.0 } else { 5.0 };
        let tr = Rect::from_min_max(Pos2::new(thumb.left() + 2.0, track.bottom() - h - 1.0), Pos2::new(thumb.right() - 2.0, track.bottom() - 1.0));
        ui.painter().rect_filled(tr, CornerRadius::same(3), col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use margin_code::Lang;

    #[test]
    fn brackets() {
        let d = Document::new("fn x() {\n  (a[1])\n}", Lang::Plain);
        assert_eq!(match_bracket(&d, (0, 8)), Some(((0, 7), (2, 0))));
        assert_eq!(match_bracket(&d, (2, 0)), Some(((2, 0), (0, 7))));
        assert_eq!(match_bracket(&d, (1, 2)), Some(((1, 2), (1, 7))));
        assert_eq!(match_bracket(&d, (1, 7)), Some(((1, 6), (1, 4))));
        assert_eq!(match_bracket(&d, (0, 1)), None);
        let d = Document::new("((((", Lang::Plain);
        assert_eq!(match_bracket(&d, (0, 1)), None);
    }
}
