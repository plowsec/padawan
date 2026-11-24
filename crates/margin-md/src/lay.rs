//! Layout and paint of one leaf. Layout is pure (galleys + decoration rects relative to the leaf);
//! the view caches it per leaf hash and paints it at a position.
//!
//! | Leaf | Look |
//! |---|---|
//! | paragraph | body font (ui size + 2), line height 1.6 |
//! | heading | 1.8 / 1.45 / 1.2 / 1.0× body, weight 650, 1 px rule under h1/h2 |
//! | code | mono, panel fill, 1 px border, radius 5, soft-wrapped, `lex_line` colours, label top-right |
//! | table | header fill, 1 px grid, padded cells, aligned, widths from content, horizontal scroll |
//! | rule | 1 px line |

use std::sync::Arc;

use egui::epaint::text::VariationCoords;
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{pos2, vec2, Color32, CornerRadius, FontFamily, FontId, Galley, Painter, Pos2, Rect, Stroke, StrokeKind};
use margin_code::{lex_line, LineState, TokenKind};
use margin_theme::{mix, Theme};

use crate::doc::{f, Alert, Align, Body, Inner, Leaf, Span, TextKind, NONE};
use crate::sel::{Area, P};

/// Content column: max width, side padding.
pub const COL_MAX: f32 = 780.0;
pub const PAD_X: f32 = 28.0;
pub const SB_W: f32 = 10.0;

/// Sizes derived from the theme (pt).
#[derive(Clone, Debug)]
pub struct Sty {
    pub b: f32,
    pub lh: f32,
    pub code: f32,
    pub code_lh: f32,
    pub small: f32,
    pub pad_top: f32,
    pub quote_w: f32,
    pub bullet_w: f32,
    pub digit_w: f32,
    pub avg_w: f32,
    pub code_avg_w: f32,
    pub cell_px: f32,
    pub cell_py: f32,
    pub code_px: f32,
    pub code_py: f32,
}

impl Sty {
    pub fn new(th: &Theme, ctx: &egui::Context) -> Sty {
        let b = (th.look.ui_size + 2.0).round();
        let code = (th.look.code_size - 0.5).max(8.0);
        let (digit_w, avg_w, code_avg_w) = ctx.fonts_mut(|f| {
            let fid = FontId::proportional(b);
            let d = f.glyph_width(&fid, '0');
            let sample = "the quick brown fox jumps over a lazy dog";
            let a = sample.chars().map(|c| f.glyph_width(&fid, c)).sum::<f32>() / sample.len() as f32;
            (d, a, f.glyph_width(&FontId::monospace(code), '0'))
        });
        Sty {
            b,
            lh: (b * 1.6).round(),
            code,
            code_lh: (code * 1.55).round(),
            small: (b - 3.0).max(9.0),
            pad_top: 26.0,
            quote_w: (b * 1.1).round(),
            bullet_w: (b * 1.6).round(),
            digit_w: digit_w.max(1.0),
            avg_w: avg_w.max(1.0),
            code_avg_w: code_avg_w.max(1.0),
            cell_px: (b * 0.75).round(),
            cell_py: (b * 0.4).round(),
            code_px: (b * 0.95).round(),
            code_py: (b * 0.75).round(),
        }
    }

    pub fn heading_size(&self, l: u8) -> f32 {
        let k = match l {
            1 => 1.8,
            2 => 1.45,
            3 => 1.2,
            4 => 1.0,
            5 => 0.93,
            _ => 0.87,
        };
        (self.b * k).round()
    }

    /// Margins (top, bottom) of a block class; the gap between two blocks is the larger of the two.
    pub fn margins(&self, c: crate::doc::Class) -> (f32, f32) {
        use crate::doc::Class;
        let b = self.b;
        match c {
            Class::Para => (0.85 * b, 0.85 * b),
            Class::H(1) => (1.7 * b, 0.75 * b),
            Class::H(2) => (1.6 * b, 0.7 * b),
            Class::H(3) => (1.35 * b, 0.6 * b),
            Class::H(_) => (1.2 * b, 0.5 * b),
            Class::Block => (1.0 * b, 1.0 * b),
            Class::Rule => (1.4 * b, 1.4 * b),
            Class::Title => (0.85 * b, 0.25 * b),
        }
    }

