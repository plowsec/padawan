//! Preview frame: virtualized scrolling over leaves, container decorations, input.
//!
//! | Piece | How |
//! |---|---|
//! | position | anchor `(top leaf, offset in its slot)`: stable when estimates are refined |
//! | heights | estimated from text size at sync; exact once laid out (visible or scrolled through) |
//! | cache | layout per leaf hash (survives re-parse), cleared on theme / width / scale change |
//! | scrollbar | proportional to the estimated total; drag maps to a position via prefix sums |
//! | input | wheel (vertical; horizontal over a wide table), link / checkbox clicks, scrollbar drag |
//!
//! Slot of a leaf = gap above + content height. Never opens anything: clicks are returned.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{pos2, vec2, CursorIcon, Painter, Rect, Sense, Stroke, StrokeKind, Vec2};
use margin_theme::{mix, Theme};

use crate::doc::{Body, CtrKind, Inner, Leaf, TextKind, NONE};
use crate::lay::{self, Cols, HitRec, Laid, Lx, Sty, COL_MAX, PAD_X, SB_W};
use crate::PreviewOutput;

/// What a clickable area does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hit {
    /// Link destination as written.
    Link(String),
    /// Task item: source line.
    Task(u32),
    /// Footnote reference: number (scrolls inside the preview).
    Footnote(u32),
}

#[derive(Clone, Default)]
pub struct View {
    pub pending: Option<u32>,
    pub hits: Vec<(Rect, Hit)>,
    doc_id: u64,
    key: Option<(Theme, f32, f32)>,
    sty: Option<Arc<Sty>>,
    col_w: f32,
    h: Vec<f32>,
    gap: Vec<f32>,
    tops: Vec<f64>,
    tops_dirty: bool,
    top: usize,
    off: f32,
    last_top_line: u32,
    cache: HashMap<u64, (Arc<Laid>, u64)>,
    known: HashMap<u64, f32>,
    cols: HashMap<u32, Arc<Cols>>,
    table_x: HashMap<u32, f32>,
    table_rects: Vec<(Rect, u32, f32)>,
    frame: u64,
    drag: Option<f32>,
}

impl std::fmt::Debug for View {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewState").field("doc", &self.doc_id).field("top", &self.top).field("off", &self.off).field("cached", &self.cache.len()).finish()
    }
}

fn digits(mut n: u64) -> f32 {
    let mut d = 1.0;
    while n >= 10 {
        n /= 10;
        d += 1.0;
    }
    d
}

/// Width of a list item's marker gutter.
fn gutter(sty: &Sty, doc: &Inner, item_parent: u32) -> f32 {
    match doc.ctr(item_parent).map(|c| c.kind) {
        Some(CtrKind::List { ordered: true, max_num, .. }) => sty.bullet_w.max((sty.digit_w * (digits(max_num) + 0.6) + 10.0).round()),
        _ => sty.bullet_w,
    }
}

fn indent(sty: &Sty, doc: &Inner, ctr: u32) -> (f32, bool) {
    let mut x = 0.0;
    let mut muted = false;
    let mut c = ctr;
    let mut n = 0;
    let mut innermost_quote = true;
    while c != NONE && n < 64 {
        let Some(k) = doc.ctr(c) else { break };
        match k.kind {
            CtrKind::Quote(a) => {
                x += sty.quote_w;
                if innermost_quote {
                    muted = a.is_none();
                    innermost_quote = false;
                }
            }
            CtrKind::Item { .. } => x += gutter(sty, doc, k.parent),
            CtrKind::Foot { .. } => x += sty.bullet_w,
            CtrKind::List { .. } => {}
        }
        c = k.parent;
        n += 1;
    }
    (x, muted)
}

