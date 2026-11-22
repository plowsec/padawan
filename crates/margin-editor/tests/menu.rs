//! Context menu through `show`: open, choose (mouse / keys), close, disabled rows, outputs, placement, screenshots.

mod common;

use common::H;
use egui::{Key, Modifiers, Pos2, Rect};
use margin_code::Lang;
use margin_theme::{Look, Mode, Theme};

const NONE: Modifiers = Modifiers::NONE;
const SRC: &str = "fn main() {\n    helper(1);\n}\nfn helper(x: u8) {}\n";

/// Screen point on the character at `p`.
fn at(h: &mut H, p: (u32, u32)) -> Pos2 {
    let r = h.caret_at(p);
    Pos2::new(r.center().x + 3.0, r.center().y)
}

fn choose(h: &mut H, row: usize) {
    let r = h.menu_row(row).expect("menu row");
    h.click(r.center(), NONE);
    h.frame();
}

#[test]
fn right_click_inside_selection_keeps_it_outside_moves_caret() {
    let mut h = H::new(SRC, Lang::Rust);
    let inside = at(&mut h, (1, 6));
    let outside = at(&mut h, (3, 1));
    h.st.set_selection((1, 4), (1, 10));
    h.frame();
    h.right_click(inside);
    assert!(h.st.context_menu_open());
    assert_eq!(h.st.selection(), Some(((1, 4), (1, 10))), "inside: selection kept");
    h.key(Key::Escape, NONE);
    assert!(!h.st.context_menu_open());
    h.frame();
    assert!(h.out.focused, "Esc gives focus back to the text");
    assert_eq!(h.st.selection(), Some(((1, 4), (1, 10))));
    h.right_click(outside);
    assert!(h.st.context_menu_open());
    assert_eq!((h.st.selection(), h.st.cursor()), (None, (3, 1)), "outside: caret moved, selection cleared");
}

#[test]
fn choose_with_mouse_and_keys() {
    let mut h = H::new(SRC, Lang::Rust);
    let p = at(&mut h, (1, 6));
    h.st.set_selection((1, 4), (1, 10));
    h.frame();
    // Copy (row 1) by click.
    h.right_click(p);
    choose(&mut h, 1);
    assert!(!h.st.context_menu_open(), "closes after choosing");
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("helper"));
    // Keys: ↓ ↓ = Copy, ↑ = Cut, Enter. Arrows do not move the cursor while open.
    h.right_click(p);
    h.key(Key::ArrowDown, NONE);
    h.key(Key::ArrowDown, NONE);
    h.key(Key::ArrowUp, NONE);
    assert_eq!(h.st.selection(), Some(((1, 4), (1, 10))));
    h.key(Key::Enter, NONE);
    assert!(!h.st.context_menu_open());
    assert_eq!(h.doc.line(1), "    (1);", "Cut chosen with Enter");
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("helper"));
    assert!(h.out.changed);
    // Select All (row 3).
    h.right_click(p);
    choose(&mut h, 3);
    assert_eq!(h.st.selected_text(&h.doc), h.doc.text());
    // Toggle Comment (last row here: no identifier under the cursor after Select All? head = end of doc).
    let q = at(&mut h, (0, 1));
    h.right_click(q);
    let last = (0..8).rev().find(|&i| h.menu_row(i).is_some()).unwrap();
    choose(&mut h, last);
    assert_eq!(h.doc.line(0), "// fn main() {");
    // Paste (row 2): focus now, text arrives later as Event::Paste.
    h.right_click(q);
    choose(&mut h, 2);
    assert!(h.out.focused);
    h.shot.paste("P");
    h.frame();
    assert!(h.doc.line(0).contains('P'), "{}", h.doc.line(0));
}

#[test]
fn goto_definition_and_find_callers() {
    let mut h = H::new(SRC, Lang::Rust);
    let p = at(&mut h, (1, 6));
    h.st.goto(0, 0);
    h.frame();
    h.right_click(p);
    assert_eq!(h.st.cursor(), (1, 6));
    h.outs.clear();
    choose(&mut h, 4);
    assert!(h.any_out(|o| o.goto_definition == Some(("helper".to_owned(), (1, 4)))), "{:?}", h.outs);
    h.outs.clear();
    h.right_click(p);
    choose(&mut h, 5);
    assert!(h.any_out(|o| o.find_callers.as_deref() == Some("helper")));
    assert_eq!(h.doc.text(), SRC);
    // Keyboard: the same rows.
    h.outs.clear();
    h.right_click(p);
    for _ in 0..5 {
        h.key(Key::ArrowDown, NONE);
    }
    h.key(Key::Enter, NONE);
    assert!(h.any_out(|o| o.goto_definition.is_some()));
}

