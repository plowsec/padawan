//! Lines + line-ending/BOM memory + the edit primitive. Pure, no egui.
//!
//! | Input | Stored | Written back by `text()` |
//! |---|---|---|
//! | `\n` / `\r\n` | lines without breaks; dominant ending (`\r\n` if strictly more than `\n`) | dominant ending |
//! | mixed endings | `original` copy kept while unedited | `original` byte for byte; after an edit: dominant ending everywhere |
//! | lone `\r` | stays in the line (drawn as a hazard) | as is |
//! | UTF-8 BOM | flag | prepended |
//! | final newline | an empty last line | follows from the lines |

use crate::text::floor_boundary;
use crate::Pos;

/// Replace `start..end` (document order) with `text` (`\n` separates lines).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub start: Pos,
    pub end: Pos,
    pub text: String,
}

impl Edit {
    pub fn new(start: Pos, end: Pos, text: impl Into<String>) -> Self {
        Self { start, end, text: text.into() }
    }

    /// Stored-text size for history accounting.
    pub fn cost(&self) -> usize {
        self.text.len() + 24
    }
}

pub struct Buffer {
    pub lines: Vec<String>,
    pub crlf: bool,
    pub bom: bool,
    /// Exact input text while it cannot be rebuilt from the lines (mixed endings).
    pub original: Option<String>,
    /// Sum of line lengths (no breaks).
    content: usize,
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        let (bom, body) = match text.strip_prefix('\u{FEFF}') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let mut lines = Vec::with_capacity(body.len() / 32 + 1);
        let (mut lf, mut crlf) = (0usize, 0usize);
        let mut content = 0usize;
        for l in body.split('\n') {
            lines.push(l.to_owned());
        }
        let n = lines.len();
        for (i, l) in lines.iter_mut().enumerate() {
            if i + 1 < n {
                if l.ends_with('\r') {
                    l.pop();
                    crlf += 1;
                } else {
                    lf += 1;
                }
            }
            content += l.len();
        }
        let mut b = Buffer { lines, crlf: crlf > lf, bom, original: None, content };
        if lf > 0 && crlf > 0 {
            b.original = Some(text.to_owned());
        }
        b
    }

    pub fn line_count(&self) -> u32 {
        u32::try_from(self.lines.len()).unwrap_or(u32::MAX)
    }

    pub fn line(&self, i: u32) -> &str {
        self.lines.get(i as usize).map_or("", String::as_str)
    }

    fn eol(&self) -> &'static str {
        if self.crlf {
            "\r\n"
        } else {
            "\n"
        }
    }

    pub fn len_bytes(&self) -> usize {
        if let Some(o) = &self.original {
            return o.len();
        }
        let breaks = self.lines.len().saturating_sub(1) * self.eol().len();
        self.content + breaks + if self.bom { 3 } else { 0 }
    }

    pub fn text(&self) -> String {
        if let Some(o) = &self.original {
            return o.clone();
        }
        let mut s = String::with_capacity(self.len_bytes());
        if self.bom {
            s.push('\u{FEFF}');
        }
        let eol = self.eol();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                s.push_str(eol);
            }
            s.push_str(l);
        }
        s
    }

    /// Valid position: line in range, column on a char boundary within the line.
    pub fn clamp(&self, (l, c): Pos) -> Pos {
        let l = l.min(self.line_count().saturating_sub(1));
        let s = self.line(l);
        (l, floor_boundary(s, c as usize) as u32)
    }

    pub fn end_pos(&self) -> Pos {
        let l = self.line_count().saturating_sub(1);
        (l, self.line(l).len() as u32)
    }

    /// Text in `a..b` (document order, clamped), lines joined by `\n`.
    pub fn slice(&self, a: Pos, b: Pos) -> String {
        let (a, b) = order(self.clamp(a), self.clamp(b));
        if a.0 == b.0 {
            return self.line(a.0)[a.1 as usize..b.1 as usize].to_owned();
        }
        let mut s = String::new();
        s.push_str(&self.line(a.0)[a.1 as usize..]);
        for l in a.0 + 1..b.0 {
            s.push('\n');
            s.push_str(self.line(l));
        }
        s.push('\n');
        s.push_str(&self.line(b.0)[..b.1 as usize]);
        s
    }

    /// The primitive. Applies `e` (clamped) and returns its inverse. `\n` splits lines; a `\r` is line content
    /// (callers normalise pasted `\r\n`).
    /// Also returns the line range touched: `(first line, lines removed after it, lines inserted after it)`.
    pub fn apply(&mut self, e: &Edit) -> (Edit, (usize, usize, usize)) {
        self.original = None;
        let (a, b) = order(self.clamp(e.start), self.clamp(e.end));
        let removed = self.slice(a, b);
        let text = e.text.as_str();
        let (al, ac) = (a.0 as usize, a.1 as usize);
        let (bl, bc) = (b.0 as usize, b.1 as usize);
        let tail = self.lines[bl][bc..].to_owned();
        let mut parts = text.split('\n');
        let first = parts.next().unwrap_or("");
        let old_len: usize = self.lines[al..=bl].iter().map(String::len).sum();
        // First line: head + first part.
        {
            let line = &mut self.lines[al];
            line.truncate(ac);
            line.push_str(first);
        }
        let mut new_lines: Vec<String> = parts.map(str::to_owned).collect();
        let inserted = new_lines.len();
        let end = if let Some(last) = new_lines.last_mut() {
            let col = last.len();
            last.push_str(&tail);
            ((al + inserted) as u32, col as u32)
        } else {
            let col = self.lines[al].len();
            self.lines[al].push_str(&tail);
            (al as u32, col as u32)
        };
        let removed_lines = bl - al;
        if removed_lines > 0 || inserted > 0 {
            self.lines.splice(al + 1..=bl, new_lines.drain(..));
        }
        let new_len: usize = self.lines[al..=al + inserted].iter().map(String::len).sum();
        self.content = self.content + new_len - old_len;
        (Edit { start: a, end, text: removed }, (al, removed_lines, inserted))
    }
}

