//! Text selection in the preview: positions, hit test, highlight rects, copied text.
//!
//! | Item | What |
//! |---|---|
//! | `P` | position `(leaf, unit, char)`; unit = table cell (`row * columns + column`), else 0 |
//! | char | index into the text of the unit as drawn (`unit_text`); a line break counts as one |
//! | `Area` | one galley painted last frame: where a pointer position maps to a `P` |
//! | copy | text as drawn. Blocks: blank line between, `\n` in tight lists; list markers kept; cells: tab, rows: `\n` |

use std::sync::Arc;

use egui::{pos2, Galley, Pos2, Rect, Vec2};

use crate::doc::{f, Body, CtrKind, Gap, Inner, Leaf, Span, TextKind};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct P {
    pub leaf: u32,
    pub unit: u32,
    pub ch: u32,
}

/// A galley on screen.
#[derive(Clone)]
pub struct Area {
    pub leaf: u32,
    pub unit: u32,
    pub pos: Pos2,
    pub galley: Arc<Galley>,
    /// Visible part (screen).
    pub rect: Rect,
}

/// Text of an inline span as drawn (images are a placeholder).
pub fn shown<'a>(doc: &Inner, sp: &'a Span, owned: &'a mut String) -> &'a str {
    if sp.f & f::IMAGE == 0 {
        return &sp.text;
    }
    let src = doc.links.get(sp.img as usize).map_or("", |l| l.shown.as_str());
    owned.clear();
    owned.push_str("\u{2009}image: ");
    owned.push_str(if sp.text.is_empty() { "(no alt)" } else { &sp.text });
    owned.push_str(" \u{2014} ");
    owned.push_str(crate::text::cap(src, 200));
    owned.push('\u{2009}');
    owned
}

fn spans_text(doc: &Inner, spans: &[Span], out: &mut String) {
    let mut owned = String::new();
    for sp in spans {
        out.push_str(shown(doc, sp, &mut owned));
    }
}

fn table_cols(doc: &Inner, tid: u32) -> u32 {
    doc.tables.get(tid as usize).map_or(0, |t| t.aligns.len() as u32)
}

/// Units in a leaf (at least 1).
pub fn units(doc: &Inner, leaf: &Leaf) -> u32 {
    match &leaf.body {
        Body::Table { tid, rows, .. } => (rows.len() as u32).saturating_mul(table_cols(doc, *tid)).max(1),
        _ => 1,
    }
}

/// Text of a unit as drawn, without the placeholder of an empty one. Appends to `out`.
pub fn unit_text(doc: &Inner, leaf: &Leaf, unit: u32, out: &mut String) {
    match &leaf.body {
        Body::Text { kind, spans } => {
            let n = out.len();
            spans_text(doc, spans, out);
            if out.len() == n {
                if let TextKind::Title(a) = kind {
                    out.push_str(a.label());
                }
            }
        }
        Body::Code { lines, .. } => {
            for (k, l) in lines.iter().enumerate() {
                if k > 0 {
                    out.push('\n');
                }
                out.push_str(l);
            }
        }
        Body::Table { tid, rows, .. } => {
            let nc = table_cols(doc, *tid).max(1);
            if let Some(cell) = rows.get((unit / nc) as usize).and_then(|r| r.get((unit % nc) as usize)) {
                spans_text(doc, cell, out);
            }
        }
        Body::Rule => {}
    }
}

/// Char index in `g` nearest to `p` (galley coordinates).
pub fn char_at(g: &Galley, p: Vec2) -> u32 {
    let mut ci = 0usize;
    let last = g.rows.len().saturating_sub(1);
    for (k, row) in g.rows.iter().enumerate() {
        let n = row.row.glyphs.len();
        if p.y < row.pos.y + row.row.size.y || k == last {
            let x = p.x - row.pos.x;
            let i = row.row.glyphs.iter().position(|gl| x < gl.pos.x + gl.advance_width * 0.5).unwrap_or(n);
            return (ci + i) as u32;
        }
        ci += n + usize::from(row.ends_with_newline);
    }
    ci as u32
}

/// Highlight rects (galley coordinates) of chars `a..b`. A selected line end shows as a short stub.
pub fn rects(g: &Galley, a: u32, b: u32, out: &mut Vec<Rect>) {
    let (a, b) = (a as usize, b as usize);
    let mut ci = 0usize;
    for row in &g.rows {
        let gl = &row.row.glyphs;
        let n = gl.len();
        let (r0, r1) = (ci, ci + n);
        if r0 >= b {
            break;
        }
        let over_end = b > r1 && a <= r1 && (row.ends_with_newline || b == u32::MAX as usize);
        let (s, e) = (a.max(r0), b.min(r1));
        if s < e || over_end {
            let end_x = gl.last().map_or(0.0, |x| x.max_x());
            let x0 = if s < r1 { gl.get(s - r0).map_or(end_x, |x| x.pos.x) } else { end_x };
            let mut x1 = if e > s { gl.get(e - 1 - r0).map_or(end_x, |x| x.max_x()) } else { x0 };
            if over_end {
                x1 += (row.row.size.y * 0.3).round();
            }
            out.push(Rect::from_min_max(pos2(row.pos.x + x0, row.pos.y), pos2(row.pos.x + x1, row.pos.y + row.row.size.y)));
        }
        ci = r1 + usize::from(row.ends_with_newline);
    }
}