fn estimate(sty: &Sty, leaf: &Leaf, w: f32) -> f32 {
    let w = w.max(40.0);
    match &leaf.body {
        Body::Text { kind, .. } => {
            let (size, lh, extra) = match kind {
                TextKind::Heading(l) => {
                    let s = sty.heading_size(*l);
                    (s, (s * 1.3).round(), if *l <= 2 { (sty.b * 0.4).round() + 1.0 } else { 0.0 })
                }
                _ => (sty.b, sty.lh, 0.0),
            };
            let per_line = (w / (sty.avg_w * size / sty.b)).max(1.0);
            let lines = (leaf.weight as f32 / per_line).ceil().max(1.0);
            lines * lh + extra
        }
        Body::Code { first, last, lines, .. } => {
            let per_line = ((w - 2.0 * sty.code_px) / sty.code_avg_w).max(8.0);
            let rows: f32 = lines.iter().map(|l| (l.chars().count() as f32 / per_line).ceil().max(1.0)).sum::<f32>().max(1.0);
            rows * sty.code_lh + if *first { sty.code_py } else { 0.0 } + if *last { sty.code_py } else { 0.0 }
        }
        Body::Table { rows, .. } => rows.len() as f32 * (((sty.b - 1.0) * 1.45).round() + 2.0 * sty.cell_py),
        Body::Rule => 1.0,
    }
}

impl View {
    fn slot(&self, i: usize) -> f32 {
        self.gap.get(i).copied().unwrap_or(0.0) + self.h.get(i).copied().unwrap_or(0.0)
    }

    fn sync(&mut self, ctx: &egui::Context, doc: &Inner, theme: &Theme, col_w: f32, ppp: f32) {
        let key = (*theme, col_w, ppp);
        if self.key != Some(key) {
            self.cache.clear();
            self.known.clear();
            self.cols.clear();
            self.key = Some(key);
            self.sty = Some(Arc::new(Sty::new(theme, ctx)));
            self.col_w = col_w;
            if self.doc_id == doc.id {
                self.doc_id = u64::MAX;
            }
        }
        if self.doc_id == doc.id {
            return;
        }
        let Some(sty) = self.sty.clone() else { return };
        let had = self.doc_id != 0 && !self.h.is_empty();
        let n = doc.leaves.len();
        self.h.clear();
        self.gap.clear();
        self.h.reserve(n);
        self.gap.reserve(n);
        for l in &doc.leaves {
            let h = match self.known.get(&l.hash) {
                Some(&h) => h,
                None => estimate(&sty, l, col_w - indent(&sty, doc, l.ctr).0),
            };
            self.h.push(h);
            self.gap.push(sty.gap(l.gap));
        }
        if had && n > 0 {
            self.top = doc.leaf_at_line(self.last_top_line).min(n - 1);
        } else {
            self.top = 0;
            self.off = 0.0;
        }
        self.cols.clear();
        self.doc_id = doc.id;
        self.tops_dirty = true;
        self.clamp(n);
    }

    fn set_h(&mut self, i: usize, h: f32) {
        if let Some(x) = self.h.get_mut(i) {
            if (*x - h).abs() > 0.01 {
                *x = h;
                self.tops_dirty = true;
            }
        }
    }

    fn laid(&mut self, lx: &Lx<'_>, i: usize) -> Option<Arc<Laid>> {
        let leaf = lx.doc.leaves.get(i)?;
        let frame = self.frame;
        if let Some((l, used)) = self.cache.get_mut(&leaf.hash) {
            *used = frame;
            let l = l.clone();
            self.set_h(i, l.h());
            return Some(l);
        }
        let (ind, muted) = indent(lx.sty, lx.doc, leaf.ctr);
        let w = (self.col_w - ind).max(40.0);
        let cols = match leaf.body {
            Body::Table { tid, .. } => Some(match self.cols.get(&tid) {
                Some(c) => c.clone(),
                None => {
                    let c = Arc::new(lay::table_cols(lx, tid, w));
                    self.cols.insert(tid, c.clone());
                    c
                }
            }),
            _ => None,
        };
        let l = Arc::new(lay::layout(lx, leaf, w, cols, muted));
        if self.known.len() > 1_000_000 {
            self.known.clear();
        }
        self.known.insert(leaf.hash, l.h());
        self.cache.insert(leaf.hash, (l.clone(), frame));
        self.set_h(i, l.h());
        Some(l)
    }

