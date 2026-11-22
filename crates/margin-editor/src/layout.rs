//! Line layout and the row model: one galley per line, wrap rows cut from it, hint rows above.
//!
//! | Row kind | Height | Source |
//! |---|---|---|
//! | lens (hint) row | `lens_h` (~0.85 × row) | `Lens` for the line, when shown |
//! | text row | `row_h` | first visual row of a line |
//! | continuation row | `row_h` | soft wrap; indented to the line's own indentation |
//!
//! Wrap: break after whitespace when the next char would cross the width, else hard break; never inside a
//! tab or hazard box; at most `MAX_WRAP_ROWS` rows per line (the last row is clipped).
//! A position at a row boundary belongs to the next row unless `up` (affinity) is set.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Galley, Stroke};
use margin_code::{Token, TokenKind};
use margin_theme::{mix, Theme};

use crate::display::{DisplayLine, Opts, Sty, LONG_LINE_CAP};
use crate::lens::LensView;
use crate::{Document, Lens, Pos};

pub const MAX_WRAP_ROWS: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VRow {
    pub c0: u32,
    pub c1: u32,
    pub x0: f32,
}

pub struct LineView {
    pub line: u32,
    pub disp: DisplayLine,
    pub galley: Arc<Galley>,
    pub rows: Vec<VRow>,
    /// Continuation-row indent (points).
    pub indent_x: f32,
    pub lens: Option<LensView>,
}

impl LineView {
    /// Glyph x of display char `ch` (end of row past the last glyph).
    pub fn gx(&self, ch: u32) -> f32 {
        glyph_x(&self.galley, ch)
    }

    /// Visual row of display char `ch`.
    pub fn row_of(&self, ch: u32, up: bool) -> usize {
        let n = self.rows.len();
        for (i, r) in self.rows.iter().enumerate() {
            let last = i + 1 == n;
            if (ch < r.c1 || last || (up && ch == r.c1)) && (ch >= r.c0 || i == 0) {
                return i;
            }
        }
        n.saturating_sub(1)
    }

    /// x (text-area coordinates, before horizontal scroll) of `ch` drawn in row `r`.
    pub fn x_in_row(&self, r: usize, ch: u32) -> f32 {
        let Some(row) = self.rows.get(r) else { return 0.0 };
        let ind = if r > 0 { self.indent_x } else { 0.0 };
        ind + self.gx(ch.clamp(row.c0, row.c1)) - row.x0
    }

    /// Right edge of row `r` (text-area x).
    pub fn row_right(&self, r: usize) -> f32 {
        self.rows.get(r).map_or(0.0, |row| self.x_in_row(r, row.c1))
    }

    /// Display char nearest to `x` in row `r`; `up` when the result is the row's end and a later row exists.
    pub fn hit_row(&self, r: usize, x: f32) -> (u32, bool) {
        let Some(row) = self.rows.get(r) else { return (0, false) };
        let last = r + 1 == self.rows.len();
        let glyphs = self.galley.rows.first().map(|p| &p.glyphs[..]).unwrap_or(&[]);
        let ind = if r > 0 { self.indent_x } else { 0.0 };
        let gx = x - ind + row.x0;
        let mut ch = row.c1;
        for c in row.c0..row.c1 {
            let Some(g) = glyphs.get(c as usize) else { break };
            if gx < g.pos.x + g.advance_width * 0.5 {
                ch = c;
                break;
            }
        }
        // Non-last rows end where the next one starts: hitting the end means "end of this row".
        let up = !last && ch == row.c1;
        let ch = ch.min(self.disp.mapped_chars.max(row.c0));
        (ch, up)
    }

    pub fn text_rows_h(&self, row_h: f32) -> f32 {
        self.rows.len() as f32 * row_h
    }

    pub fn lens_h(&self, lens_h: f32) -> f32 {
        if self.lens.is_some() {
            lens_h
        } else {
            0.0
        }
    }

    /// Source column of display char `ch`.
    pub fn col(&self, line: &str, ch: u32) -> u32 {
        self.disp.char_to_col(line, ch)
    }

    pub fn ch(&self, line: &str, col: u32) -> u32 {
        self.disp.col_to_char(line, col)
    }
}

