//! Headless tests of editor groups: split right / down, focus, tabs per group, move, close, dividers, restore.

mod common;

use common::{sample_files, H};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect};
use margin_gui::{side, Probe};

const CMD: Modifiers = Modifiers::COMMAND;
const NONE: Modifiers = Modifiers::NONE;

fn cs() -> Modifiers {
    Modifiers::COMMAND | Modifiers::SHIFT
}

/// Open through the sidebar names list: the file lands in the focused group.
fn open_file(h: &mut H, path: &str) {
    h.key(Key::E, cs());
    h.type_text(path);
    let want = path.to_owned();
    h.until("names row", move |p| p.side_list.first() == Some(&want));
    h.key(Key::Enter, NONE);
    let name = path.rsplit('/').next().unwrap().to_owned();
    h.until("tab", move |p| p.active.as_deref() == Some(name.as_str()) && p.active_view == "text");
    let f = h.rect_of(side::field_id()).unwrap();
    h.click(f.center());
    h.key(Key::Escape, NONE);
    h.until("tree", |p| !p.side_list_active);
    h.key(Key::Escape, NONE);
    h.frames(1);
}

fn right_click(h: &mut H, pos: Pos2) {
    h.shot.move_to([pos.x, pos.y]);
    h.frames(1);
    for pressed in [true, false] {
        h.shot.event(Event::PointerButton { pos, button: PointerButton::Secondary, pressed, modifiers: NONE });
        h.frames(1);
    }
}

fn ctx_pick(h: &mut H, label: &str) {
    let p = h.until("context menu", |p| !p.ctx_menu.is_empty());
    let i = p.ctx_menu.iter().position(|x| x == label).unwrap_or_else(|| panic!("no {label} in {:?}", p.ctx_menu));
    let steps = p.ctx_menu[..=i].iter().filter(|x| !x.starts_with('(')).count();
    for _ in 0..steps {
        h.key(Key::ArrowDown, NONE);
    }
    h.key(Key::Enter, NONE);
    h.frames(1);
}

fn titles(p: &Probe) -> Vec<Vec<String>> {
    p.groups.iter().map(|g| g.0.clone()).collect()
}

/// Strip rect of the `nth` tab (left to right) of the group at layout position `group`.
fn tab_rect(h: &mut H, group: usize, nth: usize) -> Rect {
    h.frames(1);
    let gid = h.probe().group_ids[group];
    let mut rects: Vec<Rect> = (1..40u64).filter_map(|id| h.rect_of(margin_gui::tabs::tab_id(gid, id))).collect();
    rects.sort_by(|a, b| a.left().total_cmp(&b.left()));
    rects[nth]
}

#[test]
fn split_right_two_views_of_one_buffer() {
    let mut h = H::new(&sample_files());
    h.opened();
    open_file(&mut h, "src/util.rs");
    h.key(Key::Backslash, CMD);
    let p = h.until("split", |p| p.groups.len() == 2 && p.groups[1].2.is_positive());
    assert_eq!(titles(&p), vec![vec!["util.rs"], vec!["util.rs"]]);
    assert_eq!((p.group, p.buffers), (1, 1));
    let (a, b) = (p.groups[0].2, p.groups[1].2);
    assert_eq!((a.right() + 1.0, a.top(), a.bottom()), (b.left(), b.top(), b.bottom()), "side by side, 1 px divider");
    assert!((a.width() - b.width()).abs() <= 2.0, "{a:?} {b:?}");
    // Typing goes to the focused (right) view.
    h.frames(2);
    h.type_text("// right");
    h.key(Key::Enter, NONE);
    let p = h.until("typed", |p| p.active_dirty);
    assert!(p.active_text.unwrap().starts_with("// right\nfn util()"));
    assert_eq!(p.cursor, Some((1, 0)));
    // cmd+1: the left view, with its own cursor, on the same text.
    h.key(Key::Num1, CMD);
    let p = h.until("left", |p| p.group == 0);
    assert_eq!(p.cursor, Some((0, 0)));
    h.frames(2);
    h.type_text("L");
    let p = h.until("typed left", |p| p.active_text.as_deref().is_some_and(|t| t.starts_with("L// right\n")));
    assert_eq!(p.cursor, Some((0, 1)));
    // A press in the right group focuses it; its cursor did not follow the left one.
    let strip = tab_rect(&mut h, 1, 0);
    h.click(Pos2::new(b.right() - 60.0, strip.center().y));
    let p = h.probe();
    assert_eq!((p.group, p.cursor), (1, Some((1, 0))));
    // One buffer: one write.
    h.key(Key::S, CMD);
    h.until("saved", |p| !p.active_dirty);
    assert_eq!(h.fake.writes().len(), 1);
    // Closing one view of a dirty buffer asks nothing; the group goes, the buffer stays.
    h.frames(2);
    h.type_text("x");
    h.until("dirty", |p| p.active_dirty);
    h.key(Key::W, CMD);
    let p = h.until("one group", |p| p.groups.len() == 1);
    assert_eq!((p.modal, p.buffers, p.active_dirty, p.group), (None, 1, true, 0));
    assert_eq!(p.groups[0].2.width(), a.width() + 1.0 + b.width(), "the group left takes the room");
    // The last view: the usual prompt.
    h.key(Key::W, CMD);
    assert_eq!(h.probe().modal.as_deref(), Some("unsaved"));
}