    fn rebuild_tops(&mut self) {
        let n = self.h.len();
        if !self.tops_dirty && self.tops.len() == n + 1 {
            return;
        }
        self.tops.clear();
        self.tops.reserve(n + 1);
        let mut y = 0.0f64;
        self.tops.push(0.0);
        for i in 0..n {
            y += f64::from(self.slot(i));
            self.tops.push(y);
        }
        self.tops_dirty = false;
    }

    fn max_off_last(&self, n: usize) -> f32 {
        if n == 0 {
            0.0
        } else {
            (self.slot(n - 1) - 48.0).max(0.0)
        }
    }

    fn clamp(&mut self, n: usize) {
        if n == 0 {
            self.top = 0;
            self.off = 0.0;
            return;
        }
        if self.top >= n {
            self.top = n - 1;
            self.off = 0.0;
        }
        if !self.off.is_finite() || self.off < 0.0 {
            self.off = 0.0;
        }
        while self.top + 1 < n && self.off >= self.slot(self.top) {
            self.off -= self.slot(self.top);
            self.top += 1;
        }
        if self.top == n - 1 {
            self.off = self.off.min(self.max_off_last(n));
        }
    }

    /// Scroll by `dy` points, laying out the leaves passed (exact heights, no jumps).
    fn scroll_by(&mut self, lx: &Lx<'_>, dy: f32) {
        let n = lx.doc.leaves.len();
        if n == 0 || !dy.is_finite() {
            return;
        }
        self.off += dy;
        let mut guard = 0;
        while self.off < 0.0 && self.top > 0 && guard < 100_000 {
            self.top -= 1;
            self.laid(lx, self.top);
            self.off += self.slot(self.top);
            guard += 1;
        }
        if self.off < 0.0 {
            self.off = 0.0;
        }
        while self.top + 1 < n && guard < 200_000 {
            self.laid(lx, self.top);
            let s = self.slot(self.top);
            if self.off < s {
                break;
            }
            self.off -= s;
            self.top += 1;
            guard += 1;
        }
        self.clamp(n);
    }

    fn pos(&mut self) -> f64 {
        self.rebuild_tops();
        self.tops.get(self.top).copied().unwrap_or(0.0) + f64::from(self.off)
    }

    fn set_pos(&mut self, p: f64, n: usize) {
        self.rebuild_tops();
        if n == 0 {
            return;
        }
        let i = self.tops.get(..n).map_or(0, |t| t.partition_point(|&t| t <= p)).saturating_sub(1);
        self.top = i;
        self.off = (p - self.tops.get(i).copied().unwrap_or(0.0)).max(0.0) as f32;
        self.clamp(n);
    }

    fn go_to_leaf(&mut self, i: usize, n: usize) {
        if i < n {
            self.top = i;
            self.off = (self.gap.get(i).copied().unwrap_or(0.0) - 12.0).max(0.0);
            self.clamp(n);
        }
    }
}

