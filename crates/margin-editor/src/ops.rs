//! Editing commands on `Document` + `Sel`. Pure (no egui), each one undo group via `Document::group`.
//!
//! | Op | Behaviour |
//! |---|---|
//! | `insert` | replace the selection; `Kind::Type` coalesces |
//! | `newline` | copy the line's indentation; one more level after `{ ( [ :` |
//! | `backspace` / `delete` | selection, else char (cluster) / word / to line edge; soft tabs: one indent level in leading spaces |
//! | `tab` | multi-line selection: indent; else a tab or spaces to the next stop |
//! | `indent` / `outdent` | every line touched by the selection (a line only touched at col 0 by the end is skipped) |
//! | `toggle_comment` | all lines commented → uncomment, else comment at the minimum indentation |

use crate::buffer::order;
use crate::text::{floor_boundary, indent_len, next_boundary, prev_boundary, word_left, word_right};
use crate::undo::{Kind, Sel};
use crate::{Document, Pos};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Char,
    Word,
    /// To the line start (backspace) / end (delete).
    Line,
}

pub struct Tabs {
    pub width: u32,
    pub soft: bool,
}

impl Tabs {
    fn unit(&self) -> String {
        if self.soft {
            " ".repeat(self.width.clamp(1, 16) as usize)
        } else {
            "\t".to_owned()
        }
    }
}

fn range(s: Sel) -> (Pos, Pos) {
    order(s.anchor, s.head)
}

pub fn insert(doc: &mut Document, s: Sel, text: &str, kind: Kind, time: f64) -> Option<Sel> {
    let (a, b) = range(s);
    doc.group(kind, time, s, |tx| Sel::caret(tx.edit(a, b, text)))
}

pub fn newline(doc: &mut Document, s: Sel, tabs: &Tabs, time: f64) -> Option<Sel> {
    let (a, _) = range(s);
    let line = doc.line(a.0);
    let col = a.1 as usize;
    let ind = &line[..indent_len(line).min(col)];
    let before = line[..floor_boundary(line, col)].trim_end();
    let mut text = String::from("\n");
    text.push_str(ind);
    if before.ends_with(['{', '(', '[', ':']) {
        text.push_str(&tabs.unit());
    }
    insert(doc, s, &text, Kind::Other, time)
}

pub fn backspace(doc: &mut Document, s: Sel, reach: Reach, tabs: &Tabs, time: f64) -> Option<Sel> {
    let (a, b) = range(s);
    if a != b {
        return doc.group(Kind::Delete, time, s, |tx| Sel::caret(tx.edit(a, b, "")));
    }
    let (l, c) = a;
    let line = doc.line(l);
    let c = c as usize;
    let from: Pos = if c == 0 {
        if l == 0 {
            return None;
        }
        (l - 1, doc.line(l - 1).len() as u32)
    } else {
        let col = match reach {
            Reach::Line => {
                let ind = indent_len(line);
                if c > ind {
                    ind
                } else {
                    0
                }
            }
            Reach::Word => word_left(line, c),
            Reach::Char => {
                let lead = &line[..c];
                if tabs.soft && !lead.is_empty() && lead.bytes().all(|b| b == b' ') {
                    let w = tabs.width.clamp(1, 16) as usize;
                    let back = match c % w {
                        0 => w,
                        r => r,
                    };
                    c - back
                } else {
                    prev_boundary(line, c)
                }
            }
        };
        (l, col as u32)
    };
    doc.group(Kind::Delete, time, s, |tx| Sel::caret(tx.edit(from, a, "")))
}

pub fn delete(doc: &mut Document, s: Sel, reach: Reach, time: f64) -> Option<Sel> {
    let (a, b) = range(s);
    if a != b {
        return doc.group(Kind::Delete, time, s, |tx| Sel::caret(tx.edit(a, b, "")));
    }
    let (l, c) = a;
    let line = doc.line(l);
    let c = c as usize;
    let to: Pos = if c >= line.len() {
        if l + 1 >= doc.line_count() {
            return None;
        }
        (l + 1, 0)
    } else {
        let col = match reach {
            Reach::Line => line.len(),
            Reach::Word => word_right(line, c),
            Reach::Char => next_boundary(line, c),
        };
        (l, col as u32)
    };
    doc.group(Kind::Delete, time, s, |tx| Sel::caret(tx.edit(a, to, "")))
}

/// Lines touched by the selection.
pub fn sel_lines(s: Sel) -> (u32, u32) {
    let (a, b) = range(s);
    let last = if b.0 > a.0 && b.1 == 0 { b.0 - 1 } else { b.0 };
    (a.0, last)
}

/// Shift a position on line `l` after inserting `n` bytes at column `at` of that line (or removing `-n`).
fn shift(p: Pos, l: u32, at: u32, n: i64) -> Pos {
    if p.0 != l || p.1 < at {
        return p;
    }
    let c = (i64::from(p.1) + n).max(i64::from(at));
    (p.0, c as u32)
}