#[test]
fn rows_depend_on_identifier_and_language() {
    // No identifier under the click: no Go to Definition / Find Callers.
    let mut h = H::new("fn a() {}\n\n", Lang::Rust);
    let p = at(&mut h, (1, 0));
    h.right_click(p);
    assert!(h.menu_row(4).is_some() && h.menu_row(5).is_none(), "Cut Copy Paste | Select All | Toggle Comment");
    // Markdown: no outline, no line comment.
    let mut h = H::new("word here\n", Lang::Markdown);
    let p = at(&mut h, (0, 1));
    h.right_click(p);
    assert!(h.menu_row(3).is_some() && h.menu_row(4).is_none());
}

#[test]
fn read_only_rows_are_disabled() {
    let mut h = H::new(SRC, Lang::Rust);
    h.doc.set_read_only(true);
    let p = at(&mut h, (1, 6));
    h.st.set_selection((1, 4), (1, 10));
    h.frame();
    h.right_click(p);
    h.png("context-menu-readonly.png");
    let n = h.shot.copied.len();
    for row in [0, 2, 6] {
        choose(&mut h, row);
        assert!(h.st.context_menu_open(), "disabled row {row} keeps the menu open");
    }
    assert_eq!((h.doc.text().as_str(), h.shot.copied.len()), (SRC, n));
    // ↓ skips Cut: first enabled row is Copy.
    h.key(Key::ArrowDown, NONE);
    h.key(Key::Enter, NONE);
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("helper"));
    assert_eq!(h.doc.text(), SRC);
}

#[test]
fn closes_on_scroll_click_outside_typing_and_option_off() {
    let text: String = (0..200).map(|i| format!("let v{i} = {i};\n")).collect();
    let mut h = H::new(&text, Lang::Rust);
    let p = at(&mut h, (2, 5));
    let q = at(&mut h, (5, 2));
    h.right_click(p);
    assert!(h.st.context_menu_open());
    h.shot.scroll([0.0, -40.0]);
    h.frame();
    assert!(!h.st.context_menu_open(), "wheel closes");
    h.frames(30); // smooth scrolling settles
    h.st.set_top_line(0);
    h.frame();
    h.right_click(p);
    h.click(q, NONE);
    assert!(!h.st.context_menu_open(), "press outside closes");
    assert_eq!(h.st.cursor(), (5, 2), "and the click still lands");
    h.right_click(p);
    h.typ("z");
    assert!(!h.st.context_menu_open(), "typing closes");
    assert_eq!(h.doc.line(2), "let vz2 = 2;", "and the text goes in");
    // Secondary click elsewhere while open: reopens there.
    h.right_click(p);
    h.right_click(q);
    assert!(h.st.context_menu_open());
    assert_eq!(h.st.cursor(), (5, 2));
    // Option off.
    h.key(Key::Escape, NONE);
    h.opts.context_menu = false;
    h.right_click(p);
    assert!(!h.st.context_menu_open());
    assert_eq!(h.st.cursor(), (5, 2), "no menu, no caret move");
}

/// Menu bounds = union of its rows.
fn menu_bounds(h: &H) -> Rect {
    (0..8).filter_map(|i| h.menu_row(i)).fold(Rect::NOTHING, |a, r| a.union(r))
}

#[test]
fn placement_near_edges_and_screenshots() {
    for (mode, name) in [(Mode::Dark, "dark"), (Mode::Light, "light")] {
        let mut h = H::with_size(SRC, Lang::Rust, [640.0, 360.0]);
        h.theme = Theme::new(mode, Look::default());
        let p = at(&mut h, (1, 6));
        h.st.set_selection((1, 4), (1, 10));
        h.frame();
        h.right_click(p);
        let b = menu_bounds(&h);
        assert!(b.left() > p.x && b.top() > p.y, "opens right/below: {b:?}");
        // Hover Copy.
        let c = h.menu_row(1).unwrap().center();
        h.shot.move_to([c.x, c.y]);
        h.frames(2);
        h.png(&format!("context-menu-{name}.png"));
        h.key(Key::Escape, NONE);
        // Bottom-right corner: flipped, inside the window.
        let corner = Pos2::new(620.0, 350.0);
        h.right_click(corner);
        assert!(h.st.context_menu_open());
        let b = menu_bounds(&h);
        assert!(b.right() <= 640.0 && b.bottom() <= 360.0 && b.left() >= 0.0 && b.top() >= 0.0, "{b:?}");
        assert!(b.right() < corner.x && b.bottom() < corner.y, "flipped left/up: {b:?}");
        h.png(&format!("context-menu-corner-{name}.png"));
    }
}