/// Container decorations of leaf `i`: quote bars, list markers, checkboxes, footnote labels.
#[allow(clippy::too_many_arguments)]
fn paint_ctrs(p: &Painter, lx: &Lx<'_>, i: usize, chain: &[u32], prev: &[u32], col_left: f32, slot_top: f32, ctop: f32, cbot: f32, first: f32, hits: &mut Vec<HitRec>) {
    let (th, sty, doc) = (lx.th, lx.sty, lx.doc);
    let mut x = col_left;
    let mut lists = 0usize;
    let ppp = p.pixels_per_point();
    for &c in chain.iter().rev() {
        let Some(k) = doc.ctr(c) else { continue };
        let is_first = k.first_leaf as usize == i;
        match k.kind {
            CtrKind::Quote(a) => {
                let t = if prev.contains(&c) { slot_top } else { ctop };
                let color = a.map_or(mix(th.ui.border, th.ui.text_faint, 0.6), |a| lay::alert_color(th, a));
                let x0 = (x * ppp).round() / ppp;
                p.rect_filled(Rect::from_min_max(pos2(x0, t), pos2(x0 + 3.0, cbot)), 0.0, color);
                x += sty.quote_w;
            }
            CtrKind::List { .. } => lists += 1,
            CtrKind::Item { task, num, line } => {
                let g = gutter(sty, doc, k.parent);
                if is_first {
                    let cy = ctop + first;
                    let ordered = matches!(doc.ctr(k.parent).map(|x| x.kind), Some(CtrKind::List { ordered: true, .. }));
                    if let Some(checked) = task {
                        let s = (sty.b * 0.95).round();
                        let r = Rect::from_min_size(pos2((x + g - s - 8.0).round(), (cy - s / 2.0).round()), vec2(s, s));
                        let hovered = p.ctx().pointer_hover_pos().is_some_and(|pp| r.expand(3.0).contains(pp));
                        if checked {
                            p.rect_filled(r, 3.0, th.ui.accent);
                            let a = pos2(r.left() + s * 0.24, r.top() + s * 0.52);
                            let b = pos2(r.left() + s * 0.42, r.top() + s * 0.70);
                            let cc = pos2(r.left() + s * 0.77, r.top() + s * 0.32);
                            p.line_segment([a, b], Stroke::new(1.6, th.ui.on_accent));
                            p.line_segment([b, cc], Stroke::new(1.6, th.ui.on_accent));
                        } else {
                            let stroke = if hovered { th.ui.accent } else { th.ui.text_muted };
                            p.rect(r, 3.0, th.ui.input_bg, Stroke::new(1.0, stroke), StrokeKind::Inside);
                        }
                        let kk = hits.iter().rev().take_while(|h| h.leaf == i as u32).count() as u32;
                        hits.push(HitRec { rect: r.expand(3.0), link: NONE, task: line, base: 0.0, leaf: i as u32, k: kk });
                    } else if ordered {
                        let font = egui::FontId::proportional(sty.b);
                        let g2 = lx.ctx.fonts_mut(|f| f.layout_no_wrap(format!("{num}."), font, th.ui.text_muted));
                        let pos = pos2((x + g - 7.0 - g2.size().x).round(), (cy - g2.size().y / 2.0).round());
                        p.galley(pos, g2, th.ui.text_muted);
                    } else {
                        let cx = (x + g * 0.42).round() + 0.5;
                        let col = th.ui.text_muted;
                        match lists.saturating_sub(1) {
                            0 => {
                                p.circle_filled(pos2(cx, cy), 2.6, col);
                            }
                            1 => {
                                p.circle_stroke(pos2(cx, cy), 2.4, Stroke::new(1.1, col));
                            }
                            _ => {
                                p.rect_filled(Rect::from_center_size(pos2(cx, cy), vec2(4.4, 4.4)), 0.0, col);
                            }
                        }
                    }
                }
                x += g;
            }
            CtrKind::Foot { num } => {
                if is_first {
                    let cy = ctop + first;
                    let font = egui::FontId::proportional(sty.small);
                    let g2 = lx.ctx.fonts_mut(|f| f.layout_no_wrap(format!("{num}."), font, th.ui.text_muted));
                    p.galley(pos2((x + sty.bullet_w - 8.0 - g2.size().x).round(), (cy - g2.size().y / 2.0).round()), g2, th.ui.text_muted);
                }
                x += sty.bullet_w;
            }
        }
    }
}