fn shift_sel(s: Sel, l: u32, at: u32, n: i64) -> Sel {
    Sel { anchor: shift(s.anchor, l, at, n), head: shift(s.head, l, at, n) }
}

pub fn tab(doc: &mut Document, s: Sel, tabs: &Tabs, time: f64) -> Option<Sel> {
    let (a, b) = range(s);
    if a.0 != b.0 {
        return indent(doc, s, tabs, time);
    }
    let text = if tabs.soft {
        let line = doc.line(a.0);
        let w = tabs.width.clamp(1, 16);
        let col = display_col(&line[..floor_boundary(line, a.1 as usize)], w);
        " ".repeat((w - col % w) as usize)
    } else {
        "\t".to_owned()
    };
    insert(doc, s, &text, Kind::Type, time)
}

/// Column (chars, tabs expanded) at the end of `s`.
pub fn display_col(s: &str, tab: u32) -> u32 {
    let tab = tab.max(1);
    s.chars().fold(0u32, |c, ch| if ch == '\t' { (c / tab + 1).saturating_mul(tab) } else { c.saturating_add(1) })
}

pub fn indent(doc: &mut Document, s: Sel, tabs: &Tabs, time: f64) -> Option<Sel> {
    let (l0, l1) = sel_lines(s);
    let unit = tabs.unit();
    doc.group(Kind::Other, time, s, |tx| {
        let mut sel = s;
        for l in l0..=l1 {
            if tx.line(l).is_empty() && l0 != l1 {
                continue;
            }
            tx.edit((l, 0), (l, 0), &unit);
            sel = shift_sel(sel, l, 0, unit.len() as i64);
        }
        sel
    })
}

/// Bytes of one indent level at the start of `line` (tab, or up to `width` spaces).
fn one_level(line: &str, width: u32) -> usize {
    if line.starts_with('\t') {
        return 1;
    }
    line.bytes().take(width.clamp(1, 16) as usize).take_while(|&b| b == b' ').count()
}

pub fn outdent(doc: &mut Document, s: Sel, tabs: &Tabs, time: f64) -> Option<Sel> {
    let (l0, l1) = sel_lines(s);
    doc.group(Kind::Other, time, s, |tx| {
        let mut sel = s;
        for l in l0..=l1 {
            let n = one_level(tx.line(l), tabs.width);
            if n > 0 {
                tx.edit((l, 0), (l, n as u32), "");
                sel = shift_sel(sel, l, 0, -(n as i64));
            }
        }
        sel
    })
}

