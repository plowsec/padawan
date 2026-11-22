//! Hint lines above functions: lookup (binary search on the sorted list) and row layout.
//!
//! | Tone | Colour | Clickable |
//! |---|---|---|
//! | `Normal` | `lens_text`, hover `lens_hover` | yes |
//! | `Sink` | `lens_sink`, hover `lens_hover` | yes |
//! | `Faint` | `lens_text` faded to the background | no |
//!
//! Segments are joined by ` · `. The row starts at the line's indentation.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::Galley;
use margin_theme::mix;

use crate::display::DisplayLine;
use crate::layout::Lay;
use crate::{Lens, LensTone};

/// First lens for `line` in a list sorted by line.
pub fn lens_for(lenses: &[Lens], line: u32) -> Option<&Lens> {
    let i = lenses.partition_point(|l| l.line < line);
    lenses.get(i).filter(|l| l.line == line && !l.segs.is_empty())
}

pub struct LensSegView {
    pub x0: f32,
    pub x1: f32,
    pub idx: usize,
    pub id: u32,
    pub clickable: bool,
    pub hover: String,
}

pub struct LensView {
    pub galley: Arc<Galley>,
    /// Text-area x where the row starts.
    pub x: f32,
    pub segs: Vec<LensSegView>,
}

impl Lay<'_> {
    pub fn lens_view(&self, lens: &Lens, line_galley: &Galley, _disp: &DisplayLine, line: u32) -> LensView {
        let glyphs = line_galley.rows.first().map(|r| &r.glyphs[..]).unwrap_or(&[]);
        let x = glyphs.iter().find(|g| !matches!(g.chr, ' ' | '·' | '→')).map_or(0.0, |g| g.pos.x);
        let c = &self.theme.code;
        let faint = mix(c.lens_text, c.bg, 0.45);
        let mut job = LayoutJob { keep_trailing_whitespace: true, break_on_newline: false, ..Default::default() };
        let mut ranges = Vec::new();
        for (i, s) in lens.segs.iter().enumerate() {
            if i > 0 {
                job.append(" · ", 0.0, TextFormat { font_id: self.lens_font.clone(), color: faint, valign: egui::Align::Center, ..Default::default() });
            }
            let hovered = self.lens_hover == Some((line, i)) && s.tone != LensTone::Faint;
            let color = if hovered {
                c.lens_hover
            } else {
                match s.tone {
                    LensTone::Normal => c.lens_text,
                    LensTone::Sink => c.lens_sink,
                    LensTone::Faint => faint,
                }
            };
            let a = job.text.chars().count();
            // Never lay out raw control / bidi chars from hint text either.
            let clean: String = s.text.chars().map(|ch| if crate::text::is_hazard(ch) || ch == '\t' { '\u{FFFD}' } else { ch }).take(400).collect();
            job.append(&clean, 0.0, TextFormat { font_id: self.lens_font.clone(), color, valign: egui::Align::Center, ..Default::default() });
            let b = job.text.chars().count();
            ranges.push((a, b, i));
        }
        let galley = self.layout_job(job);
        let segs = ranges
            .into_iter()
            .map(|(a, b, i)| {
                let s = &lens.segs[i];
                LensSegView {
                    x0: x + crate::layout::glyph_x(&galley, a as u32),
                    x1: x + crate::layout::glyph_x(&galley, b as u32),
                    idx: i,
                    id: s.id,
                    clickable: s.tone != LensTone::Faint,
                    hover: s.hover.clone(),
                }
            })
            .collect();
        LensView { galley, x, segs }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LensSeg;

    #[test]
    fn lookup_binary_search() {
        let seg = vec![LensSeg { text: "x".into(), ..Default::default() }];
        let ls: Vec<Lens> = (0..5000).map(|i| Lens { line: i * 3, segs: seg.clone() }).collect();
        assert_eq!(lens_for(&ls, 300).map(|l| l.line), Some(300));
        assert!(lens_for(&ls, 301).is_none());
        assert!(lens_for(&ls, u32::MAX).is_none());
        assert!(lens_for(&[Lens { line: 2, segs: vec![] }], 2).is_none());
    }
}
