//! Behaviour through `show` in a headless egui.

mod common;

use common::{cmd, shift, H, SHOTS};
use egui::{Event, Key, Modifiers, Pos2};
use margin_code::Lang;
use margin_editor::{Lens, LensSeg, LensTone};

#[test]
fn type_navigate_select_copy_paste_undo() {
    let mut h = H::new("hello world\nsecond line", Lang::Plain);
    assert!(h.out.focused);
    h.typ("abc");
    assert_eq!(h.doc.line(0), "abchello world");
    assert!(h.out.changed);
    assert_eq!(h.st.cursor(), (0, 3));
    h.key(Key::ArrowRight, shift());
    h.key(Key::ArrowRight, shift());
    assert_eq!(h.st.selection(), Some(((0, 3), (0, 5))));
    h.ev(Event::Copy);
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("he"));
    // Copy with no selection: the whole line.
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(h.st.cursor().0, 1);
    h.ev(Event::Copy);
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("second line\n"));
    h.key(Key::End, Modifiers::NONE);
    assert_eq!(h.st.cursor(), (1, 11));
    h.shot.paste("X\r\nY");
    h.frame();
    assert_eq!(h.doc.text(), "abchello world\nsecond lineX\nY");
    h.key(Key::Z, cmd());
    assert_eq!(h.doc.text(), "abchello world\nsecond line");
    assert_eq!(h.st.cursor(), (1, 11));
    h.key(Key::Z, cmd());
    assert_eq!(h.doc.text(), "hello world\nsecond line");
    assert!(!h.doc.is_dirty());
    h.key(Key::Z, Modifiers { shift: true, ..cmd() });
    assert_eq!(h.doc.text(), "abchello world\nsecond line");
    // Word movement (alt on macOS, ctrl elsewhere) and line start.
    let word = if cfg!(target_os = "macos") { Modifiers::ALT } else { Modifiers::CTRL };
    h.st.goto(0, 0);
    h.frame();
    h.key(Key::ArrowRight, word);
    assert_eq!(h.st.cursor(), (0, 8));
    h.key(Key::Home, Modifiers::NONE);
    assert_eq!(h.st.cursor(), (0, 0));
    // Enter keeps indentation; Tab / Shift+Tab.
    h.typ("    ");
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(h.st.cursor(), (1, 4));
    h.key(Key::Tab, shift());
    assert_eq!(h.doc.line(1), "abchello world");
    // Cut the line, select all.
    h.ev(Event::Cut);
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("abchello world\n"));
    h.key(Key::A, cmd());
    assert_eq!(h.st.selected_text(&h.doc), h.doc.text());
    h.key(Key::Backspace, Modifiers::NONE);
    assert_eq!(h.doc.text(), "");
}

#[test]
fn shell_keys_are_left_alone() {
    let mut h = H::new("x", Lang::Plain);
    h.shot.key(Key::S, cmd()).key(Key::P, cmd());
    let mut seen = 0;
    let (doc, st, opts, theme) = (&mut h.doc, &mut h.st, &h.opts, &h.theme);
    h.shot.run(1, |ui| {
        margin_editor::show(ui, doc, st, opts, theme, &[]);
        seen = ui.input(|i| i.events.iter().filter(|e| matches!(e, Event::Key { key: Key::S | Key::P, .. })).count());
    });
    assert_eq!(seen, 4, "press+release of cmd+S and cmd+P must stay in the input");
}

#[test]
fn mouse_click_drag_double_triple_gutter() {
    let mut h = H::new("alpha beta gamma\nsecond line here\nthird", Lang::Plain);
    let a = h.caret_at((0, 0)).center();
    let b = h.caret_at((1, 6)).center();
    h.st.goto(2, 0);
    h.frame();
    // Drag from a to b.
    h.shot.press_at([a.x + 1.0, a.y], Modifiers::NONE);
    h.frame();
    h.shot.move_to([b.x + 1.0, b.y]);
    h.frame();
    h.shot.release();
    h.frame();
    assert_eq!(h.st.selection(), Some(((0, 0), (1, 6))));
    assert_eq!(h.st.selected_text(&h.doc), "alpha beta gamma\nsecond");
    // Shift+click extends.
    let c = h.caret_at((2, 3)).center();
    h.st.goto(0, 0);
    h.frame();
    h.click(Pos2::new(c.x + 1.0, c.y), shift());
    assert_eq!(h.st.selection(), Some(((0, 0), (2, 3))));
    // Double click: word. Triple: line.
    let w = h.caret_at((0, 8)).center();
    h.shot.press_at([w.x, w.y], Modifiers::NONE);
    h.frame();
    h.shot.release();
    h.frame();
    h.shot.press_at([w.x, w.y], Modifiers::NONE);
    h.frame();
    assert_eq!(h.st.selected_text(&h.doc), "beta");
    h.shot.release();
    h.frame();
    h.shot.press_at([w.x, w.y], Modifiers::NONE);
    h.frame();
    h.shot.release();
    h.frame();
    assert_eq!(h.st.selected_text(&h.doc), "alpha beta gamma\n");
    // Gutter click selects the line.
    let l1 = h.caret_at((1, 0));
    h.frames(30); // past the double-click window
    h.click(Pos2::new(5.0, l1.center().y), Modifiers::NONE);
    assert_eq!(h.st.selection(), Some(((1, 0), (2, 0))));
}