    pub fn gap(&self, g: crate::doc::Gap) -> f32 {
        use crate::doc::Gap;
        match g {
            Gap::Start => self.pad_top,
            Gap::Cont => 0.0,
            Gap::Tight => (0.2 * self.b).round(),
            // Alert title hugs its body.
            Gap::Normal { prev: crate::doc::Class::Title, .. } => self.margins(crate::doc::Class::Title).1.round(),
            Gap::Normal { prev, cur } => self.margins(prev).1.max(self.margins(cur).0).round(),
        }
    }
}

pub fn alert_color(th: &Theme, a: Alert) -> Color32 {
    match a {
        Alert::Note => th.syn.link,
        Alert::Important => th.ui.accent,
        Alert::Tip => th.ui.ok,
        Alert::Warning => th.ui.warning,
        Alert::Caution => th.ui.error,
    }
}

pub struct Lx<'a> {
    pub ctx: &'a egui::Context,
    pub doc: &'a Inner,
    pub th: &'a Theme,
    pub sty: &'a Sty,
}

#[derive(Clone, Copy, Debug)]
pub enum Deco {
    Code(Rect),
    Image(Rect),
    Link { rect: Rect, link: u32, base: f32 },
}

#[derive(Clone, Debug)]
pub struct TextLaid {
    pub galley: Arc<Galley>,
    pub deco: Vec<Deco>,
}

#[derive(Clone, Debug)]
pub struct TRow {
    pub y: f32,
    pub h: f32,
    pub header: bool,
    pub cells: Vec<TextLaid>,
}

#[derive(Clone, Debug)]
pub struct Cols {
    pub w: Vec<f32>,
    pub total: f32,
}

#[derive(Clone, Debug)]
pub enum Laid {
    Text { t: TextLaid, h: f32, first: f32, rule: bool },
    Code { g: Arc<Galley>, label: Option<Arc<Galley>>, top: f32, h: f32, first: f32 },
    Table { rows: Vec<TRow>, cols: Arc<Cols>, h: f32, first: f32, last: bool },
    Rule { h: f32 },
}

impl Laid {
    pub fn h(&self) -> f32 {
        match self {
            Laid::Text { h, .. } | Laid::Code { h, .. } | Laid::Table { h, .. } | Laid::Rule { h } => *h,
        }
    }

