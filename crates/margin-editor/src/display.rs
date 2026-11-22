//! Source line → display string + byte-column ↔ display-char mapping. Pure, no egui.
//!
//! | Source | Display | Span |
//! |---|---|---|
//! | normal chars | same chars (space → `·` with `show_whitespace`) | `Plain` (1 char ↔ 1 char) |
//! | tab | spaces to the next stop (`→` first with `show_whitespace`) | `Tab`, atomic |
//! | hazard (`text::is_hazard`) | ` U+202E ` (drawn boxed) | `Hazard`, atomic |
//! | IME preedit | the preedit text at the cursor | `Preedit`, atomic, 0 source bytes |
//! | bytes past `LONG_LINE_CAP` | not shown; ` … +N bytes` marker after the mapped chars | none |
//!
//! Tab stops count display chars (a wide CJK char counts 1). Columns past the cap map to the end.

use margin_code::{Token, TokenKind};

use crate::text::{floor_boundary, hazard_label, is_hazard};

/// Bytes of one line laid out at most (the first N bytes; the rest is summarised by a marker).
pub const LONG_LINE_CAP: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sty {
    Tok(TokenKind),
    Space,
    Hazard,
    /// Thin padding around a hazard label.
    HazardPad,
    Preedit,
    Marker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Plain,
    Tab,
    Hazard,
    Preedit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub src: u32,
    pub src_len: u32,
    pub ch: u32,
    pub ch_len: u32,
    pub kind: SpanKind,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayLine {
    pub text: String,
    /// `(display byte range, style)`, contiguous, covering `text`.
    pub runs: Vec<(std::ops::Range<usize>, Sty)>,
    pub spans: Vec<Span>,
    /// Display chars that map to source (the marker comes after).
    pub mapped_chars: u32,
    /// Source bytes shown (`== line.len()` unless capped).
    pub shown: u32,
    /// Bytes not shown.
    pub hidden: usize,
}

pub struct Opts<'a> {
    pub tab: u32,
    pub show_ws: bool,
    pub cap: usize,
    /// `(col, text)` IME composition inserted at `col`.
    pub preedit: Option<(u32, &'a str)>,
}

impl DisplayLine {
    pub fn build(line: &str, tokens: &[Token], o: &Opts<'_>) -> DisplayLine {
        let tab = o.tab.clamp(1, 64);
        let shown_len = floor_boundary(line, o.cap);
        let mut d = DisplayLine { shown: shown_len as u32, hidden: line.len() - shown_len, ..Default::default() };
        let mut ch = 0u32; // display chars so far (also the column for tab stops)
        let mut tok = 0usize;
        let mut preedit = o.preedit.filter(|(c, t)| !t.is_empty() && (*c as usize) <= shown_len);
        let push_run = |d: &mut DisplayLine, s: &str, sty: Sty| {
            let a = d.text.len();
            d.text.push_str(s);
            let b = d.text.len();
            match d.runs.last_mut() {
                Some((r, st)) if *st == sty && r.end == a => r.end = b,
                _ => d.runs.push((a..b, sty)),
            }
        };
        let push_span = |d: &mut DisplayLine, sp: Span| {
            if let Some(last) = d.spans.last_mut() {
                if last.kind == SpanKind::Plain && sp.kind == SpanKind::Plain && last.src + last.src_len == sp.src && last.ch + last.ch_len == sp.ch {
                    last.src_len += sp.src_len;
                    last.ch_len += sp.ch_len;
                    return;
                }
            }
            d.spans.push(sp);
        };
        let mut buf = [0u8; 4];
        for (i, c) in line[..shown_len].char_indices() {
            if let Some((pc, pt)) = preedit {
                if pc as usize == i {
                    let n = pt.chars().count() as u32;
                    push_run(&mut d, pt, Sty::Preedit);
                    push_span(&mut d, Span { src: i as u32, src_len: 0, ch, ch_len: n, kind: SpanKind::Preedit });
                    ch += n;
                    preedit = None;
                }
            }
            while tok < tokens.len() && tokens[tok].end as usize <= i {
                tok += 1;
            }
            let kind = match tokens.get(tok) {
                Some(t) if t.start as usize <= i => t.kind,
                _ => TokenKind::Text,
            };
            let clen = c.len_utf8() as u32;
            if c == '\t' {
                let n = tab - ch % tab;
                if o.show_ws {
                    push_run(&mut d, "→", Sty::Space);
                    if n > 1 {
                        push_run(&mut d, &" ".repeat(n as usize - 1), Sty::Tok(kind));
                    }
                } else {
                    push_run(&mut d, &" ".repeat(n as usize), Sty::Tok(kind));
                }
                push_span(&mut d, Span { src: i as u32, src_len: 1, ch, ch_len: n, kind: SpanKind::Tab });
                ch += n;
            } else if is_hazard(c) {
                let label = hazard_label(c);
                let n = label.len() as u32 + 2;
                push_run(&mut d, " ", Sty::HazardPad);
                push_run(&mut d, &label, Sty::Hazard);
                push_run(&mut d, " ", Sty::HazardPad);
                push_span(&mut d, Span { src: i as u32, src_len: clen, ch, ch_len: n, kind: SpanKind::Hazard });
                ch += n;
            } else {
                if c == ' ' && o.show_ws {
                    push_run(&mut d, "·", Sty::Space);
                } else {
                    push_run(&mut d, c.encode_utf8(&mut buf), Sty::Tok(kind));
                }
                push_span(&mut d, Span { src: i as u32, src_len: clen, ch, ch_len: 1, kind: SpanKind::Plain });
                ch += 1;
            }
        }
        if let Some((_, pt)) = preedit {
            let n = pt.chars().count() as u32;
            push_run(&mut d, pt, Sty::Preedit);
            push_span(&mut d, Span { src: shown_len as u32, src_len: 0, ch, ch_len: n, kind: SpanKind::Preedit });
            ch += n;
        }
        d.mapped_chars = ch;
        if d.hidden > 0 {
            let m = format!("  … +{} bytes", d.hidden);
            push_run(&mut d, &m, Sty::Marker);
        }
        d
    }

    /// Display char index of source byte column `col` (clamped; past the cap → end of mapped chars).
    pub fn col_to_char(&self, line: &str, col: u32) -> u32 {
        if col >= self.shown {
            return self.mapped_chars;
        }
        // First span with src_len > 0 whose range contains col (a preedit at col sorts before: skip it).
        let i = self.spans.partition_point(|s| s.src + s.src_len <= col);
        let Some(s) = self.spans.get(i) else { return self.mapped_chars };
        match s.kind {
            SpanKind::Plain => {
                let a = s.src as usize;
                let b = floor_boundary(line, col as usize).max(a);
                s.ch + line.get(a..b).map_or(0, |t| t.chars().count() as u32)
            }
            _ => s.ch,
        }
    }

    /// Source column of display char `ch`. Inside an atomic span: nearest edge.
    pub fn char_to_col(&self, line: &str, ch: u32) -> u32 {
        if ch >= self.mapped_chars {
            return self.shown;
        }
        let i = self.spans.partition_point(|s| s.ch + s.ch_len <= ch);
        let Some(s) = self.spans.get(i) else { return self.shown };
        match s.kind {
            SpanKind::Plain => {
                let k = (ch - s.ch) as usize;
                let a = s.src as usize;
                let rest = line.get(a..(s.src + s.src_len) as usize).unwrap_or("");
                a as u32 + rest.char_indices().nth(k).map_or(rest.len(), |(o, _)| o) as u32
            }
            _ => {
                if (ch - s.ch) * 2 < s.ch_len {
                    s.src
                } else {
                    s.src + s.src_len
                }
            }
        }
    }

    /// Atomic span starting exactly at display char `ch`, if any (wrap must not split it).
    pub fn atomic_around(&self, ch: u32) -> Option<(u32, u32)> {
        let i = self.spans.partition_point(|s| s.ch + s.ch_len <= ch);
        let s = self.spans.get(i)?;
        (s.kind != SpanKind::Plain && s.ch < ch && ch < s.ch + s.ch_len).then_some((s.ch, s.ch + s.ch_len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(tab: u32) -> Opts<'static> {
        Opts { tab, show_ws: false, cap: LONG_LINE_CAP, preedit: None }
    }

    #[test]
    fn tabs_and_hazards_map_both_ways() {
        let line = "a\tb\u{202E}é世\tz";
        let d = DisplayLine::build(line, &[], &o(4));
        assert_eq!(d.text, "a   b U+202E é世 z");
        assert!(!d.text.contains('\u{202E}'));
        // col -> char
        let cols: Vec<u32> = line.char_indices().map(|(i, _)| i as u32).chain([line.len() as u32]).collect();
        let chars: Vec<u32> = cols.iter().map(|&c| d.col_to_char(line, c)).collect();
        assert_eq!(chars, vec![0, 1, 4, 5, 13, 14, 15, 16, 17]);
        // char -> col, round trip on boundaries
        for (&c, &ch) in cols.iter().zip(&chars) {
            assert_eq!(d.char_to_col(line, ch), c);
        }
        // inside the tab / hazard: nearest edge
        assert_eq!(d.char_to_col(line, 2), 1);
        assert_eq!(d.char_to_col(line, 3), 2);
        assert_eq!(d.char_to_col(line, 6), 3);
        assert_eq!(d.char_to_col(line, 11), 6);
        // inside a UTF-8 sequence: floored
        assert_eq!(d.col_to_char(line, 9), d.col_to_char(line, 8));
        assert_eq!(d.atomic_around(7), Some((5, 13)));
        assert_eq!(d.atomic_around(5), None);
    }

    #[test]
    fn whitespace_and_preedit() {
        let d = DisplayLine::build("a b\t", &[], &Opts { tab: 4, show_ws: true, cap: 100, preedit: Some((1, "にほ")) });
        assert_eq!(d.text, "aにほ·b→  ");
        assert_eq!(d.col_to_char("a b\t", 1), 3); // caret after the preedit
        assert_eq!(d.col_to_char("a b\t", 4), 8);
    }

    #[test]
    fn long_line_cap() {
        let line = "x".repeat(50_000);
        let d = DisplayLine::build(&line, &[], &o(4));
        assert_eq!(d.shown as usize, LONG_LINE_CAP);
        assert_eq!(d.hidden, 40_000);
        assert!(d.text.ends_with("… +40000 bytes"));
        assert_eq!(d.col_to_char(&line, 49_000), LONG_LINE_CAP as u32);
        assert_eq!(d.char_to_col(&line, 999_999), LONG_LINE_CAP as u32);
        // Cap on a multi-byte boundary.
        let line = "é".repeat(6000);
        let d = DisplayLine::build(&line, &[], &o(4));
        assert_eq!(d.shown as usize, LONG_LINE_CAP);
    }

    #[test]
    fn tokens_become_runs() {
        let toks = [Token { start: 0, end: 2, kind: TokenKind::Keyword }, Token { start: 3, end: 4, kind: TokenKind::Number }];
        let d = DisplayLine::build("fn 1", &toks, &o(4));
        assert_eq!(d.runs, vec![(0..2, Sty::Tok(TokenKind::Keyword)), (2..3, Sty::Tok(TokenKind::Text)), (3..4, Sty::Tok(TokenKind::Number))]);
    }
}