#[test]
fn lens_click_cmd_click_and_f12() {
    let mut h = H::new("fn main() {\n    helper(1);\n}\nfn helper(x: u8) {}\n", Lang::Rust);
    h.lenses = vec![Lens {
        line: 3,
        segs: vec![
            LensSeg { text: "2 callers".into(), tone: LensTone::Normal, id: 7, hover: "callers".into() },
            LensSeg { text: "indexing…".into(), tone: LensTone::Faint, id: 9, hover: String::new() },
        ],
    }];
    h.frames(2);
    let r = h.caret_at((3, 0));
    let lens_y = r.top() - 6.0;
    // Clicking a lens segment reports it and leaves the cursor alone.
    h.click(Pos2::new(r.left() + 3.0, lens_y), Modifiers::NONE);
    assert!(h.any_out(|o| o.lens_clicked == Some((3, 7))), "lens click not reported");
    assert_eq!(h.st.cursor(), (3, 0));
    // Cmd+click on `helper` in line 1.
    let p = h.caret_at((1, 6)).center();
    h.st.goto(0, 0);
    h.frame();
    h.outs.clear();
    h.click(Pos2::new(p.x + 2.0, p.y), cmd());
    assert!(h.any_out(|o| o.goto_definition == Some(("helper".to_owned(), (1, 4)))), "{:?}", h.outs.last());
    h.outs.clear();
    h.st.goto(3, 5);
    h.frame();
    assert_eq!(h.out.word_at_cursor, "helper");
    h.key(Key::F12, Modifiers::NONE);
    assert!(h.any_out(|o| o.goto_definition == Some(("helper".to_owned(), (3, 3)))));
}

#[test]
fn goto_scrolls_and_scroll_wheel() {
    let text: String = (0..10_000).map(|i| format!("line {i}\n")).collect();
    let mut h = H::new(&text, Lang::Rust);
    h.st.goto(5000, 2);
    h.frames(2);
    let top = h.st.top_line();
    assert!((4970..5000).contains(&top), "top {top}");
    assert!(h.st.cursor_rect().is_some());
    // Already visible: no jump.
    h.st.goto(top + 2, 0);
    h.frame();
    assert_eq!(h.st.top_line(), top);
    // Wheel.
    h.shot.move_to([300.0, 300.0]);
    h.shot.scroll([0.0, -400.0]);
    h.frames(20);
    assert!(h.st.top_line() > top + 5, "wheel did not scroll: {} -> {}", top, h.st.top_line());
    // Out-of-range goto is clamped; end of document stays reachable.
    h.st.goto(u32::MAX, u32::MAX);
    h.frames(2);
    assert_eq!(h.st.cursor(), (10_000, 0));
    // PageUp moves up a screen.
    let before = h.st.cursor().0;
    h.key(Key::PageUp, Modifiers::NONE);
    assert!(h.st.cursor().0 + 10 < before);
}