pub fn show(ui: &mut egui::Ui, doc: &Inner, v: &mut View, theme: &Theme) -> PreviewOutput {
    let rect = ui.available_rect_before_wrap();
    let id = ui.scope_id().with("margin-md-preview");
    let _ = ui.allocate_rect(rect, Sense::hover());
    let ctx = ui.ctx().clone();
    let ppp = ctx.pixels_per_point();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme.code.bg);
    let col_w = (rect.width() - 2.0 * PAD_X - SB_W).clamp(80.0, COL_MAX).round();
    v.frame += 1;
    v.sync(&ctx, doc, theme, col_w, ppp);
    let mut out = PreviewOutput::default();
    let Some(sty) = v.sty.clone() else { return out };
    let lx = Lx { ctx: &ctx, doc, th: theme, sty: &sty };
    let n = doc.leaves.len();

    if let Some(l) = v.pending.take() {
        if n > 0 {
            v.go_to_leaf(doc.leaf_at_line(l), n);
        }
    }

    // Wheel.
    if ui.rect_contains_pointer(rect) {
        let wheel = ui.input(|i| i.smooth_scroll_delta);
        if wheel != Vec2::ZERO {
            if wheel.x != 0.0 {
                if let Some(pp) = ui.input(|i| i.pointer.hover_pos()) {
                    for (r, tid, maxx) in &v.table_rects {
                        if r.contains(pp) {
                            let x = v.table_x.entry(*tid).or_default();
                            *x = (*x - wheel.x).clamp(0.0, *maxx);
                        }
                    }
                }
            }
            if wheel.y != 0.0 {
                v.scroll_by(&lx, -wheel.y);
            }
            ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        }
    }

    // Scrollbar drag (before painting: the frame shows the new position).
    let track = Rect::from_min_max(pos2(rect.right() - SB_W, rect.top()), rect.right_bottom());
    let view_h = rect.height();
    let thumb = |v: &mut View| -> Option<(f32, f32, f64)> {
        v.rebuild_tops();
        let max_p = v.tops.get(n.saturating_sub(1)).copied().unwrap_or(0.0) + f64::from(v.max_off_last(n));
        if n == 0 || max_p < 1.0 {
            return None;
        }
        let content = max_p as f32 + view_h;
        let th = (view_h / content * view_h).clamp(28.0_f32.min(view_h), view_h);
        let p = v.pos();
        let ty = rect.top() + ((p / max_p) as f32).clamp(0.0, 1.0) * (view_h - th);
        Some((ty, th, max_p))
    };
    if let Some((ty, th, max_p)) = thumb(v) {
        let sb = ui.interact(track, id.with("sb"), Sense::click_and_drag());
        if let Some(pp) = sb.interact_pointer_pos() {
            if sb.drag_started() || sb.clicked() || (sb.is_pointer_button_down_on() && v.drag.is_none()) {
                v.drag = Some(if pp.y >= ty && pp.y <= ty + th { pp.y - ty } else { th / 2.0 });
            }
            if let Some(g) = v.drag {
                if sb.dragged() || sb.drag_started() || sb.clicked() || sb.is_pointer_button_down_on() {
                    let frac = ((pp.y - g - rect.top()) / (view_h - th).max(1.0)).clamp(0.0, 1.0);
                    v.set_pos(f64::from(frac) * max_p, n);
                }
            }
        }
        if sb.drag_stopped() || !sb.is_pointer_button_down_on() {
            v.drag = None;
        }
    }
    v.clamp(n);

    // Visible leaves.
    let mut recs: Vec<HitRec> = Vec::new();
    v.table_rects.clear();
    let col_left = rect.left() + PAD_X;
    let mut y = rect.top() - v.off;
    let mut i = v.top;
    let mut chain = Vec::new();
    let mut prev = Vec::new();
    if i > 0 {
        if let Some(l) = doc.leaves.get(i - 1) {
            doc.chain(l.ctr, &mut prev);
        }
    }
    let clip = rect;
    while i < n && y < rect.bottom() {
        let Some(laid) = v.laid(&lx, i) else { break };
        let leaf = &doc.leaves[i];
        let gap = v.gap.get(i).copied().unwrap_or(0.0);
        let ctop = (y + gap).round();
        let h = laid.h();
        doc.chain(leaf.ctr, &mut chain);
        let (ind, muted) = indent(&sty, doc, leaf.ctr);
        if ctop + h >= rect.top() - 2.0 {
            paint_ctrs(&painter, &lx, i, &chain, &prev, col_left, y, ctop, ctop + h, laid.first(), &mut recs);
            let x = col_left + ind;
            let w = (col_w - ind).max(40.0);
            match (&*laid, &leaf.body) {
                (Laid::Text { t, rule, .. }, Body::Text { kind, .. }) => {
                    let color = lay::text_style(&lx, *kind, muted).color;
                    lay::paint_text(&painter, theme, t, pos2(x, ctop), clip, i as u32, &mut recs, color);
                    if *rule {
                        painter.hline(x..=x + w, ((ctop + h - 1.0) * ppp).round() / ppp + 0.5 / ppp, Stroke::new(1.0, theme.ui.border));
                    }
                }
                (Laid::Code { .. }, body) => lay::paint_code(&painter, theme, &sty, &laid, body, x, ctop, w),
                (Laid::Table { cols, .. }, body @ Body::Table { tid, .. }) => {
                    let maxx = (cols.total - w).max(0.0);
                    let sx = v.table_x.get(tid).copied().unwrap_or(0.0).min(maxx);
                    if maxx > 0.0 {
                        v.table_rects.push((Rect::from_min_size(pos2(x, ctop), vec2(w, h)), *tid, maxx));
                    }
                    lay::paint_table(&painter, &lx, &laid, body, x, ctop, w, sx, i as u32, &mut recs);
                }
                (Laid::Rule { .. }, _) => {
                    painter.hline(x..=x + w, (ctop * ppp).round() / ppp + 0.5 / ppp, Stroke::new(1.0, theme.ui.border));
                }
                _ => {}
            }
        }
        y = ctop + h;
        i += 1;
        std::mem::swap(&mut chain, &mut prev);
    }

    // Clicks, hover, tooltips.
    let mut hovered_link: Option<(u32, u32)> = None;
    let mut foot_jump: Option<u32> = None;
    v.hits.clear();
    for r in &recs {
        let area = r.rect.intersect(rect);
        if !area.is_positive() {
            continue;
        }
        let resp = ui.interact(area, id.with(("hit", r.leaf, r.k)), Sense::click());
        if r.task != NONE {
            v.hits.push((area, Hit::Task(r.task)));
            if resp.hovered() {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
            if resp.clicked() {
                out.task_toggled = Some(r.task);
            }
            continue;
        }
        let Some(link) = doc.links.get(r.link as usize) else { continue };
        let hit = if link.foot != NONE { Hit::Footnote(link.foot) } else { Hit::Link(link.dest.clone()) };
        v.hits.push((area, hit));
        if resp.hovered() {
            hovered_link = Some((r.leaf, r.link));
            ctx.set_cursor_icon(CursorIcon::PointingHand);
        }
        if resp.clicked() {
            if link.foot != NONE {
                foot_jump = Some(link.foot);
            } else {
                out.link_clicked = Some(link.dest.clone());
            }
        }
        if link.foot == NONE {
            let mut tip = link.shown.clone();
            if !link.title.is_empty() {
                tip.push('\n');
                tip.push_str(&link.title);
            }
            resp.on_hover_text_at_pointer(tip);
        }
    }
    if let Some((leaf, link)) = hovered_link {
        for r in recs.iter().filter(|r| r.leaf == leaf && r.link == link) {
            painter.hline(r.rect.left()..=r.rect.right(), (r.base * ppp).round() / ppp + 0.5 / ppp, Stroke::new(1.0, theme.ui.accent));
        }
    }
    if let Some(num) = foot_jump {
        if let Some(&(_, leaf)) = doc.foot_leaf.iter().find(|(f, _)| *f == num) {
            v.go_to_leaf(leaf as usize, n);
            ctx.request_repaint();
        }
    }

    // Scrollbar.
    if let Some((ty, th, _)) = thumb(v) {
        let active = v.drag.is_some() || ui.rect_contains_pointer(track);
        let c = if active { mix(theme.code.scrollbar, theme.ui.text_muted, 0.35) } else { theme.code.scrollbar };
        painter.rect_filled(Rect::from_min_max(pos2(track.right() - 7.0, ty + 2.0), pos2(track.right() - 2.0, ty + th - 2.0)), 3.0, c);
    }

    out.top_line = doc.leaves.get(v.top).map_or(0, |l| l.line);
    v.last_top_line = out.top_line;
    if v.cache.len() > 600 {
        let f = v.frame;
        v.cache.retain(|_, (_, used)| f.saturating_sub(*used) < 3);
    }
    out
}