#[test]
fn tabs_per_group_move_and_close() {
    let mut h = H::new(&sample_files());
    h.opened();
    open_file(&mut h, "README.md");
    open_file(&mut h, "src/util.rs");
    // cmd+2 with one group: split.
    h.key(Key::Num2, CMD);
    h.until("split", |p| p.groups.len() == 2);
    open_file(&mut h, "src/main.c");
    let p = h.probe();
    assert_eq!(titles(&p), vec![vec!["README.md", "util.rs"], vec!["util.rs", "main.c"]]);
    assert_eq!((p.group, p.buffers, p.tabs.clone()), (1, 3, vec!["util.rs".to_owned(), "main.c".to_owned()]));
    // A file open in the other group only: a second tab on the same buffer.
    open_file(&mut h, "README.md");
    let p = h.probe();
    assert_eq!(titles(&p)[1], vec!["util.rs", "main.c", "README.md"]);
    assert_eq!(p.buffers, 3);
    // ctrl+tab stays in the group.
    h.key(Key::Tab, Modifiers::CTRL);
    let p = h.probe();
    assert_eq!((p.active.as_deref(), p.groups[0].1.as_deref()), (Some("util.rs"), Some("util.rs")));
    // Click a tab of the other group.
    let r = tab_rect(&mut h, 0, 0);
    h.click(r.center());
    let p = h.probe();
    assert_eq!((p.group, p.active.as_deref()), (0, Some("README.md")));
    // Tab menu: move main.c to the previous group.
    let r = tab_rect(&mut h, 1, 1);
    right_click(&mut h, r.center());
    assert_eq!(h.probe().ctx_menu, vec!["Close", "Close Others", "Close All", "Split Right", "Split Down", "Move to Next Group", "Move to Previous Group", "Copy Path", "Copy Relative Path"]);
    ctx_pick(&mut h, "Move to Previous Group");
    let p = h.probe();
    assert_eq!(titles(&p), vec![vec!["README.md", "main.c", "util.rs"], vec!["util.rs", "README.md"]]);
    assert_eq!((p.group, p.active.as_deref()), (0, Some("main.c")));
    // Close All on the right group: only its tabs; the buffers stay open on the left.
    let r = tab_rect(&mut h, 1, 0);
    right_click(&mut h, r.center());
    ctx_pick(&mut h, "Close All");
    let p = h.until("one group", |p| p.groups.len() == 1);
    assert_eq!((titles(&p), p.buffers), (vec![vec!["README.md".to_owned(), "main.c".to_owned(), "util.rs".to_owned()]], 3));
    // Close Others.
    let r = tab_rect(&mut h, 0, 1);
    right_click(&mut h, r.center());
    ctx_pick(&mut h, "Close Others");
    let p = h.probe();
    assert_eq!((p.tabs, p.buffers), (vec!["main.c".to_owned()], 1));
    // Settings is one tab: no split.
    h.key(Key::Comma, CMD);
    h.key(Key::Backslash, CMD);
    let p = h.probe();
    assert_eq!((p.groups.len(), p.active.as_deref()), (1, Some("Settings")));
}