#[test]
fn find_bar_finds_and_replaces() {
    let mut h = H::new("foo bar\nFoo baz foo\nnothing", Lang::Plain);
    h.st.open_find(Some("foo"), true);
    h.frames(3);
    assert!(h.st.find_open());
    assert_eq!(h.st.selection(), Some(((0, 0), (0, 3))));
    assert_eq!(h.st.find_status(), "1 of 3");
    // Enter in the query field = next.
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(h.st.selection(), Some(((1, 0), (1, 3))));
    h.key(Key::Enter, shift());
    assert_eq!(h.st.selection(), Some(((0, 0), (0, 3))));
    h.st.find_next(&h.doc);
    h.st.find_next(&h.doc);
    assert_eq!(h.st.selection(), Some(((1, 8), (1, 11))));
    h.frame();
    assert_eq!(h.st.find_status(), "3 of 3");
    // Typing in the replace field: click it first.
    let rf = h.widget_rect("find-replace").expect("replace field");
    h.click(rf.center(), Modifiers::NONE);
    h.typ("qux");
    let all = h.widget_rect("find-repall").expect("replace all button");
    h.click(all.center(), Modifiers::NONE);
    assert_eq!(h.doc.text(), "qux bar\nqux baz qux\nnothing");
    // One undo group.
    h.st.undo(&mut h.doc);
    assert_eq!(h.doc.text(), "foo bar\nFoo baz foo\nnothing");
    // Esc closes and gives focus back to the text.
    let q = h.widget_rect("find-query").expect("query field");
    h.click(q.center(), Modifiers::NONE);
    h.key(Key::Escape, Modifiers::NONE);
    h.frame();
    assert!(!h.st.find_open());
    assert!(h.out.focused);
    h.st.open_find(Some("zzz"), false);
    h.frames(3);
    assert_eq!(h.st.find_status(), "No results");
}

#[test]
fn read_only_ignores_typing() {
    let mut h = H::new("keep", Lang::Plain);
    h.doc.set_read_only(true);
    h.typ("x");
    h.key(Key::Backspace, Modifiers::NONE);
    h.key(Key::Enter, Modifiers::NONE);
    h.shot.paste("pasted");
    h.frame();
    h.ev(Event::Cut);
    h.key(Key::Z, cmd());
    assert_eq!(h.doc.text(), "keep");
    assert!(!h.out.changed);
    // Navigation and copy still work.
    h.key(Key::End, shift());
    h.ev(Event::Copy);
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("keep"));
}

#[test]
fn no_repaint_when_idle() {
    let mut h = H::new("fn a() {}\nlet x = 1;\n", Lang::Rust);
    h.frames(5);
    assert!(!h.shot.ctx().has_requested_repaint(), "repaint requested while idle");
    // A big file being lexed does ask for repaints, then stops.
    let text: String = (0..300_000).map(|i| format!("let v{i} = {i};\n")).collect();
    let mut h = H::new(&text, Lang::Rust);
    h.st.goto(299_000, 0);
    let mut n = 0;
    loop {
        h.frame();
        n += 1;
        if !h.shot.ctx().has_requested_repaint() || n > 5000 {
            break;
        }
    }
    assert!(n < 5000, "never went idle");
}

#[test]
fn ime_preedit_and_commit() {
    let mut h = H::new("ab", Lang::Plain);
    h.st.goto(0, 1);
    h.frame();
    h.ev(Event::Ime(egui::ImeEvent::Preedit { text: "にほ".into(), active_range_chars: None }));
    assert_eq!(h.doc.text(), "ab", "preedit is not text");
    let with = h.st.cursor_rect().unwrap();
    h.ev(Event::Ime(egui::ImeEvent::Commit("日本".into())));
    assert_eq!(h.doc.text(), "a日本b");
    assert_eq!(h.st.cursor(), (0, 7));
    assert!(with.left() > 0.0);
}

#[test]
fn hostile_text_renders_and_survives_input() {
    let nasty = "a\u{202E}b\u{2066}c\u{200B}\0\x1b[31m\u{FEFF}\té\u{301}\u{E0041}\r\n世界\u{7f}";
    let mut h = H::new(nasty, Lang::Plain);
    for k in [Key::ArrowRight, Key::ArrowRight, Key::ArrowRight, Key::ArrowDown, Key::End, Key::ArrowLeft, Key::Backspace, Key::Delete, Key::Home] {
        h.key(k, Modifiers::NONE);
        let (l, c) = h.st.cursor();
        assert!(h.doc.line(l).is_char_boundary(c as usize));
    }
    h.typ("ok");
    match h.shot.png(format!("{SHOTS}/hostile.png")) {
        Err(margin_shot::ShotError::NoGpu(why)) => eprintln!("skip png: {why}"),
        r => r.unwrap(),
    }
}

