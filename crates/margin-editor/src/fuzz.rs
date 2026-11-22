//! Random walk over editing ops on hostile text: never panics, keeps every invariant. Test-only.

use crate::buffer::order;
use crate::ops::{self, Reach, Tabs};
use crate::testrng::Rng;
use crate::text::{next_boundary, prev_boundary, word_left, word_right};
use crate::undo::{Kind, Sel};
use crate::{Document, EditorState, Pos};
use margin_code::Lang;

const PIECES: &[&str] = &[
    "a", "Z", " ", "\t", "\n", "\r\n", "\r", "é", "e\u{301}", "世界", "😀", "\u{202E}", "\u{2066}", "\u{200B}", "\u{FEFF}", "\0", "\x1b", "{", "}", "(", "//", "#", "foo", "\u{E0041}", "\u{7f}",
];

fn rand_text(r: &mut Rng, max: usize) -> String {
    (0..r.below(max + 1)).map(|_| PIECES[r.below(PIECES.len())]).collect()
}

fn rand_pos(r: &mut Rng, d: &Document) -> Pos {
    // Deliberately hostile: out of range and inside UTF-8 sequences.
    let l = r.below(d.line_count() as usize + 2) as u32;
    let c = r.below(d.line(l.min(d.line_count() - 1)).len() + 3) as u32;
    (l, c)
}

fn check(d: &Document, s: Sel) {
    for p in [s.anchor, s.head] {
        assert!(p.0 < d.line_count(), "line out of range {p:?}");
        assert!(d.line(p.0).is_char_boundary(p.1 as usize), "not a char boundary {p:?} in {:?}", d.line(p.0));
    }
    assert_eq!(d.len_bytes(), d.text().len());
    for i in 0..d.line_count() {
        assert!(!d.line(i).contains('\n'));
    }
}

#[test]
fn random_walk() {
    let mut r = Rng::new(0xC0FFEE);
    for round in 0..40 {
        let start = rand_text(&mut r, 60);
        let mut d = Document::new(&start, Lang::Plain);
        assert_eq!(d.text(), start);
        d.set_lang([Lang::Plain, Lang::Markdown, Lang::Rust, Lang::Python, Lang::Ruby, Lang::Lua][round % 6]);
        let norm = {
            let mut x = Document::new(&start, Lang::Plain);
            x.replace((0, 0), (0, 0), "");
            x.text()
        };
        let mut s = Sel::default();
        let tabs = Tabs { width: 1 + r.below(8) as u32, soft: r.below(2) == 0 };
        let mut t = 0.0;
        for _ in 0..300 {
            t += if r.below(3) == 0 { 1.0 } else { 0.1 };
            let op = r.below(18);
            let res = match op {
                0 => ops::insert(&mut d, s, &rand_text(&mut r, 4), Kind::Type, t),
                1 => crate::indent::newline(&mut d, s, &tabs, r.below(4) != 0, t),
                2 => ops::backspace(&mut d, s, [Reach::Char, Reach::Word, Reach::Line][r.below(3)], &tabs, t),
                3 => ops::delete(&mut d, s, [Reach::Char, Reach::Word, Reach::Line][r.below(3)], t),
                4 => ops::tab(&mut d, s, &tabs, t),
                5 => ops::outdent(&mut d, s, &tabs, t),
                6 => ops::toggle_comment(&mut d, s, ["//", "#", "--"][r.below(3)], t),
                7 => ops::cut(&mut d, s, t).map(|x| x.1),
                8 => {
                    let mut st = EditorState::default();
                    st.v.sel = s;
                    if r.below(2) == 0 {
                        st.undo(&mut d);
                    } else {
                        st.redo(&mut d);
                    }
                    Some(d.clamp(st.v.sel.head)).map(|h| Sel { anchor: d.clamp(st.v.sel.anchor), head: h })
                }
                9 => {
                    let q = crate::find::Query { text: rand_text(&mut r, 2).replace('\n', ""), case: r.below(2) == 0, word: r.below(2) == 0 };
                    crate::find::replace_all(&mut d, &q, &rand_text(&mut r, 2), s, t);
                    None
                }
                10 => {
                    // Cursor moves on the raw line text.
                    let (l, c) = s.head;
                    let line = d.line(l);
                    let c = c as usize;
                    let n = match r.below(4) {
                        0 => next_boundary(line, c),
                        1 => prev_boundary(line, c),
                        2 => word_left(line, c),
                        _ => word_right(line, c),
                    };
                    Some(Sel { anchor: if r.below(2) == 0 { s.anchor } else { (l, n as u32) }, head: (l, n as u32) })
                }
                11 => Some(Sel { anchor: d.clamp(rand_pos(&mut r, &d)), head: d.clamp(rand_pos(&mut r, &d)) }),
                12 => d.replace(rand_pos(&mut r, &d), rand_pos(&mut r, &d), &rand_text(&mut r, 3)).map(|e| Sel::caret(e.end)),
                13 => ops::indent(&mut d, s, &tabs, t),
                14 => crate::indent::closer(&mut d, s, ['}', ']', ')'][r.below(3)], &tabs, t).flatten(),
                15 => ops::duplicate_lines(&mut d, s, r.below(2) == 0, t),
                16 => ops::move_lines(&mut d, s, r.below(2) == 0, t),
                _ => ops::delete_lines(&mut d, s, t),
            };
            if let Some(n) = res {
                s = Sel { anchor: d.clamp(n.anchor), head: d.clamp(n.head) };
            }
            s = Sel { anchor: d.clamp(s.anchor), head: d.clamp(s.head) };
            check(&d, s);
            if std::env::var("FUZZ_DEBUG").is_ok() && d.can_undo() {
                let before = d.text();
                let mut st = EditorState::default();
                st.undo(&mut d);
                st.redo(&mut d);
                assert_eq!(d.text(), before, "round {round} op {op} undo+redo asymmetric");
            }
            let (a, b) = order(s.anchor, s.head);
            let _ = d.buf.slice(a, b);
        }
        // Undo everything: back to the start (exact bytes when unedited-equivalent, else normalised); redo the same count.
        let mut st = EditorState::default();
        let end = d.text();
        let mut n = 0;
        while d.can_undo() && n < 100_000 {
            st.undo(&mut d);
            n += 1;
        }
        assert!(d.text() == start || d.text() == norm, "round {round}: undo-all mismatch");
        assert!(!d.is_dirty(), "round {round}: clean after undoing everything");
        for _ in 0..n {
            st.redo(&mut d);
        }
        assert_eq!(d.text(), end, "round {round}: redo-all mismatch");
    }
}