#[test]
fn split_down_dividers_and_restore() {
    let mut h = H::new(&sample_files());
    h.opened();
    open_file(&mut h, "src/util.rs");
    h.key(Key::Backslash, CMD);
    h.until("split", |p| p.groups.len() == 2);
    open_file(&mut h, "src/main.c");
    // Split button, alt+click: down.
    let gid = h.probe().group_ids[1];
    let r = h.rect_of(margin_gui::split_button_id(gid)).expect("split button");
    let at = r.center();
    h.shot.move_to([at.x, at.y]);
    h.frames(1);
    for pressed in [true, false] {
        h.shot.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::ALT });
        h.frames(1);
    }
    let p = h.until("three", |p| p.groups.len() == 3 && p.groups[2].2.is_positive());
    assert_eq!(titles(&p), vec![vec!["util.rs"], vec!["util.rs", "main.c"], vec!["main.c"]]);
    assert_eq!(p.group, 2);
    let (a, b, c) = (p.groups[0].2, p.groups[1].2, p.groups[2].2);
    assert_eq!((b.left(), b.right(), b.bottom() + 1.0), (c.left(), c.right(), c.top()), "stacked");
    assert_eq!((a.top(), a.bottom(), a.right() + 1.0), (b.top(), c.bottom(), b.left()));
    // Drag the vertical divider 100 px to the left.
    let d = h.rect_of(margin_gui::group_divider_id(&[], 0)).expect("divider").center();
    h.shot.move_to([d.x, d.y]);
    h.frames(1);
    h.shot.press_at([d.x, d.y], NONE);
    h.frames(1);
    h.shot.move_to([d.x - 100.0, d.y]);
    h.frames(2);
    h.shot.release();
    h.frames(2);
    let p = h.probe();
    assert!((p.groups[0].2.width() - (a.width() - 100.0)).abs() <= 2.0, "{:?} → {:?}", a, p.groups[0].2);
    assert!((p.groups[2].2.width() - (c.width() + 100.0)).abs() <= 2.0);
    // The cursor of each view is its own: put the bottom view on line 8.
    h.key(Key::G, Modifiers::CTRL);
    h.type_text("9");
    h.key(Key::Enter, NONE);
    h.until("line 9", |p| p.cursor == Some((8, 0)));
    h.frames(3);
    h.png("groups.png");
    // Quit, start again: groups, tabs, sizes and focus are back.
    let widths: Vec<f32> = h.probe().groups.iter().map(|g| g.2.width()).collect();
    h.app.on_exit();
    h.app.flush_config();
    let state = std::fs::read_to_string(h.cfg.path().join("state.conf")).unwrap();
    for want in ["tab = src/util.rs\nactive = src/util.rs\ngroup = 1\ntab = src/util.rs\ntab = src/main.c\nactive = src/main.c\ngroup = 2\ntab = src/main.c\nactive = src/main.c\nlayout = r(0:", ",d(1:500,2:500):", "focus = 2\n"] {
        assert!(state.contains(want), "{want:?} not in {state}");
    }
    let mut h2 = h.restart(Some(common::ROOT), false);
    let p = h2.until("restored", |p| p.groups.len() == 3 && p.active_view == "text" && p.groups[2].2.is_positive());
    assert_eq!(titles(&p), vec![vec!["util.rs"], vec!["util.rs", "main.c"], vec!["main.c"]]);
    assert_eq!((p.group, p.buffers, p.active.as_deref()), (2, 2, Some("main.c")));
    for (g, w) in p.groups.iter().zip(&widths) {
        assert!((g.2.width() - w).abs() <= 2.0, "{:?} vs {w}", g.2);
    }
    // The next / previous group commands walk the layout.
    h2.key(Key::Num1, CMD);
    assert_eq!(h2.probe().group, 0);
    h2.key(Key::Num3, CMD);
    assert_eq!(h2.probe().group, 2);
}