#[test]
fn soft_wrap_visual_rows() {
    let long = (0..60).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ");
    let text = format!("{long}\nnext");
    let mut h = H::with_size(&text, Lang::Markdown, [320.0, 400.0]);
    h.st.goto(0, 0);
    h.frame();
    let r0 = h.st.cursor_rect().unwrap();
    h.key(Key::ArrowDown, Modifiers::NONE);
    let (l, c) = h.st.cursor();
    assert_eq!(l, 0, "down moves by visual row inside a wrapped line");
    assert!(c > 0);
    let r1 = h.st.cursor_rect().unwrap();
    assert!(r1.top() > r0.top() && (r1.left() - r0.left()).abs() < 2.0, "{r0:?} {r1:?}");
    // End: end of the visual row, not of the line. Again: end of the line.
    h.key(Key::End, Modifiers::NONE);
    let (l, c_end) = h.st.cursor();
    assert!(l == 0 && (c_end as usize) < long.len(), "{c_end}");
    let r2 = h.st.cursor_rect().unwrap();
    assert!((r2.top() - r1.top()).abs() < 1.0, "End stays on its visual row");
    h.key(Key::End, Modifiers::NONE);
    assert_eq!(h.st.cursor(), (0, long.len() as u32));
    // Home: start of the visual row first.
    h.key(Key::Home, Modifiers::NONE);
    let (_, c_home) = h.st.cursor();
    assert!(c_home > 0);
    h.key(Key::Home, Modifiers::NONE);
    assert_eq!(h.st.cursor(), (0, 0));
    // Many rows down reaches the next line.
    for _ in 0..40 {
        h.key(Key::ArrowDown, Modifiers::NONE);
    }
    assert_eq!(h.st.cursor().0, 1);
    match h.shot.png(format!("{SHOTS}/wrap-narrow.png")) {
        Err(margin_shot::ShotError::NoGpu(why)) => eprintln!("skip png: {why}"),
        r => r.unwrap(),
    }
}

#[test]
fn random_input_never_breaks_invariants() {
    struct R(u64);
    impl R {
        fn n(&mut self, k: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % k as u64) as usize
        }
    }
    let pieces = ["a", " ", "\t", "é", "e\u{301}", "世", "\u{202E}", "\u{200B}", "\0", "{", "foo", "😀", "\u{FEFF}"];
    let keys = [Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::ArrowDown, Key::Home, Key::End, Key::PageUp, Key::PageDown, Key::Backspace, Key::Delete, Key::Enter, Key::Tab, Key::Z, Key::A, Key::Slash, Key::F12, Key::Escape];
    let mods = [Modifiers::NONE, Modifiers::SHIFT, Modifiers::COMMAND, Modifiers::ALT, Modifiers { shift: true, ..Modifiers::COMMAND }];
    let start: String = (0..200).map(|i| format!("{}{}\n", pieces[i % pieces.len()].repeat(i % 7), "x".repeat(i % 50))).collect();
    let mut r = R(0x9E3779B97F4A7C15);
    for (round, lang) in [Lang::Rust, Lang::Markdown].into_iter().enumerate() {
        let mut h = H::with_size(&start, lang, [500.0, 300.0]);
        h.lenses = (0..40).map(|i| Lens { line: i * 5, segs: vec![LensSeg { text: format!("{i} callers"), ..Default::default() }] }).collect();
        h.opts.show_whitespace = round == 1;
        for step in 0..1500 {
            match r.n(10) {
                0..=3 => {
                    let k = keys[r.n(keys.len())];
                    let m = mods[r.n(mods.len())];
                    h.shot.key(k, m);
                }
                4 | 5 => {
                    h.shot.text(pieces[r.n(pieces.len())]);
                }
                6 => {
                    let p = [r.n(500) as f32, r.n(300) as f32];
                    if r.n(2) == 0 {
                        h.shot.press_at(p, mods[r.n(2)]);
                    } else {
                        h.shot.move_to(p).release();
                    }
                }
                7 => {
                    h.shot.move_to([250.0, 150.0]).scroll([r.n(200) as f32 - 100.0, r.n(2000) as f32 - 1000.0]);
                }
                8 => {
                    let e = [Event::Copy, Event::Cut, Event::Paste("p\r\nq".into())][r.n(3)].clone();
                    h.shot.event(e);
                }
                _ => match r.n(6) {
                    0 => h.st.goto(r.n(400) as u32, r.n(80) as u32),
                    1 => h.st.open_find(Some(pieces[r.n(pieces.len())]), r.n(2) == 0),
                    2 => h.st.find_next(&h.doc),
                    3 => h.st.select(r.n(300) as u32, r.n(30) as u32, r.n(100) as u32),
                    4 => h.st.close_find(),
                    _ => {
                        h.shot.event(Event::Ime(egui::ImeEvent::Preedit { text: "かな".into(), active_range_chars: None }));
                    }
                },
            }
            h.frame();
            let (l, c) = h.st.cursor();
            assert!(l < h.doc.line_count(), "step {step}");
            assert!(h.doc.line(l).is_char_boundary(c as usize), "step {step}: ({l}, {c})");
            assert_eq!(h.doc.len_bytes(), h.doc.text().len());
        }
    }
}