pub fn glyph_x(g: &Galley, ch: u32) -> f32 {
    let Some(row) = g.rows.first() else { return 0.0 };
    match row.glyphs.get(ch as usize) {
        Some(gl) => gl.pos.x,
        None => row.glyphs.last().map_or(0.0, |gl| gl.max_x()),
    }
}

/// Everything needed to lay out lines this frame.
pub struct Lay<'a> {
    pub ctx: egui::Context,
    pub theme: &'a Theme,
    pub font: FontId,
    pub small: FontId,
    pub lens_font: FontId,
    pub row_h: f32,
    pub lens_h: f32,
    pub tab: u32,
    pub show_ws: bool,
    pub wrap: bool,
    /// Width available to text (points).
    pub wrap_w: f32,
    pub lenses: &'a [Lens],
    pub show_lenses: bool,
    pub preedit: Option<(Pos, &'a str)>,
    /// Prose: wrapped list items hang under their text (after `- `, `* `, `1. `).
    pub prose: bool,
    /// Hovered lens segment `(line, seg index)`.
    pub lens_hover: Option<(u32, usize)>,
}

impl Lay<'_> {
    pub fn color(&self, sty: Sty) -> Color32 {
        let s = &self.theme.syn;
        let c = &self.theme.code;
        match sty {
            Sty::Space => c.guide,
            Sty::Hazard | Sty::HazardPad => c.hazard_text,
            Sty::Preedit => c.text,
            Sty::Marker => c.lens_text,
            Sty::Tok(k) => match k {
                TokenKind::Text => c.text,
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
            },
        }
    }

    fn format(&self, sty: Sty) -> TextFormat {
        let font = match sty {
            Sty::Hazard | Sty::Marker => self.small.clone(),
            Sty::HazardPad => FontId::new((self.font.size * 0.3).max(2.0), self.font.family.clone()),
            _ => self.font.clone(),
        };
        let mut f = TextFormat { font_id: font, color: self.color(sty), valign: egui::Align::Center, ..Default::default() };
        match sty {
            Sty::Tok(TokenKind::Emphasis) => f.italics = true,
            Sty::Preedit => f.underline = Stroke::new(1.0, self.theme.code.text),
            Sty::Tok(TokenKind::Link) => f.underline = Stroke::new(1.0, mix(self.theme.syn.link, self.theme.code.bg, 0.5)),
            _ => {}
        }
        f
    }

    pub fn layout_job(&self, job: LayoutJob) -> Arc<Galley> {
        self.ctx.fonts_mut(|f| f.layout_job(job))
    }

    /// Lay out one line (tokens from the lexer cache when known, else plain).
    pub fn line(&self, doc: &Document, line: u32, toks: &mut Vec<Token>) -> LineView {
        doc.tokens(line, toks);
        let text = doc.line(line);
        let preedit = self.preedit.filter(|(p, _)| p.0 == line).map(|(p, t)| (p.1, t));
        let disp = DisplayLine::build(text, toks, &Opts { tab: self.tab, show_ws: self.show_ws, cap: LONG_LINE_CAP, preedit });
        let mut job = LayoutJob { keep_trailing_whitespace: true, break_on_newline: false, ..Default::default() };
        job.text.clone_from(&disp.text);
        for (r, sty) in &disp.runs {
            job.sections.push(egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: egui::text::ByteIndex(r.start)..egui::text::ByteIndex(r.end),
                format: self.format(*sty),
            });
        }
        if job.sections.is_empty() {
            // Empty line: one empty section so the galley has the code font's height.
            job.sections.push(egui::text::LayoutSection { leading_space: 0.0, byte_range: egui::text::ByteIndex(0)..egui::text::ByteIndex(0), format: self.format(Sty::Tok(TokenKind::Text)) });
        }
        let galley = self.layout_job(job);
        let (rows, indent_x) = if self.wrap { wrap_rows(&galley, &disp, self.wrap_w, self.prose) } else { (vec![VRow { c0: 0, c1: glyph_count(&galley), x0: 0.0 }], 0.0) };
        let lens = if self.show_lenses { crate::lens::lens_for(self.lenses, line).map(|l| self.lens_view(l, &galley, &disp, line)) } else { None };
        LineView { line, disp, galley, rows, indent_x, lens }
    }

    /// Block height (lens row + text rows) without building a galley when not wrapping.
    pub fn block_h(&self, doc: &Document, line: u32, toks: &mut Vec<Token>) -> f32 {
        let lens = if self.show_lenses && crate::lens::lens_for(self.lenses, line).is_some() { self.lens_h } else { 0.0 };
        if !self.wrap {
            return lens + self.row_h;
        }
        let v = self.line(doc, line, toks);
        lens + v.text_rows_h(self.row_h)
    }

    pub fn has_lens(&self, line: u32) -> bool {
        self.show_lenses && crate::lens::lens_for(self.lenses, line).is_some()
    }

    pub fn text_h(&self, doc: &Document, line: u32, toks: &mut Vec<Token>) -> f32 {
        self.block_h(doc, line, toks) - if self.has_lens(line) { self.lens_h } else { 0.0 }
    }
}