/// Position under `p`: the area containing it, else the nearest (rows first, then columns).
pub fn hit(areas: &[Area], p: Pos2) -> Option<P> {
    let d = |a: &Area| {
        let dy = (a.rect.top() - p.y).max(p.y - a.rect.bottom()).max(0.0);
        let dx = (a.rect.left() - p.x).max(p.x - a.rect.right()).max(0.0);
        (dy, dx)
    };
    let a = areas.iter().min_by(|x, y| d(x).partial_cmp(&d(y)).unwrap_or(std::cmp::Ordering::Equal))?;
    Some(P { leaf: a.leaf, unit: a.unit, ch: char_at(&a.galley, p - a.pos) })
}

/// Part of unit `(leaf, unit)` inside the ordered selection `a..b`: `(from, to)`, `to == u32::MAX` when it runs on.
pub fn span(sel: (P, P), leaf: u32, unit: u32) -> Option<(u32, u32)> {
    let (a, b) = sel;
    let k = (leaf, unit);
    if a >= b || k < (a.leaf, a.unit) || k > (b.leaf, b.unit) {
        return None;
    }
    let s = if k == (a.leaf, a.unit) { a.ch } else { 0 };
    let e = if k == (b.leaf, b.unit) { b.ch } else { u32::MAX };
    (s < e).then_some((s, e))
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Word (or run of other characters of one kind) around char `ch`.
pub fn word_at(text: &str, ch: u32) -> (u32, u32) {
    let cs: Vec<char> = text.chars().collect();
    if cs.is_empty() {
        return (0, 0);
    }
    let i = (ch as usize).min(cs.len() - 1);
    let class = |c: char| (is_word(c), c.is_whitespace());
    let k = class(cs[i]);
    if cs[i] == '\n' || k == (false, false) {
        return (i as u32, i as u32 + 1);
    }
    let mut s = i;
    while s > 0 && cs[s - 1] != '\n' && class(cs[s - 1]) == k {
        s -= 1;
    }
    let mut e = i + 1;
    while e < cs.len() && cs[e] != '\n' && class(cs[e]) == k {
        e += 1;
    }
    (s as u32, e as u32)
}

/// End of the document.
pub fn end(doc: &Inner) -> P {
    match doc.leaves.last() {
        Some(l) => P { leaf: (doc.leaves.len() - 1) as u32, unit: units(doc, l) - 1, ch: u32::MAX },
        None => P::default(),
    }
}

/// List marker in front of leaf `i` when it starts an item: indent per nesting level, `- `, `3. `, `- [x] `.
fn marker(doc: &Inner, i: usize, leaf: &Leaf, out: &mut String) {
    let mut chain = Vec::new();
    doc.chain(leaf.ctr, &mut chain);
    let Some(at) = chain.iter().position(|&c| doc.ctr(c).is_some_and(|k| matches!(k.kind, CtrKind::Item { .. }) && k.first_leaf as usize == i)) else { return };
    // Only the innermost item, and only if nothing but lists sits between it and the leaf.
    if chain.iter().take(at).any(|&c| !doc.ctr(c).is_some_and(|k| matches!(k.kind, CtrKind::List { .. }))) {
        return;
    }
    let Some(item) = doc.ctr(chain[at]) else { return };
    let depth = chain.iter().skip(at + 1).filter(|&&c| doc.ctr(c).is_some_and(|k| matches!(k.kind, CtrKind::Item { .. }))).count();
    for _ in 0..depth.min(32) {
        out.push_str("  ");
    }
    let ordered = matches!(doc.ctr(item.parent).map(|k| k.kind), Some(CtrKind::List { ordered: true, .. }));
    if let CtrKind::Item { task, num, .. } = item.kind {
        if ordered {
            out.push_str(&format!("{num}. "));
        } else {
            out.push_str("- ");
        }
        match task {
            Some(true) => out.push_str("[x] "),
            Some(false) => out.push_str("[ ] "),
            None => {}
        }
    }
}

/// Text of the ordered selection `a..b`.
pub fn copy(doc: &Inner, a: P, b: P) -> String {
    let mut out = String::new();
    let n = doc.leaves.len();
    if a >= b || n == 0 {
        return out;
    }
    let mut buf = String::new();
    let last = (b.leaf as usize).min(n - 1);
    for i in (a.leaf as usize)..=last {
        let leaf = &doc.leaves[i];
        if i > a.leaf as usize {
            out.push_str(match leaf.gap {
                Gap::Start => "",
                Gap::Cont | Gap::Tight => "\n",
                Gap::Normal { .. } => "\n\n",
            });
        }
        if i > a.leaf as usize || (a.unit == 0 && a.ch == 0) {
            marker(doc, i, leaf, &mut out);
        }
        if matches!(leaf.body, Body::Rule) {
            if i > a.leaf as usize && i < last {
                out.push_str("---");
            }
            continue;
        }
        let nu = units(doc, leaf);
        let nc = match &leaf.body {
            Body::Table { tid, .. } => table_cols(doc, *tid).max(1),
            _ => 1,
        };
        let u0 = if i == a.leaf as usize { a.unit.min(nu - 1) } else { 0 };
        let u1 = if i == b.leaf as usize { b.unit.min(nu - 1) } else { nu - 1 };
        for u in u0..=u1 {
            if u > u0 {
                out.push(if u % nc == 0 { '\n' } else { '\t' });
            }
            buf.clear();
            unit_text(doc, leaf, u, &mut buf);
            let s = if (i as u32, u) == (a.leaf, a.unit) { a.ch as usize } else { 0 };
            let e = if (i as u32, u) == (b.leaf, b.unit) { b.ch as usize } else { usize::MAX };
            out.extend(buf.chars().skip(s).take(e.saturating_sub(s)).filter(|c| *c != '\u{2009}'));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(src: &str) -> String {
        let d = crate::build::build(src);
        copy(&d, P::default(), end(&d))
    }

    #[test]
    fn copies_blocks_lists_code_tables() {
        assert_eq!(all("# Title\n\nSome *styled* `code` and [a link](x.md).\n"), "Title\n\nSome styled code and a link.");
        assert_eq!(all("- one\n- two\n  - deep\n\n1. first\n2. [ ] no\n"), "- one\n- two\n  - deep\n\n1. first\n2. [ ] no");
        assert_eq!(all("- [x] done\n- [ ] todo\n"), "- [x] done\n- [ ] todo");
        assert_eq!(all("```rust\nfn a() {}\n\n  b\n```\n\nafter\n"), "fn a() {}\n\n  b\n\nafter");
        assert_eq!(all("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 |   |\n"), "a\tb\n1\t2\n3\t");
        assert_eq!(all("a\n\n---\n\nb\n"), "a\n\n---\n\nb");
        assert_eq!(all(""), "");
    }

    #[test]
    fn long_code_block_chunks_join_with_one_newline() {
        let lines: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let src = format!("```\n{}\n```\n", lines.join("\n"));
        assert_eq!(all(&src), lines.join("\n"));
    }

    #[test]
    fn partial_ranges() {
        let d = crate::build::build("alpha beta\n\ngamma delta\n");
        let p = |leaf, ch| P { leaf, unit: 0, ch };
        assert_eq!(copy(&d, p(0, 6), p(1, 5)), "beta\n\ngamma");
        assert_eq!(copy(&d, p(0, 2), p(0, 4)), "ph");
        assert_eq!(copy(&d, p(1, 3), p(1, 3)), "");
        assert_eq!(copy(&d, p(1, 3), p(0, 0)), "", "not ordered");
        assert_eq!(copy(&d, p(0, 0), p(99, 0)), "alpha beta\n\ngamma delta", "past the end");
        // A marker only when the item is taken from its start.
        let d = crate::build::build("- item one\n- item two\n");
        assert_eq!(copy(&d, p(0, 5), p(1, 4)), "one\n- item");
    }

    #[test]
    fn words() {
        assert_eq!(word_at("foo_bar baz", 2), (0, 7));
        assert_eq!(word_at("foo_bar baz", 7), (7, 8));
        assert_eq!(word_at("a  b", 1), (1, 3));
        assert_eq!(word_at("x.y", 1), (1, 2));
        assert_eq!(word_at("héllo wörld", 8), (6, 11));
        assert_eq!(word_at("ab", 99), (0, 2));
        assert_eq!(word_at("", 0), (0, 0));
        assert_eq!(word_at("a\nb", 1), (1, 2));
    }

    #[test]
    fn spans_of_units() {
        let p = |leaf, unit, ch| P { leaf, unit, ch };
        let s = (p(1, 0, 4), p(3, 2, 7));
        assert_eq!(span(s, 0, 0), None);
        assert_eq!(span(s, 1, 0), Some((4, u32::MAX)));
        assert_eq!(span(s, 2, 5), Some((0, u32::MAX)));
        assert_eq!(span(s, 3, 1), Some((0, u32::MAX)));
        assert_eq!(span(s, 3, 2), Some((0, 7)));
        assert_eq!(span(s, 3, 3), None);
        assert_eq!(span((p(1, 0, 4), p(1, 0, 4)), 1, 0), None);
    }
}