pub fn toggle_comment(doc: &mut Document, s: Sel, prefix: &str, time: f64) -> Option<Sel> {
    if prefix.is_empty() {
        return None;
    }
    let (l0, l1) = sel_lines(s);
    let lines: Vec<u32> = (l0..=l1).filter(|&l| !doc.line(l).trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    let all = lines.iter().all(|&l| doc.line(l).trim_start_matches([' ', '\t']).starts_with(prefix));
    let min_ind = lines.iter().map(|&l| indent_len(doc.line(l))).min().unwrap_or(0) as u32;
    doc.group(Kind::Other, time, s, |tx| {
        let mut sel = s;
        for &l in &lines {
            let line = tx.line(l);
            if all {
                let at = indent_len(line);
                let mut n = prefix.len();
                if line[at + n..].starts_with(' ') {
                    n += 1;
                }
                tx.edit((l, at as u32), (l, (at + n) as u32), "");
                sel = shift_sel(sel, l, at as u32, -(n as i64));
            } else {
                let ins = format!("{prefix} ");
                tx.edit((l, min_ind), (l, min_ind), &ins);
                sel = shift_sel(sel, l, min_ind, ins.len() as i64);
            }
        }
        sel
    })
}

/// Text to copy/cut: the selection, or the whole line (with its break) when empty.
pub fn copy_range(doc: &Document, s: Sel) -> (Pos, Pos) {
    let (a, b) = range(s);
    if a != b {
        return (a, b);
    }
    let l = a.0;
    if l + 1 < doc.line_count() {
        ((l, 0), (l + 1, 0))
    } else if l > 0 {
        ((l - 1, doc.line(l - 1).len() as u32), (l, doc.line(l).len() as u32))
    } else {
        ((l, 0), (l, doc.line(l).len() as u32))
    }
}

pub fn cut(doc: &mut Document, s: Sel, time: f64) -> Option<(String, Sel)> {
    let (a, b) = copy_range(doc, s);
    let whole = range(s).0 == range(s).1;
    let mut text = doc.buf.slice(a, b);
    if whole && !text.ends_with('\n') {
        // Last line: copy it as a line.
        text = format!("{}\n", text.trim_start_matches('\n'));
    }
    let sel = doc.group(Kind::Other, time, s, |tx| Sel::caret(tx.edit(a, b, "")))?;
    Some((text, sel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use margin_code::Lang;

    fn d(t: &str) -> Document {
        Document::new(t, Lang::Plain)
    }

    const SOFT: Tabs = Tabs { width: 4, soft: true };

    #[test]
    fn newline_auto_indent() {
        let mut doc = d("    fn x() {");
        let s = newline(&mut doc, Sel::caret((0, 12)), &SOFT, 0.0).unwrap();
        assert_eq!(doc.text(), "    fn x() {\n        ");
        assert_eq!(s.head, (1, 8));
        let mut doc = d("\tif x:");
        newline(&mut doc, Sel::caret((0, 6)), &Tabs { width: 4, soft: false }, 0.0);
        assert_eq!(doc.text(), "\tif x:\n\t\t");
        let mut doc = d("  ab");
        newline(&mut doc, Sel::caret((0, 1)), &SOFT, 0.0);
        assert_eq!(doc.text(), " \n  ab");
    }

    #[test]
    fn backspace_variants() {
        let mut doc = d("        x");
        let s = backspace(&mut doc, Sel::caret((0, 8)), Reach::Char, &SOFT, 0.0).unwrap();
        assert_eq!((doc.text().as_str(), s.head), ("    x", (0, 4)));
        let mut doc = d("      x");
        backspace(&mut doc, Sel::caret((0, 6)), Reach::Char, &SOFT, 0.0);
        assert_eq!(doc.text(), "    x");
        let mut doc = d("ae\u{301}");
        backspace(&mut doc, Sel::caret((0, 4)), Reach::Char, &SOFT, 0.0);
        assert_eq!(doc.text(), "a");
        let mut doc = d("a\nb");
        backspace(&mut doc, Sel::caret((1, 0)), Reach::Char, &SOFT, 0.0);
        assert_eq!(doc.text(), "ab");
        let mut doc = d("  foo bar");
        backspace(&mut doc, Sel::caret((0, 9)), Reach::Word, &SOFT, 0.0);
        assert_eq!(doc.text(), "  foo ");
        backspace(&mut doc, Sel::caret((0, 6)), Reach::Line, &SOFT, 0.0);
        assert_eq!(doc.text(), "  ");
        let mut doc = d("ab\ncd");
        delete(&mut doc, Sel::caret((0, 2)), Reach::Char, 0.0);
        assert_eq!(doc.text(), "abcd");
        delete(&mut doc, Sel::caret((0, 1)), Reach::Line, 0.0);
        assert_eq!(doc.text(), "a");
        assert!(backspace(&mut doc, Sel::caret((0, 0)), Reach::Char, &SOFT, 0.0).is_none());
    }

    #[test]
    fn indent_outdent() {
        let mut doc = d("a\n  b\n\nc");
        let s = Sel { anchor: (0, 1), head: (3, 1) };
        let s2 = indent(&mut doc, s, &SOFT, 0.0).unwrap();
        assert_eq!(doc.text(), "    a\n      b\n\n    c");
        assert_eq!(s2, Sel { anchor: (0, 5), head: (3, 5) });
        let s3 = outdent(&mut doc, s2, &SOFT, 0.0).unwrap();
        assert_eq!(doc.text(), "a\n  b\n\nc");
        assert_eq!(s3, s);
        outdent(&mut doc, s3, &SOFT, 0.0);
        assert_eq!(doc.text(), "a\nb\n\nc");
        // End at col 0 of a line: that line is not touched.
        let mut doc = d("a\nb");
        indent(&mut doc, Sel { anchor: (0, 0), head: (1, 0) }, &Tabs { width: 4, soft: false }, 0.0);
        assert_eq!(doc.text(), "\ta\nb");
        // Tab without a multi-line selection: spaces to the next stop.
        let mut doc = d("ab");
        tab(&mut doc, Sel::caret((0, 1)), &SOFT, 0.0);
        assert_eq!(doc.text(), "a   b");
    }

    #[test]
    fn comment_toggle() {
        let mut doc = d("  a\n\n    b");
        let s = Sel { anchor: (0, 0), head: (2, 5) };
        let s = toggle_comment(&mut doc, s, "//", 0.0).unwrap();
        assert_eq!(doc.text(), "  // a\n\n  //   b");
        toggle_comment(&mut doc, s, "//", 0.0).unwrap();
        assert_eq!(doc.text(), "  a\n\n    b");
        // Mixed: comment all.
        let mut doc = d("# x\ny");
        toggle_comment(&mut doc, Sel { anchor: (0, 0), head: (1, 1) }, "#", 0.0);
        assert_eq!(doc.text(), "# # x\n# y");
        assert!(toggle_comment(&mut doc, Sel::caret((0, 0)), "", 0.0).is_none());
    }

    #[test]
    fn cut_whole_line() {
        let mut doc = d("a\nb\nc");
        let (t, s) = cut(&mut doc, Sel::caret((1, 1)), 0.0).unwrap();
        assert_eq!((t.as_str(), doc.text().as_str(), s.head), ("b\n", "a\nc", (1, 0)));
        let (t, _) = cut(&mut doc, Sel::caret((1, 0)), 0.0).unwrap();
        assert_eq!((t.as_str(), doc.text().as_str()), ("c\n", "a"));
    }
}