pub fn order(a: Pos, b: Pos) -> (Pos, Pos) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testrng::Rng;

    #[test]
    fn roundtrip_basic() {
        for t in ["", "a", "a\n", "a\r\nb\r\n", "\u{FEFF}x\ny", "\n\n", "a\r\nb\nc", "a\rb\n", "\r\n", "x\r"] {
            let b = Buffer::new(t);
            assert_eq!(b.text(), t, "{t:?}");
            assert_eq!(b.len_bytes(), t.len(), "{t:?}");
        }
        assert_eq!(Buffer::new("a\nb\n").line_count(), 3);
        assert!(Buffer::new("a\r\nb\r\nc\n").crlf);
        assert!(!Buffer::new("a\r\nb\nc\n").crlf);
    }

    #[test]
    fn mixed_edited_uses_dominant() {
        let mut b = Buffer::new("a\r\nb\r\nc\n");
        b.apply(&Edit::new((0, 1), (0, 1), "!"));
        assert_eq!(b.text(), "a!\r\nb\r\nc\r\n");
    }

    #[test]
    fn apply_and_inverse() {
        let mut b = Buffer::new("hello\nworld\n");
        let (inv, _) = b.apply(&Edit::new((0, 2), (1, 3), "XY\nZ"));
        assert_eq!(b.text(), "heXY\nZld\n");
        assert_eq!(inv, Edit::new((0, 2), (1, 1), "llo\nwor"));
        let (back, _) = b.apply(&inv);
        assert_eq!(b.text(), "hello\nworld\n");
        assert_eq!(back, Edit::new((0, 2), (1, 3), "XY\nZ"));
        // Clamps hostile positions, reversed ranges.
        let (inv, _) = b.apply(&Edit::new((99, 99), (0, 1), ""));
        assert_eq!(b.text(), "h");
        b.apply(&inv);
        assert_eq!(b.text(), "hello\nworld\n");
        // Inside a UTF-8 sequence: floored.
        let mut b = Buffer::new("é");
        b.apply(&Edit::new((0, 1), (0, 1), "x"));
        assert_eq!(b.text(), "xé");
    }

    fn gen_text(r: &mut Rng) -> String {
        const PIECES: &[&str] = &["a", "é", "世", "\n", "\r\n", "\r", "\t", "\u{202E}", " ", "\u{FEFF}", "x\u{301}", "\0"];
        let mut s = String::new();
        if r.below(4) == 0 {
            s.push('\u{FEFF}');
        }
        for _ in 0..r.below(40) {
            s.push_str(PIECES[r.below(PIECES.len())]);
        }
        s
    }

    #[test]
    fn prop_roundtrip_and_inverse() {
        let mut r = Rng::new(0x5eed);
        for _ in 0..3000 {
            let t = gen_text(&mut r);
            let mut b = Buffer::new(&t);
            assert_eq!(b.text(), t);
            assert_eq!(b.len_bytes(), t.len());
            // Random edit then its inverse gives the normalised text back.
            let norm = {
                let mut c = Buffer::new(&t);
                c.original = None;
                c.text()
            };
            let lc = b.line_count();
            let p = |r: &mut Rng| (r.below(lc as usize + 1) as u32, r.below(8) as u32);
            let e = Edit::new(p(&mut r), p(&mut r), gen_text(&mut r).replace('\u{FEFF}', ""));
            let (inv, _) = b.apply(&e);
            assert_eq!(b.len_bytes(), b.text().len());
            b.apply(&inv);
            assert_eq!(b.text(), norm);
            for l in &b.lines {
                assert!(!l.contains('\n'));
            }
        }
    }
}