#[test]
fn markdown_preview_is_per_group() {
    let files = vec![("a.md", "# Top\n\nText.\n")];
    let mut h = H::new(&files);
    h.opened();
    open_file(&mut h, "a.md");
    h.key(Key::V, cs());
    h.until("parsed", |p| p.preview_on && p.preview_parsed);
    // The new view starts like the one it was split from.
    h.key(Key::Backslash, CMD);
    let p = h.until("split", |p| p.groups.len() == 2);
    assert!(p.preview_on && p.preview_parsed);
    // Off on the right only.
    h.key(Key::V, cs());
    assert!(!h.probe().preview_on);
    h.key(Key::Num1, CMD);
    assert!(h.probe().preview_on);
    h.key(Key::Num2, CMD);
    assert!(!h.probe().preview_on);
    // The toggle of the unfocused group: focuses it and flips its preview.
    let gid = h.probe().group_ids[0];
    let r = h.rect_of(margin_gui::preview_toggle_id(gid)).expect("toggle of the left group");
    h.click(r.center());
    let p = h.probe();
    assert_eq!((p.group, p.preview_on), (0, false));
    h.key(Key::Num2, CMD);
    assert!(!h.probe().preview_on);
}

fn drag(h: &mut H, from: Pos2, to: Pos2) {
    h.shot.move_to([from.x, from.y]);
    h.frames(1);
    h.shot.press_at([from.x, from.y], NONE);
    h.frames(1);
    let mid = from + (to - from) * 0.5;
    h.shot.move_to([mid.x, mid.y]);
    h.frames(2);
    h.shot.move_to([to.x, to.y]);
    h.frames(2);
}

fn drop(h: &mut H) {
    h.shot.release();
    h.frames(3);
}

#[test]
fn drag_tabs_between_groups() {
    let mut h = H::new(&sample_files());
    h.opened();
    for f in ["README.md", "src/util.rs", "src/main.c"] {
        open_file(&mut h, f);
    }
    let names = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
    // Onto the right edge of the body: a new group there.
    let from = tab_rect(&mut h, 0, 2).center();
    let body = h.probe().groups[0].2;
    drag(&mut h, from, Pos2::new(body.right() - 30.0, body.center().y));
    assert_eq!(h.probe().groups.len(), 1, "nothing moves before the drop");
    drop(&mut h);
    let p = h.until("split", |p| p.groups.len() == 2 && p.groups[1].2.is_positive());
    assert_eq!(titles(&p), vec![names(&["README.md", "util.rs"]), names(&["main.c"])]);
    assert_eq!((p.group, p.buffers, p.active.as_deref()), (1, 3, Some("main.c")));
    // Onto the middle of the other group: into it.
    let from = tab_rect(&mut h, 0, 1).center();
    let to = h.probe().groups[1].2.center();
    drag(&mut h, from, to);
    drop(&mut h);
    let p = h.probe();
    assert_eq!(titles(&p), vec![names(&["README.md"]), names(&["main.c", "util.rs"])]);
    assert_eq!((p.group, p.active.as_deref()), (1, Some("util.rs")));
    // Along the strip: reorder.
    let from = tab_rect(&mut h, 1, 1).center();
    let first = tab_rect(&mut h, 1, 0);
    drag(&mut h, from, Pos2::new(first.left() + 4.0, first.center().y));
    drop(&mut h);
    assert_eq!(titles(&h.probe())[1], names(&["util.rs", "main.c"]));
    // Esc cancels.
    let from = tab_rect(&mut h, 1, 0).center();
    let to = h.probe().groups[0].2.center();
    drag(&mut h, from, to);
    h.key(Key::Escape, NONE);
    drop(&mut h);
    assert_eq!(titles(&h.probe()), vec![names(&["README.md"]), names(&["util.rs", "main.c"])]);
    // The last tab of a group dragged away: the group goes. Typing lands in the moved tab.
    let from = tab_rect(&mut h, 0, 0).center();
    let strip = tab_rect(&mut h, 1, 1);
    drag(&mut h, from, Pos2::new(strip.right() + 40.0, strip.center().y));
    drop(&mut h);
    let p = h.until("one group", |p| p.groups.len() == 1);
    assert_eq!((titles(&p), p.active.as_deref()), (vec![names(&["util.rs", "main.c", "README.md"])], Some("README.md")));
    h.frames(2);
    h.type_text("typed ");
    h.until("typed", |p| p.active_text.as_deref().is_some_and(|t| t.starts_with("typed # Proj")));
    // A plain click on a tab still just activates it.
    let r = tab_rect(&mut h, 0, 0);
    h.click(r.center());
    assert_eq!(h.probe().active.as_deref(), Some("util.rs"));
}
