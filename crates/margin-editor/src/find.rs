//! Literal find: matcher, incremental counter, next/prev, replace. Pure, no egui.
//!
//! | Item | Rule |
//! |---|---|
//! | case | off by default: Unicode lowercase folding (ASCII fast path) |
//! | whole word | no identifier char right before / after |
//! | matches | non-overlapping, left to right |
//! | counter | `Scan` walks lines with a time budget; restarts on edit or query change; stops at `MAX_MATCHES` |
//! | next / prev | direct search from the cursor, wraps; budgeted, resumes next frame if a 64 MiB file needs it |

use std::time::{Duration, Instant};

use crate::text::is_ident;
use crate::{Document, Pos};

pub const MAX_MATCHES: usize = 100_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub case: bool,
    pub word: bool,
}

pub struct Matcher {
    q: String,
    case: bool,
    word: bool,
}

#[derive(Default)]
pub struct Fold {
    s: String,
    map: Vec<u32>,
}

impl Matcher {
    pub fn new(q: &Query) -> Option<Matcher> {
        if q.text.is_empty() || q.text.contains('\n') {
            return None;
        }
        let text = if q.case { q.text.clone() } else { q.text.to_lowercase() };
        Some(Matcher { q: text, case: q.case, word: q.word })
    }

    /// Calls `f(start, end)` for each match in `line` (byte columns); `f` returns false to stop.
    pub fn each(&self, line: &str, fold: &mut Fold, mut f: impl FnMut(u32, u32) -> bool) {
        if line.len() < self.q.len() && (self.case || line.is_ascii()) {
            return;
        }
        let ascii = line.is_ascii();
        let hay: &str = if self.case {
            line
        } else if ascii {
            fold.s.clear();
            fold.s.push_str(line);
            fold.s.make_ascii_lowercase();
            &fold.s
        } else {
            fold.s.clear();
            fold.map.clear();
            for (i, c) in line.char_indices() {
                for lc in c.to_lowercase() {
                    let n = lc.len_utf8();
                    fold.s.push(lc);
                    fold.map.extend(std::iter::repeat_n(i as u32, n));
                }
            }
            &fold.s
        };
        let mapped = !self.case && !ascii;
        let to_src = |i: usize| -> u32 {
            if !mapped {
                i as u32
            } else if i >= fold.map.len() {
                line.len() as u32
            } else {
                fold.map[i]
            }
        };
        let mut at = 0usize;
        while at <= hay.len() {
            let Some(off) = hay[at..].find(self.q.as_str()) else { break };
            let fa = at + off;
            let fb = fa + self.q.len();
            let a = to_src(fa);
            let mut b = to_src(fb);
            if b <= a {
                b = crate::text::next_boundary(line, a as usize) as u32;
            }
            let ok = !self.word || {
                let before = line[..a as usize].chars().next_back();
                let after = line[b as usize..].chars().next();
                !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
            };
            if ok {
                if !f(a, b) {
                    return;
                }
                at = fb;
            } else {
                at = fa + hay[fa..].chars().next().map_or(1, char::len_utf8);
            }
        }
    }

    pub fn line_matches(&self, line: &str, fold: &mut Fold) -> Vec<(u32, u32)> {
        let mut v = Vec::new();
        self.each(line, fold, |a, b| {
            v.push((a, b));
            true
        });
        v
    }
}

/// Incremental match counter.
#[derive(Default)]
pub struct Scan {
    key: Option<(u64, Query)>,
    next_line: u32,
    pub matches: Vec<(u32, u32, u32)>,
    pub done: bool,
    pub capped: bool,
    fold: Fold,
}

impl Scan {
    pub fn reset(&mut self) {
        self.key = None;
        self.matches.clear();
        self.done = false;
        self.capped = false;
        self.next_line = 0;
    }

    /// Continue scanning for `q` within `budget`. Restarts when the document or query changed.
    pub fn step(&mut self, doc: &Document, q: &Query, budget: Duration) {
        let key = (doc.revision(), q.clone());
        if self.key.as_ref() != Some(&key) {
            self.reset();
            self.key = Some(key);
        }
        if self.done {
            return;
        }
        let Some(m) = Matcher::new(q) else {
            self.done = true;
            return;
        };
        let t0 = Instant::now();
        let n = doc.line_count();
        let mut fold = std::mem::take(&mut self.fold);
        while self.next_line < n {
            let l = self.next_line;
            let mut full = false;
            let ms = &mut self.matches;
            m.each(doc.line(l), &mut fold, |a, b| {
                ms.push((l, a, b));
                full = ms.len() >= MAX_MATCHES;
                !full
            });
            self.next_line += 1;
            if full {
                self.capped = true;
                self.next_line = n;
                break;
            }
            if l.is_multiple_of(512) && t0.elapsed() >= budget {
                break;
            }
        }
        self.fold = fold;
        self.done = self.next_line >= n;
    }