pub fn glyph_count(g: &Galley) -> u32 {
    g.rows.first().map_or(0, |r| r.glyphs.len() as u32)
}

/// Glyphs of a list marker plus its spaces (`- `, `* `, `+ `, `12. `, `3) `, `> `), else 0.
fn list_marker_len(g: &[egui::epaint::text::Glyph]) -> usize {
    let digits = g.iter().take_while(|x| x.chr.is_ascii_digit()).count();
    let marker = match g.first().map(|x| x.chr) {
        Some('-' | '*' | '+' | '>') => 1,
        Some(_) if (1..=9).contains(&digits) && matches!(g.get(digits).map(|x| x.chr), Some('.' | ')')) => digits + 1,
        _ => return 0,
    };
    let sp = g.iter().skip(marker).take_while(|x| x.chr == ' ' || x.chr == '·').count();
    if sp == 0 || marker + sp >= g.len() {
        return 0;
    }
    marker + sp
}

fn is_space_glyph(c: char) -> bool {
    c == ' ' || c == '·' || c == '→'
}

/// Visual rows of a single-row galley wrapped at `width`; returns rows and the continuation indent.
pub fn wrap_rows(g: &Galley, disp: &DisplayLine, width: f32, prose: bool) -> (Vec<VRow>, f32) {
    let glyphs = g.rows.first().map(|r| &r.glyphs[..]).unwrap_or(&[]);
    let n = glyphs.len() as u32;
    let width = width.max(8.0);
    let mut lead = glyphs.iter().take_while(|gl| is_space_glyph(gl.chr)).count();
    if prose {
        lead += list_marker_len(glyphs.get(lead..).unwrap_or(&[]));
    }
    let indent = if (lead as u32) < n { glyphs.get(lead).map_or(0.0, |gl| gl.pos.x) } else { 0.0 };
    let indent = if indent > width * 0.5 { 0.0 } else { indent };
    let mut rows = Vec::new();
    let mut c0 = 0u32;
    while c0 < n {
        let avail = if rows.is_empty() { width } else { width - indent };
        let x0 = glyphs[c0 as usize].pos.x;
        if rows.len() + 1 >= MAX_WRAP_ROWS {
            rows.push(VRow { c0, c1: n, x0 });
            break;
        }
        let mut c = c0;
        while c < n && glyphs[c as usize].max_x() - x0 <= avail {
            c += 1;
        }
        let fit_end = c.max(c0 + 1);
        // Spaces hang past the edge.
        let mut e = fit_end;
        while e < n && is_space_glyph(glyphs[e as usize].chr) {
            e += 1;
        }
        if e >= n {
            rows.push(VRow { c0, c1: n, x0 });
            break;
        }
        // Last break opportunity (after a space), else a hard break where the text stops fitting.
        let mut b = e;
        while b > c0 && !is_space_glyph(glyphs[b as usize - 1].chr) {
            b -= 1;
        }
        if b <= c0 {
            b = fit_end;
        }
        if let Some((a, z)) = disp.atomic_around(b) {
            b = if a > c0 { a } else { z };
        }
        let b = b.clamp(c0 + 1, n);
        rows.push(VRow { c0, c1: b, x0 });
        c0 = b;
    }
    if rows.is_empty() {
        rows.push(VRow { c0: 0, c1: 0, x0: 0.0 });
    }
    (rows, indent)
}