    /// Vertical centre of the first line (list markers align to it).
    pub fn first(&self) -> f32 {
        match self {
            Laid::Text { first, .. } | Laid::Code { first, .. } | Laid::Table { first, .. } => *first,
            Laid::Rule { h } => h / 2.0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct TStyle {
    pub size: f32,
    pub color: Color32,
    pub strong: bool,
    pub lh: f32,
}

fn wght(w: f32) -> VariationCoords {
    VariationCoords::new([(b"wght", w)])
}

fn fmt(th: &Theme, ts: TStyle, fl: u8, linked: bool) -> TextFormat {
    let mut tf = TextFormat { line_height: Some(ts.lh), color: ts.color, ..Default::default() };
    let mut size = ts.size;
    let mut fam = FontFamily::Proportional;
    if fl & f::CODE != 0 {
        fam = FontFamily::Monospace;
        size = (ts.size * 0.86).round();
        if ts.color == th.ui.text {
            tf.color = th.code.text;
        }
    }
    if fl & (f::IMAGE | f::HTML) != 0 {
        tf.color = th.ui.text_muted;
        if fl & f::IMAGE != 0 {
            size = (ts.size * 0.9).round();
        }
    }
    if fl & f::STRONG != 0 || ts.strong {
        tf.coords = wght(650.0);
        if tf.color == th.ui.text {
            tf.color = th.ui.text_strong;
        }
    }
    if linked {
        tf.color = th.ui.accent;
    }
    if fl & f::FOOT != 0 {
        size = (ts.size * 0.72).round();
        tf.color = th.ui.accent;
        tf.valign = egui::Align::TOP;
        tf.line_height = Some((ts.lh * 0.8).round());
    }
    if fl & f::EM != 0 {
        tf.italics = true;
    }
    if fl & f::STRIKE != 0 {
        tf.strikethrough = Stroke::new(1.0, tf.color);
    }
    tf.font_id = FontId::new(size, fam);
    tf
}

#[derive(Clone, Copy, PartialEq)]
enum DK {
    Code,
    Image,
    Link(u32),
}

/// Spans → galley + decoration rects.
pub fn text_laid(lx: &Lx<'_>, spans: &[Span], ts: TStyle, w: f32, fallback: &str) -> TextLaid {
    let th = lx.th;
    let mut job = LayoutJob { wrap: TextWrapping { max_width: w.max(1.0), ..Default::default() }, break_on_newline: true, ..Default::default() };
    let mut ranges: Vec<(usize, usize, DK)> = Vec::new();
    let mut chars = 0usize;
    let mut owned = String::new();
    for sp in spans {
        let linked = sp.link != NONE;
        let text: &str = crate::sel::shown(lx.doc, sp, &mut owned);
        if text.is_empty() {
            continue;
        }
        let n = text.chars().count();
        if sp.f & f::IMAGE != 0 {
            ranges.push((chars, chars + n, DK::Image));
        } else if sp.f & f::CODE != 0 {
            ranges.push((chars, chars + n, DK::Code));
        }
        if linked {
            ranges.push((chars, chars + n, DK::Link(sp.link)));
        }
        chars += n;
        job.append(text, 0.0, fmt(th, ts, sp.f, linked));
    }
    if job.text.is_empty() {
        job.append(if fallback.is_empty() { " " } else { fallback }, 0.0, fmt(th, ts, 0, false));
    }
    let galley = lx.ctx.fonts_mut(|f| f.layout_job(job));
    let deco = decos(&galley, &mut ranges, ts);
    TextLaid { galley, deco }
}

/// Per-row extents of decorated char ranges.
fn decos(g: &Galley, ranges: &mut [(usize, usize, DK)], ts: TStyle) -> Vec<Deco> {
    let mut out = Vec::new();
    if ranges.is_empty() {
        return out;
    }
    ranges.sort_by_key(|r| r.0);
    let code_h = (ts.size * 1.32).round();
    // Ranges can overlap (image inside a link): one cursor per range kind is enough, scan all.
    for &(s, e, k) in ranges.iter() {
        let mut ci = 0usize;
        for row in &g.rows {
            let n = row.row.glyphs.len();
            let (r0, r1) = (ci, ci + n);
            if r1 > s && r0 < e {
                let a = s.max(r0) - r0;
                let b = e.min(r1) - r0;
                if let (Some(ga), Some(gb)) = (row.row.glyphs.get(a), row.row.glyphs.get(b.saturating_sub(1))) {
                    let rh = row.row.size.y;
                    let (x0, x1) = (row.pos.x + ga.pos.x, row.pos.x + gb.max_x());
                    let y0 = row.pos.y;
                    match k {
                        DK::Code => {
                            let ch = code_h.min(rh);
                            let t = y0 + ((rh - ch) / 2.0).round();
                            out.push(Deco::Code(Rect::from_min_max(pos2(x0 - 3.0, t), pos2(x1 + 3.0, t + ch))));
                        }
                        DK::Image => {
                            let ch = code_h.min(rh);
                            let t = y0 + ((rh - ch) / 2.0).round();
                            out.push(Deco::Image(Rect::from_min_max(pos2(x0, t), pos2(x1, t + ch))));
                        }
                        DK::Link(l) => {
                            let base = y0 + ga.pos.y + 2.0;
                            out.push(Deco::Link { rect: Rect::from_min_max(pos2(x0, y0), pos2(x1, y0 + rh)), link: l, base });
                        }
                    }
                }
            }
            ci = r1 + usize::from(row.ends_with_newline);
            if ci >= e {
                break;
            }
        }
    }
    out
}

fn first_center(g: &Galley) -> f32 {
    g.rows.first().map_or(0.0, |r| r.pos.y + r.row.size.y / 2.0)
}

pub fn tok_color(th: &Theme, k: TokenKind) -> Color32 {
    let s = &th.syn;
    match k {
        TokenKind::Text => th.code.text,
        TokenKind::Keyword => s.keyword,
        TokenKind::Type => s.type_,
        TokenKind::Function => s.function,
        TokenKind::String => s.string,
        TokenKind::Escape => s.escape,
        TokenKind::Number => s.number,
        TokenKind::Comment => s.comment,
        TokenKind::Operator => s.operator,
        TokenKind::Punct => s.punct,
        TokenKind::Constant => s.constant,
        TokenKind::Attribute => s.attribute,
        TokenKind::Heading => s.heading,
        TokenKind::Emphasis => s.emphasis,
        TokenKind::Strong => s.strong,
        TokenKind::Link => s.link,
        TokenKind::CodeSpan => s.code_span,
        TokenKind::Tag => s.tag,
        TokenKind::Key => s.key,
        TokenKind::Marker => s.marker,
        TokenKind::Inserted => s.inserted,
        TokenKind::Deleted => s.deleted,
    }
}

/// Text style of a leaf (`muted`: inside a plain block quote).
pub fn text_style(lx: &Lx<'_>, kind: TextKind, muted: bool) -> TStyle {
    let (th, sty) = (lx.th, lx.sty);
    let base = if muted { th.ui.text_muted } else { th.ui.text };
    match kind {
        TextKind::Para | TextKind::Empty => TStyle { size: sty.b, color: base, strong: false, lh: sty.lh },
        TextKind::Heading(l) => {
            let size = sty.heading_size(l);
            let color = if l >= 6 { th.ui.text_muted } else { th.ui.text_strong };
            TStyle { size, color, strong: true, lh: (size * 1.3).round() }
        }
        TextKind::Title(a) => TStyle { size: sty.b, color: alert_color(th, a), strong: true, lh: sty.lh },
    }
}

/// Lay out leaf `leaf` at width `w`.
pub fn layout(lx: &Lx<'_>, leaf: &Leaf, w: f32, cols: Option<Arc<Cols>>, muted: bool) -> Laid {
    let (th, sty) = (lx.th, lx.sty);
    match &leaf.body {
        Body::Text { kind, spans } => {
            let ts = text_style(lx, *kind, muted);
            let fallback = match kind {
                TextKind::Title(a) => a.label(),
                _ => " ",
            };
            let t = text_laid(lx, spans, ts, w, fallback);
            let rule = matches!(kind, TextKind::Heading(1 | 2));
            let mut h = t.galley.size().y.max(ts.lh * 0.5);
            if rule {
                h += (sty.b * 0.4).round() + 1.0;
            }
            let first = first_center(&t.galley);
            Laid::Text { t, h, first, rule }
        }
        Body::Code { lang, label, lines, first, last, state, muted } => {
            let inner = (w - 2.0 * sty.code_px).max(20.0);
            let font = FontId::monospace(sty.code);
            let base = TextFormat { font_id: font.clone(), line_height: Some(sty.code_lh), color: if *muted { th.ui.text_muted } else { th.code.text }, ..Default::default() };
            let mut job = LayoutJob { wrap: TextWrapping { max_width: inner, ..Default::default() }, break_on_newline: true, ..Default::default() };
            let mut st: LineState = *state;
            let mut toks = Vec::new();
            for (k, line) in lines.iter().enumerate() {
                if k > 0 {
                    job.append("\n", 0.0, base.clone());
                }
                if *muted || *lang == margin_code::Lang::Plain {
                    job.append(line, 0.0, base.clone());
                    continue;
                }
                toks.clear();
                st = lex_line(*lang, st, line, &mut toks);
                let mut p = 0usize;
                for t in &toks {
                    let (a, b) = (t.start as usize, t.end as usize);
                    if a > p {
                        job.append(line.get(p..a).unwrap_or(""), 0.0, base.clone());
                    }
                    if b > a {
                        let mut tf = base.clone();
                        tf.color = tok_color(th, t.kind);
                        if t.kind == TokenKind::Comment {
                            tf.italics = false;
                        }
                        job.append(line.get(a..b).unwrap_or(""), 0.0, tf);
                    }
                    p = p.max(b);
                }
                if p < line.len() {
                    job.append(line.get(p..).unwrap_or(""), 0.0, base.clone());
                }
            }
            if job.text.is_empty() {
                job.append(" ", 0.0, base.clone());
            }
            let g = lx.ctx.fonts_mut(|f| f.layout_job(job));
            let label = (*first && !label.is_empty()).then(|| lx.ctx.fonts_mut(|f| f.layout_no_wrap(label.clone(), FontId::proportional(sty.small), th.ui.text_faint)));
            let mut top = if *first { sty.code_py } else { 0.0 };
            if let Some(lg) = &label {
                let row_w = g.rows.first().map_or(0.0, |r| r.row.size.x);
                if row_w > inner - lg.size().x - sty.code_px * 0.5 {
                    top += lg.size().y + 2.0;
                }
            }
            let bottom = if *last { sty.code_py } else { 0.0 };
            let first_c = top + first_center(&g);
            let h = top + g.size().y + bottom;
            Laid::Code { g, label, top, h, first: first_c }
        }
        Body::Table { row0, rows, last, .. } => {
            let cols = cols.unwrap_or_else(|| Arc::new(Cols { w: Vec::new(), total: 0.0 }));
            let mut y = 0.0;
            let mut out = Vec::with_capacity(rows.len());
            for (r, row) in rows.iter().enumerate() {
                let header = *row0 as usize + r == 0;
                let ts = TStyle { size: sty.b - 1.0, color: th.ui.text, strong: header, lh: ((sty.b - 1.0) * 1.45).round() };
                let mut cells = Vec::with_capacity(cols.w.len());
                let mut rh: f32 = ts.lh;
                for (c, cw) in cols.w.iter().enumerate() {
                    let spans = row.get(c).map_or(&[][..], |v| &v[..]);
                    let t = text_laid(lx, spans, ts, (cw - 2.0 * sty.cell_px).max(8.0), " ");
                    rh = rh.max(t.galley.size().y);
                    cells.push(t);
                }
                let h = rh + 2.0 * sty.cell_py;
                out.push(TRow { y, h, header, cells });
                y += h;
            }
            let first = out.first().map_or(0.0, |r| r.h / 2.0);
            if *last {
                y += 1.0;
                if cols.total > w + 0.5 {
                    y += 10.0;
                }
            }
            Laid::Table { rows: out, cols, h: y, first, last: *last }
        }
        Body::Rule => Laid::Rule { h: 1.0 },
    }
}

/// Column widths of table `tid` fitted into `avail`: natural widths of the sampled rows, shrunk
/// proportionally above a per-column minimum; wider than `avail` only if the minimums do not fit.
pub fn table_cols(lx: &Lx<'_>, tid: u32, avail: f32) -> Cols {
    let sty = lx.sty;
    let Some(info) = lx.doc.tables.get(tid as usize) else { return Cols { w: Vec::new(), total: 0.0 } };
    let ncols = info.aligns.len();
    let mut nat = vec![0.0f32; ncols];
    let mut word = vec![0.0f32; ncols];
    let mut seen = 0usize;
    'outer: for leaf in lx.doc.leaves.iter().skip(info.first_leaf as usize).take(info.n_leaves as usize) {
        if let Body::Table { rows, row0, .. } = &leaf.body {
            for (r, row) in rows.iter().enumerate() {
                if seen > crate::build::TABLE_SAMPLE {
                    break 'outer;
                }
                seen += 1;
                let header = *row0 as usize + r == 0;
                let ts = TStyle { size: sty.b - 1.0, color: lx.th.ui.text, strong: header, lh: sty.lh };
                for (c, n) in nat.iter_mut().enumerate() {
                    let Some(spans) = row.get(c) else { continue };
                    let short: Vec<Span> = spans.iter().take(64).cloned().map(|mut s| {
                        s.text = crate::text::cap(&s.text, 600).to_owned();
                        s
                    }).collect();
                    let t = text_laid(lx, &short, ts, f32::INFINITY, " ");
                    *n = n.max(t.galley.size().x.ceil() + 2.0 * sty.cell_px + 1.0);
                    // Longest word: a column is not squeezed below it (no mid-word breaks).
                    let longest = short.iter().flat_map(|s| s.text.split_whitespace().map(move |w| (w, s.f))).max_by_key(|(w, _)| w.chars().count());
                    if let Some((w, fl)) = longest {
                        let sp = [Span { text: w.to_owned(), f: fl, link: NONE, img: NONE }];
                        let g = text_laid(lx, &sp, ts, f32::INFINITY, " ");
                        word[c] = word[c].max(g.galley.size().x.ceil() + 2.0 * sty.cell_px + 1.0);
                    }
                }
            }
        }
    }
    let floor_w = (sty.b * 3.0).round();
    let cap_w = (sty.b * 10.0).round();
    let mins: Vec<f32> = nat.iter().zip(&word).map(|(&n, &w)| n.min(w.clamp(floor_w, cap_w))).collect();
    let sum_nat: f32 = nat.iter().sum();
    let sum_min: f32 = mins.iter().sum();
    let w: Vec<f32> = if sum_nat <= avail {
        nat
    } else if sum_min >= avail {
        mins
    } else {
        let k = (avail - sum_min) / (sum_nat - sum_min).max(1.0);
        nat.iter().zip(&mins).map(|(&n, &m)| (m + (n - m) * k).floor()).collect()
    };
    let total = w.iter().sum();
    Cols { w, total }
}

/// Clickable rectangle collected while painting.
#[derive(Clone, Copy, Debug)]
pub struct HitRec {
    pub rect: Rect,
    pub link: u32,
    pub task: u32,
    pub base: f32,
    pub leaf: u32,
    pub k: u32,
}

/// What painting collects and needs besides the leaf: click areas, text areas, the selection.
pub struct Px<'a> {
    pub hits: &'a mut Vec<HitRec>,
    pub areas: &'a mut Vec<Area>,
    /// Ordered selection.
    pub sel: Option<(P, P)>,
    pub sel_color: Color32,
    scratch: Vec<Rect>,
}

impl<'a> Px<'a> {
    pub fn new(hits: &'a mut Vec<HitRec>, areas: &'a mut Vec<Area>, sel: Option<(P, P)>, sel_color: Color32) -> Self {
        Px { hits, areas, sel, sel_color, scratch: Vec::new() }
    }