    /// 1-based index of the match starting at `p`, if counted.
    pub fn index_of(&self, p: Pos) -> Option<usize> {
        self.matches.binary_search_by(|m| (m.0, m.1).cmp(&p)).ok().map(|i| i + 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Next,
    Prev,
}

/// Search outcome: found, nothing at all, or budget spent (resume at `line` next frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Found(Pos, Pos),
    None,
    Pending(Resume),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resume {
    pub from: Pos,
    pub dir: Dir,
    /// Lines already searched.
    pub done: u32,
}

/// Match after `from` (Next: start >= from) or before it (Prev: start < from), wrapping around.
pub fn search(doc: &Document, q: &Query, r: Resume, budget: Duration) -> Hit {
    let Some(m) = Matcher::new(q) else { return Hit::None };
    let n = doc.line_count();
    let from = doc.clamp(r.from);
    let t0 = Instant::now();
    let mut fold = Fold::default();
    let mut k = r.done;
    while k <= n {
        // k = 0: the start line (part after/before `from`); 1..n-1: other lines; n: start line again (wrapped part).
        let l = match r.dir {
            Dir::Next => (from.0 + k) % n,
            Dir::Prev => (from.0 + n - k % n) % n,
        };
        let ms = m.line_matches(doc.line(l), &mut fold);
        let pick = match (r.dir, k) {
            (Dir::Next, 0) => ms.iter().find(|x| x.0 >= from.1).copied(),
            (Dir::Next, _) if k == n => ms.iter().find(|x| x.0 < from.1).copied(),
            (Dir::Next, _) => ms.first().copied(),
            (Dir::Prev, 0) => ms.iter().rev().find(|x| x.0 < from.1).copied(),
            (Dir::Prev, _) if k == n => ms.iter().rev().find(|x| x.0 >= from.1).copied(),
            (Dir::Prev, _) => ms.last().copied(),
        };
        if let Some((a, b)) = pick {
            return Hit::Found((l, a), (l, b));
        }
        k += 1;
        if k.is_multiple_of(1024) && t0.elapsed() >= budget {
            return Hit::Pending(Resume { done: k, ..r });
        }
    }
    Hit::None
}

/// Every match in the document (no cap): for replace all.
pub fn all_matches(doc: &Document, q: &Query) -> Vec<(u32, u32, u32)> {
    let Some(m) = Matcher::new(q) else { return Vec::new() };
    let mut fold = Fold::default();
    let mut out = Vec::new();
    for l in 0..doc.line_count() {
        m.each(doc.line(l), &mut fold, |a, b| {
            out.push((l, a, b));
            true
        });
    }
    out
}

/// Replace every match in one undo group. Returns the count.
pub fn replace_all(doc: &mut Document, q: &Query, with: &str, sel: crate::undo::Sel, time: f64) -> usize {
    let all = all_matches(doc, q);
    if all.is_empty() {
        return 0;
    }
    let with = with.replace("\r\n", "\n");
    let n = all.len();
    let done = doc.group(crate::undo::Kind::Other, time, sel, |tx| {
        for &(l, a, b) in all.iter().rev() {
            tx.edit((l, a), (l, b), &with);
        }
        let first = all[0];
        crate::undo::Sel::caret((first.0, first.1))
    });
    if done.is_some() {
        n
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use margin_code::Lang;

    fn q(t: &str, case: bool, word: bool) -> Query {
        Query { text: t.into(), case, word }
    }

    #[test]
    fn matcher_case_word_unicode() {
        let mut f = Fold::default();
        let m = Matcher::new(&q("foo", false, false)).unwrap();
        assert_eq!(m.line_matches("Foo fOO foofoo", &mut f), vec![(0, 3), (4, 7), (8, 11), (11, 14)]);
        let m = Matcher::new(&q("foo", true, false)).unwrap();
        assert_eq!(m.line_matches("Foo foo", &mut f), vec![(4, 7)]);
        let m = Matcher::new(&q("foo", false, true)).unwrap();
        assert_eq!(m.line_matches("foo foobar _foo foo.", &mut f), vec![(0, 3), (16, 19)]);
        let m = Matcher::new(&q("straße", false, false)).unwrap();
        assert_eq!(m.line_matches("é STRAßE x", &mut f), vec![(3, 10)]);
        let m = Matcher::new(&q("é", false, false)).unwrap();
        assert_eq!(m.line_matches("aÉb", &mut f), vec![(1, 3)]);
        // Folding that changes lengths (İ → i̇) never yields bad boundaries.
        let m = Matcher::new(&q("i", false, false)).unwrap();
        for (a, b) in m.line_matches("İxİ", &mut f) {
            assert!("İxİ".is_char_boundary(a as usize) && "İxİ".is_char_boundary(b as usize) && a < b);
        }
        assert!(Matcher::new(&q("", false, false)).is_none());
        let m = Matcher::new(&q("aa", false, false)).unwrap();
        assert_eq!(m.line_matches("aaa", &mut f), vec![(0, 2)]);
    }

    #[test]
    fn incremental_scan_and_search() {
        let text: String = (0..5000).map(|i| if i % 10 == 0 { "x needle y\n" } else { "hay\n" }).collect();
        let doc = Document::new(&text, Lang::Plain);
        let qq = q("needle", false, false);
        let mut s = Scan::default();
        let mut steps = 0;
        while !s.done {
            s.step(&doc, &qq, Duration::ZERO);
            steps += 1;
        }
        assert!(steps > 1, "zero budget must take several steps");
        assert_eq!(s.matches.len(), 500);
        assert_eq!(s.index_of((10, 2)), Some(2));
        let r = |from, dir| Resume { from, dir, done: 0 };
        assert_eq!(search(&doc, &qq, r((0, 3), Dir::Next), Duration::from_secs(1)), Hit::Found((10, 2), (10, 8)));
        assert_eq!(search(&doc, &qq, r((0, 2), Dir::Next), Duration::from_secs(1)), Hit::Found((0, 2), (0, 8)));
        assert_eq!(search(&doc, &qq, r((0, 2), Dir::Prev), Duration::from_secs(1)), Hit::Found((4990, 2), (4990, 8)));
        assert_eq!(search(&doc, &qq, r((4995, 0), Dir::Next), Duration::from_secs(1)), Hit::Found((0, 2), (0, 8)));
        assert_eq!(search(&doc, &q("zzz", false, false), r((5, 0), Dir::Next), Duration::from_secs(1)), Hit::None);
        // Zero budget: pending, then resumes to the same answer.
        let mut res = r((1, 0), Dir::Next);
        let hit = loop {
            match search(&doc, &qq, res, Duration::ZERO) {
                Hit::Pending(next) => res = next,
                h => break h,
            }
        };
        assert_eq!(hit, Hit::Found((10, 2), (10, 8)));
        // Single-line wrap: the start line is searched again for the part before `from`.
        let one = Document::new("ab ab", Lang::Plain);
        assert_eq!(search(&one, &q("ab", true, false), r((0, 4), Dir::Next), Duration::from_secs(1)), Hit::Found((0, 0), (0, 2)));
        assert_eq!(search(&one, &q("ab", true, false), r((0, 0), Dir::Prev), Duration::from_secs(1)), Hit::Found((0, 3), (0, 5)));
    }

    #[test]
    fn scan_restarts_on_edit_and_caps() {
        let mut doc = Document::new("a a a\n", Lang::Plain);
        let qq = q("a", false, false);
        let mut s = Scan::default();
        s.step(&doc, &qq, Duration::from_secs(1));
        assert_eq!(s.matches.len(), 3);
        doc.replace((1, 0), (1, 0), "a");
        s.step(&doc, &qq, Duration::from_secs(1));
        assert_eq!(s.matches.len(), 4);
        let big = Document::new(&"a".repeat(MAX_MATCHES + 10), Lang::Plain);
        let mut s = Scan::default();
        s.step(&big, &qq, Duration::from_secs(10));
        assert!(s.done && s.capped && s.matches.len() == MAX_MATCHES);
    }

    #[test]
    fn replace_all_is_one_undo_group() {
        let mut doc = Document::new("foo x foo\nfoo", Lang::Plain);
        let n = replace_all(&mut doc, &q("FOO", false, false), "barbaz", Default::default(), 0.0);
        assert_eq!(n, 3);
        assert_eq!(doc.text(), "barbaz x barbaz\nbarbaz");
        let mut st = crate::EditorState::default();
        st.undo(&mut doc);
        assert_eq!(doc.text(), "foo x foo\nfoo");
        assert!(!doc.is_dirty());
    }
}