    /// Selection highlight of one galley (under its text) + its text area.
    fn galley(&mut self, p: &Painter, g: &Arc<Galley>, pos: Pos2, clip: Rect, leaf: u32, unit: u32) {
        let rect = Rect::from_min_size(pos, g.size()).intersect(clip);
        if !rect.is_positive() {
            return;
        }
        self.areas.push(Area { leaf, unit, pos, galley: g.clone(), rect });
        let Some((a, b)) = self.sel.and_then(|s| crate::sel::span(s, leaf, unit)) else { return };
        self.scratch.clear();
        crate::sel::rects(g, a, b, &mut self.scratch);
        for r in &self.scratch {
            let r = r.translate(pos.to_vec2()).intersect(clip);
            if r.is_positive() {
                p.rect_filled(r, 0.0, self.sel_color);
            }
        }
    }
}

fn snap(v: f32, ppp: f32) -> f32 {
    (v * ppp).round() / ppp + 0.5 / ppp
}

#[allow(clippy::too_many_arguments)]
pub fn paint_text(p: &Painter, th: &Theme, t: &TextLaid, pos: Pos2, clip: Rect, leaf: u32, unit: u32, px: &mut Px<'_>, color: Color32) {
    for d in &t.deco {
        if let Deco::Code(r) = d {
            p.rect_filled(r.translate(pos.to_vec2()), 3.0, th.ui.row_hover);
        }
    }
    px.galley(p, &t.galley, pos, clip, leaf, unit);
    let hits = &mut *px.hits;
    p.galley(pos, t.galley.clone(), color);
    for d in &t.deco {
        match d {
            Deco::Image(r) => {
                p.rect_stroke(r.translate(pos.to_vec2()).expand2(vec2(1.0, 0.0)), 3.0, Stroke::new(1.0, th.ui.border), StrokeKind::Inside);
            }
            Deco::Link { rect, link, base } => {
                let r = rect.translate(pos.to_vec2()).intersect(clip);
                if r.is_positive() {
                    let k = hits.iter().rev().take_while(|h| h.leaf == leaf).count() as u32;
                    hits.push(HitRec { rect: r, link: *link, task: NONE, base: base + pos.y, leaf, k });
                }
            }
            Deco::Code(_) => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn paint_code(p: &Painter, th: &Theme, sty: &Sty, l: &Laid, body: &Body, x: f32, y: f32, w: f32, leaf: u32, px: &mut Px<'_>) {
    let Laid::Code { g, label, top, h, .. } = l else { return };
    let Body::Code { first, last, .. } = body else { return };
    let ppp = p.pixels_per_point();
    let r = Rect::from_min_size(pos2(x, y), vec2(w, *h));
    let fill = th.ui.panel_bg;
    let border = th.ui.border;
    if *first && *last {
        p.rect(r, 5.0, fill, Stroke::new(1.0, border), StrokeKind::Inside);
    } else {
        // Chunk of a long block: square corners, side borders, top/bottom border on the ends.
        p.rect_filled(r, CornerRadius::ZERO, fill);
        let s = Stroke::new(1.0, border);
        let (l, rr) = (snap(r.left(), ppp), snap(r.right() - 1.0, ppp));
        p.vline(l, r.top()..=r.bottom(), s);
        p.vline(rr, r.top()..=r.bottom(), s);
        if *first {
            p.hline(r.left()..=r.right(), snap(r.top(), ppp), s);
        }
        if *last {
            p.hline(r.left()..=r.right(), snap(r.bottom() - 1.0, ppp), s);
        }
    }
    if let Some(lg) = label {
        p.galley(pos2(r.right() - sty.code_px * 0.7 - lg.size().x, r.top() + 5.0), lg.clone(), th.ui.text_faint);
    }
    let pos = pos2(x + sty.code_px, y + top);
    px.galley(p, g, pos, p.clip_rect(), leaf, 0);
    p.galley(pos, g.clone(), th.code.text);
}

#[allow(clippy::too_many_arguments)]
pub fn paint_table(p: &Painter, lx: &Lx<'_>, l: &Laid, body: &Body, x: f32, y: f32, w: f32, sx: f32, leaf: u32, px: &mut Px<'_>) {
    let Laid::Table { rows, cols, h, last, .. } = l else { return };
    let Body::Table { tid, .. } = body else { return };
    let th = lx.th;
    let sty = lx.sty;
    let ppp = p.pixels_per_point();
    let aligns = lx.doc.tables.get(*tid as usize).map(|t| &t.aligns[..]).unwrap_or(&[]);
    let total = cols.total;
    let sx = sx.clamp(0.0, (total - w).max(0.0));
    let clip = Rect::from_min_max(pos2(x - 1.0, y - 1.0), pos2(x + w + 1.0, y + h + 1.0)).intersect(p.clip_rect());
    let pc = p.with_clip_rect(clip);
    let tx = x - sx;
    let line = Stroke::new(1.0, th.ui.border);
    let rows_end = rows.last().map_or(0.0, |r| r.y + r.h);
    let ncols = cols.w.len().max(1);
    for (ri, row) in rows.iter().enumerate() {
        let ry = y + row.y;
        if ry > clip.bottom() || ry + row.h < clip.top() {
            continue;
        }
        if row.header {
            pc.rect_filled(Rect::from_min_size(pos2(tx, ry), vec2(total, row.h)), 0.0, th.ui.panel_bg);
        }
        pc.hline(tx..=tx + total, snap(ry, ppp), line);
        let mut cx = tx;
        for (c, cell) in row.cells.iter().enumerate() {
            let cw = cols.w.get(c).copied().unwrap_or(0.0);
            let inner = cw - 2.0 * sty.cell_px;
            let gw = cell.galley.size().x;
            let off = match aligns.get(c) {
                Some(Align::Center) => ((inner - gw) / 2.0).max(0.0),
                Some(Align::Right) => (inner - gw).max(0.0),
                _ => 0.0,
            };
            let pos = pos2((cx + sty.cell_px + off).round(), (ry + sty.cell_py).round());
            let color = if row.header { th.ui.text_strong } else { th.ui.text };
            paint_text(&pc, th, cell, pos, clip, leaf, (ri * ncols + c) as u32, px, color);
            cx += cw;
        }
        let mut vx = tx;
        pc.vline(snap(vx, ppp), ry..=ry + row.h, line);
        for cw in &cols.w {
            vx += cw;
            pc.vline(snap(vx - 1.0, ppp), ry..=ry + row.h, line);
        }
    }
    if *last {
        pc.hline(tx..=tx + total, snap(y + rows_end, ppp), line);
        if total > w + 0.5 {
            let ty = y + rows_end + 5.0;
            let track = Rect::from_min_size(pos2(x, ty), vec2(w, 4.0));
            p.rect_filled(track, 2.0, mix(th.code.bg, th.code.scrollbar, 0.35));
            let tw = (w / total * w).max(24.0);
            let t0 = x + sx / (total - w).max(1.0) * (w - tw);
            p.rect_filled(Rect::from_min_size(pos2(t0, ty), vec2(tw, 4.0)), 2.0, th.code.scrollbar);
        }
    }
}
