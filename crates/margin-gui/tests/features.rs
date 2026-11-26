//! Headless tests of the shell features: sidebar names list, search tabs + replace, untitled + Save As + hot exit,
//! menu bar, context menus, call paths view, new window, Markdown preview wiring, settings, hostile input.

mod common;

use std::sync::Arc;

use common::{sample_files, Script, H};
use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use margin_editor::EditAction;
use margin_gui::hints::{SEG_CALLERS, SEG_SINKS};
use margin_gui::{menubar, search_panel, side};
use margin_md::PreviewOutput;
use margin_proto::{CallPath, FileMatch, FlowArg, Hop, RelPath, Request, Site};

const CMD: Modifiers = Modifiers::COMMAND;
const NONE: Modifiers = Modifiers::NONE;

fn cs() -> Modifiers {
    Modifiers::COMMAND | Modifiers::SHIFT
}

fn right_click(h: &mut H, pos: Pos2) {
    h.shot.move_to([pos.x, pos.y]);
    h.frames(1);
    h.shot.event(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, modifiers: NONE });
    h.frames(1);
    h.shot.event(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: false, modifiers: NONE });
    h.frames(1);
}

fn click_id(h: &mut H, id: egui::Id) {
    h.frames(1);
    let r = h.rect_of(id).unwrap_or_else(|| panic!("no widget {id:?}"));
    h.click(r.center());
}

/// Line of the hint with a `sinks` segment, once the engine's caller count is in: that segment comes later
/// and moves the others to the right.
fn sinks_hint(h: &mut H) -> u32 {
    let full = |l: &(u32, Vec<String>)| l.1.iter().any(|s| s.starts_with("sinks")) && l.1.iter().any(|s| s.ends_with("callers"));
    let p = h.until("hints with engine numbers", |p| p.lenses.iter().any(full));
    p.lenses.iter().find(|l| full(l)).unwrap().0
}

fn click_button(h: &mut H, label: &str) {
    let p = h.until("modal buttons", |p| !p.modal_buttons.is_empty());
    let r = p.modal_buttons.iter().find(|b| b.0 == label).unwrap_or_else(|| panic!("no button {label}: {:?}", p.modal_buttons)).1;
    h.click(r.center());
}

fn ctx_pick(h: &mut H, label: &str) {
    let p = h.until("context menu", |p| !p.ctx_menu.is_empty());
    let i = p.ctx_menu.iter().position(|x| x == label).unwrap_or_else(|| panic!("no {label} in {:?}", p.ctx_menu));
    // Separators are not in the probe list, disabled items are `(…)` and skipped: walk with the keyboard.
    let steps = p.ctx_menu[..=i].iter().filter(|x| !x.starts_with('(')).count();
    for _ in 0..steps {
        h.key(Key::ArrowDown, NONE);
    }
    h.key(Key::Enter, NONE);
    assert!(h.probe().ctx_menu.is_empty(), "menu closes after a pick");
}

fn open_file(h: &mut H, path: &str) {
    h.key(Key::E, cs());
    h.type_text(path);
    let want = path.to_owned();
    h.until("names row", move |p| p.side_list.first() == Some(&want));
    h.key(Key::Enter, NONE);
    let name = path.rsplit('/').next().unwrap().to_owned();
    h.until("tab", move |p| p.active.as_deref() == Some(name.as_str()) && p.active_view == "text");
    // Back to the tree.
    let f = h.rect_of(side::field_id()).unwrap();
    h.click(f.center());
    h.key(Key::Escape, NONE);
    h.until("tree", |p| !p.side_list_active);
    // Second Esc: back to the editor.
    h.key(Key::Escape, NONE);
    h.frames(1);
}

fn finds(h: &H) -> Vec<(String, String, String, u32)> {
    h.fake
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            Request::FindFiles { query, include, exclude, limit } if limit == side::NAMES_LIMIT => Some((query, include, exclude, limit)),
            _ => None,
        })
        .collect()
}

// ---- A. sidebar names / content ----

#[test]
fn names_list_debounces_filters_and_opens() {
    let mut h = H::new(&sample_files());
    h.opened();
    h.key(Key::E, cs());
    assert_eq!(h.probe().side_mode, "names");
    // One char per frame (16 ms < 80 ms debounce): only the last query goes out.
    for c in ["m", "a", "i", "n"] {
        h.type_text(c);
    }
    let p = h.until("list", |p| p.side_list == ["src/main.c"]);
    assert_eq!(p.side_count, "1 file");
    let f = finds(&h);
    assert_eq!(f, vec![("main".into(), String::new(), String::new(), side::NAMES_LIMIT)], "debounced to one request");
    // Glob query.
    h.key(Key::A, CMD);
    h.type_text("*.md");
    let p = h.until("glob", |p| p.side_list.len() == 2);
    assert!(p.side_list.contains(&"docs/guide.md".to_owned()) && p.side_list.contains(&"README.md".to_owned()), "{:?}", p.side_list);
    // Filters compose: include src/** and exclude *.py with an empty query → filtered list.
    h.key(Key::A, CMD);
    h.key(Key::Backspace, NONE);
    click_id(&mut h, side::filters_id());
    click_id(&mut h, side::glob_id(0));
    h.type_text("src/**");
    click_id(&mut h, side::glob_id(1));
    h.type_text("*.py");
    let p = h.until("filtered", |p| p.side_list.len() == 2 && p.side_list.iter().all(|x| x.starts_with("src/") && !x.ends_with(".py")));
    assert!(p.side_list_active);
    let last = finds(&h).pop().unwrap();
    assert_eq!((last.0.as_str(), last.1.as_str(), last.2.as_str()), ("", "src/**", "*.py"));
    // Up/Down/Enter from the field.
    click_id(&mut h, side::field_id());
    h.key(Key::ArrowDown, NONE);
    h.key(Key::Enter, NONE);
    let p = h.until("opened second row", |p| p.active_view == "text");
    let second = p.side_list[1].rsplit('/').next().unwrap().to_owned();
    assert_eq!(p.active.as_deref(), Some(second.as_str()));
}

#[test]
fn names_list_keeps_old_rows_until_new_ones_and_esc_twice_leaves() {
    let items: Vec<FileMatch> = (0..5000).map(|i| FileMatch { path: RelPath::new(&format!("d/f{i}.txt")).unwrap(), score: 0, positions: vec![2] }).collect();
    let script = Script { files_override: Some(Arc::new(items)), total_files: Some(48_211), ..Default::default() };
    let mut h = H::with(&sample_files(), script, Some(common::ROOT));
    h.opened();
    h.key(Key::E, cs());
    h.type_text("f");
    let p = h.until("capped", |p| p.side_list.len() == 2000);
    assert_eq!(p.side_count, "first 2,000 matches of 48,211 files");
    // Typing more: the old rows stay on screen while the new request is out.
    h.type_text("1");
    let p = h.probe();
    assert_eq!(p.side_list.len(), 2000);
    // Esc clears the field (tree again); second Esc leaves the field.
    h.key(Key::Escape, NONE);
    let p = h.until("tree", |p| !p.side_list_active);
    assert!(p.side_query.is_empty());
    h.key(Key::Escape, NONE);
    assert!(!h.shot.ctx().memory(|m| m.has_focus(side::field_id())));
}

#[test]
fn content_mode_enter_opens_new_tabs_and_tabs_rerun() {
    let mut h = H::new(&sample_files());
    h.opened();
    h.key(Key::F, cs());
    h.type_text("parse_header");
    h.key(Key::Enter, NONE);
    let p = h.until("tab 1", |p| p.search_hits == 3);
    let t1 = p.search_tab.unwrap();
    // Content mode keeps the tree in the sidebar.
    assert!(!p.side_list_active && !p.tree.is_empty());
    h.key(Key::F, cs());
    h.key(Key::A, CMD);
    h.type_text("helper");
    h.key(Key::Enter, NONE);
    let p = h.until("tab 2", |p| p.search_tabs == 2 && p.active.as_deref() == Some("Search: helper") && p.search_hits == 2);
    let t2 = p.search_tab.unwrap();
    assert_ne!(t1, t2);
    assert_eq!(p.tabs.iter().filter(|t| t.starts_with("Search: ")).count(), 2);
    // Re-run in this tab: edit its pattern + Enter → a new Search, same tab.
    let before = h.fake.requests().iter().filter(|r| matches!(r, Request::Search { .. })).count();
    click_id(&mut h, search_panel::query_id(t2));
    h.key(Key::A, CMD);
    h.type_text("return");
    h.key(Key::Enter, NONE);
    let p = h.until("re-run", |p| p.search_query == "return" && p.search_hits > 2);
    assert_eq!(p.search_tabs, 2);
    assert!(h.fake.requests().iter().filter(|r| matches!(r, Request::Search { .. })).count() > before);
    // The first tab kept its own results.
    h.key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT);
    h.frames(1);
    let p = h.probe();
    if p.search_tab == Some(t1) {
        assert_eq!(p.search_hits, 3);
    }
    // Closing a search tab while it runs cancels its request (no panic on late replies).
    h.key(Key::W, CMD);
    h.frames(3);
}

#[test]
fn search_hit_context_menu_copies_line_and_path() {
    let mut h = H::new(&sample_files());
    h.opened();
    h.key(Key::F, cs());
    h.type_text("helper");
    h.key(Key::Enter, NONE);
    let p = h.until("hits", |p| p.search_hits == 2);
    let tab = p.search_tab.unwrap();
    h.frames(1);
    let r = h.rect_of(search_panel::results_id(tab).with(1usize)).unwrap();
    right_click(&mut h, r.center());
    let p = h.probe();
    assert_eq!(p.ctx_menu, vec!["Copy Line", "Copy Path", "Copy Relative Path"]);
    ctx_pick(&mut h, "Copy Line");
    assert!(h.shot.copied.iter().any(|c| c.contains("helper(")), "{:?}", h.shot.copied);
    right_click(&mut h, r.center());
    ctx_pick(&mut h, "Copy Relative Path");
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("src/main.c"));
    right_click(&mut h, r.center());
    ctx_pick(&mut h, "Copy Path");
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("/fake/proj/src/main.c"));
}

// ---- B. replace ----

fn replace_files() -> Vec<(&'static str, &'static str)> {
    vec![("a.txt", "foo bar\nfoo\n"), ("b.txt", "foo\n"), ("c.txt", "nothing\n")]
}

#[test]
fn replace_confirms_skips_dirty_reloads_clean_and_reruns() {
    let mut h = H::new(&replace_files());
    h.opened();
    open_file(&mut h, "a.txt");
    open_file(&mut h, "b.txt");
    h.type_text("x");
    h.until("b dirty", |p| p.active_dirty);
    h.key(Key::H, cs());
    let p = h.probe();
    assert!(p.search_replace_shown && p.active.as_deref() == Some("Search"));
    let tab = p.search_tab.unwrap();
    h.type_text("foo");
    h.until("hits", |p| p.search_hits == 3 && p.search_files == 2);
    click_id(&mut h, search_panel::replace_id(tab));
    h.type_text("baz");
    click_id(&mut h, search_panel::replace_all_id(tab));
    let p = h.until("confirm", |p| p.modal.is_some() && p.modal_title.is_some());
    assert_eq!(p.modal_title.as_deref(), Some("Replace 3 matches in 2 files with “baz”?"));
    // Enter does not confirm.
    h.key(Key::Enter, NONE);
    assert!(h.probe().modal.is_none());
    assert!(!h.fake.requests().iter().any(|r| matches!(r, Request::Replace { .. })));
    click_id(&mut h, search_panel::replace_all_id(tab));
    click_button(&mut h, "Replace");
    let p = h.until("toast", |p| p.toasts.iter().any(|t| t.starts_with("Replaced")));
    assert!(p.toasts.iter().any(|t| t == "Replaced 2 in 1 file · 1 skipped"), "{:?}", p.toasts);
    let req = h.fake.requests().into_iter().find_map(|r| if let Request::Replace { query, replacement, only, skip } = r { Some((query, replacement, only, skip)) } else { None }).unwrap();
    assert_eq!(req.0.pattern, "foo");
    assert_eq!(req.1, "baz");
    assert!(req.2.is_empty());
    assert_eq!(req.3, vec![RelPath::new("b.txt").unwrap()], "dirty tab skipped");
    // Re-run: only b.txt (skipped) still matches; the reason is in the tab status.
    let p = h.until("re-run", |p| p.search_hits == 1);
    assert!(p.search_note.as_deref().is_some_and(|n| n.contains("b.txt")), "{:?}", p.search_note);
    // Clean open tab reloaded from disk.
    h.key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT);
    h.key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT);
    let p = h.until("a.txt reloaded", |p| p.active.as_deref() == Some("a.txt") && p.active_text.as_deref() == Some("baz bar\nbaz\n"));
    assert!(!p.active_dirty);
}

#[test]
fn replace_in_one_file_and_empty_replacement_deletes() {
    let mut h = H::new(&replace_files());
    h.opened();
    h.key(Key::H, cs());
    let tab = h.probe().search_tab.unwrap();
    h.type_text("foo");
    h.until("hits", |p| p.search_hits == 3);
    h.frames(1);
    // Group 0 = a.txt (sorted). Per-file button.
    click_id(&mut h, search_panel::replace_file_id(tab, 0));
    let p = h.until("confirm", |p| p.modal_title.is_some());
    assert_eq!(p.modal_title.as_deref(), Some("Delete 2 matches in 1 file?"));
    click_button(&mut h, "Replace");
    h.until("toast", |p| p.toasts.iter().any(|t| t == "Replaced 2 in 1 file"));
    let req = h.fake.requests().into_iter().find_map(|r| if let Request::Replace { only, replacement, .. } = r { Some((only, replacement)) } else { None }).unwrap();
    assert_eq!(req, (vec![RelPath::new("a.txt").unwrap()], String::new()));
    assert_eq!(h.fake.fs.lock().unwrap().files["a.txt"].data, b" bar\n\n");
    assert_eq!(h.fake.fs.lock().unwrap().files["b.txt"].data, b"foo\n");
}

// ---- C. untitled, Save As, hot exit ----

#[test]
fn untitled_without_folder_and_hot_exit_restore() {
    let mut h = H::with(&sample_files(), Script::default(), None);
    h.key(Key::N, CMD);
    let p = h.probe();
    assert_eq!(p.active.as_deref(), Some("Untitled-1"));
    assert_eq!(p.active_untitled, Some(1));
    assert!(!p.active_dirty, "empty untitled is clean");
    h.frames(1);
    h.type_text("hello");
    let p = h.until("text", |p| p.active_text.as_deref() == Some("hello"));
    assert_eq!(p.active_view, "text");
    // Debounced write (~1 s at 60 fps).
    h.frames(75);
    h.app.flush_config();
    let f = h.cfg.path().join("untitled/untitled-1.txt");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "hello");
    // cmd+S without a folder: the dialog says so.
    h.key(Key::S, CMD);
    let p = h.until("save as", |p| p.modal_title.is_some());
    assert_eq!(p.modal.as_deref(), Some("save-as"));
    assert!(p.modal_buttons.iter().any(|b| b.0 == "Open Folder…"));
    h.key(Key::Escape, NONE);
    // Quit does not prompt for untitled buffers.
    h.key(Key::Q, CMD);
    assert!(h.probe().modal.is_none());
    // Restart: restored.
    let mut h = h.restart(None, false);
    let p = h.until("restored", |p| p.tabs == ["Untitled-1"]);
    assert_eq!(p.active_text.as_deref(), Some("hello"));
    h.key(Key::N, CMD);
    assert_eq!(h.probe().active.as_deref(), Some("Untitled-2"));
    // A fresh window restores nothing and numbers after what is on disk.
    let mut h = h.restart(None, true);
    h.frames(2);
    assert!(h.probe().tabs.is_empty());
    h.key(Key::N, CMD);
    assert_eq!(h.probe().active.as_deref(), Some("Untitled-2"));
    assert!(f.exists(), "the fresh window does not touch other buffers");
}

#[test]
fn save_as_creates_dirs_writes_create_new_and_unlinks_hot_file() {
    let mut h = H::new(&sample_files());
    h.opened();
    h.key(Key::N, CMD);
    h.frames(1);
    h.type_text("# Idea");
    h.frames(75);
    h.app.flush_config();
    let hot = h.cfg.path().join("untitled/untitled-1.txt");
    assert!(hot.exists());
    h.key(Key::S, CMD);
    h.until("dialog", |p| p.modal.as_deref() == Some("save-as"));
    h.frames(1);
    // Invalid: live error, Enter does nothing.
    h.type_text("../x.md");
    h.key(Key::Enter, NONE);
    assert_eq!(h.probe().modal.as_deref(), Some("save-as"));
    assert!(!h.fake.requests().iter().any(|r| matches!(r, Request::WriteFile { .. })));
    h.key(Key::A, CMD);
    h.type_text("notes/new/idea.md");
    h.key(Key::Enter, NONE);
    let p = h.until("saved", |p| p.active.as_deref() == Some("idea.md") && p.modal.is_none());
    assert!(!p.active_dirty && p.active_untitled.is_none());
    let reqs = h.fake.requests();
    let dirs: Vec<String> = reqs.iter().filter_map(|r| if let Request::CreateDir { path } = r { Some(path.to_string()) } else { None }).collect();
    assert_eq!(dirs, vec!["notes", "notes/new"]);
    assert!(reqs.iter().any(|r| matches!(r, Request::WriteFile { path, create_new: true, data, .. } if path.as_str() == "notes/new/idea.md" && data == b"# Idea")));
    h.app.flush_config();
    assert!(!hot.exists(), "hot-exit file unlinked after save");
    // It is a normal Markdown tab now: preview available.
    h.key(Key::V, cs());
    assert!(h.probe().preview_on);
    // Exists: inline error, the dialog stays.
    h.key(Key::N, CMD);
    h.frames(1);
    h.type_text("x");
    h.key(Key::S, cs());
    h.until("dialog", |p| p.modal.as_deref() == Some("save-as"));
    h.frames(1);
    h.type_text("README.md");
    h.key(Key::Enter, NONE);
    let p = h.until("exists", |p| p.save_as_error.is_some());
    assert_eq!(p.save_as_error.as_deref(), Some("README.md already exists"));
    assert_eq!(p.modal.as_deref(), Some("save-as"));
}

#[test]
fn save_without_a_folder_writes_where_asked() {
    let mut h = H::with(&sample_files(), Script::default(), None);
    h.key(Key::N, CMD);
    h.frames(1);
    h.type_text("# Idea");
    h.frames(75);
    h.app.flush_config();
    let hot = h.cfg.path().join("untitled/untitled-1.txt");
    assert!(hot.exists());
    h.key(Key::S, CMD);
    let p = h.until("dialog", |p| p.modal.as_deref() == Some("save-as") && !p.modal_buttons.is_empty());
    let labels: Vec<&str> = p.modal_buttons.iter().map(|b| b.0.as_str()).collect();
    assert_eq!(labels, vec!["Save", "Open Folder…", "Cancel"]);
    assert_eq!(p.save_as_text.as_deref(), Some("~/"));
    h.frames(1);
    // Not absolute: live error, Enter sends nothing.
    for bad in ["notes/idea.md", "~/notes/"] {
        h.key(Key::A, CMD);
        h.type_text(bad);
        h.key(Key::Enter, NONE);
        h.frames(2);
        let p = h.probe();
        assert_eq!(p.modal.as_deref(), Some("save-as"), "{bad}");
        assert!(p.save_as_error.is_none() && h.fake.saved.lock().unwrap().is_empty(), "{bad}");
    }
    // Refused outside the window process: its message, the dialog stays.
    h.key(Key::A, CMD);
    h.type_text("~/.ssh/idea.md");
    h.key(Key::Enter, NONE);
    let p = h.until("refused", |p| p.save_as_error.is_some());
    assert!(p.save_as_error.unwrap().starts_with("refusing to write"));
    h.key(Key::A, CMD);
    h.type_text("~/notes/theirs.md");
    h.key(Key::Enter, NONE);
    let p = h.until("exists", |p| p.save_as_error.is_some());
    assert_eq!(p.save_as_error.as_deref(), Some("theirs.md already exists"));
    assert!(h.fake.saved.lock().unwrap().is_empty());

    h.key(Key::A, CMD);
    h.type_text("~/notes/idea.md");
    h.key(Key::Enter, NONE);
    let p = h.until("saved", |p| p.modal.is_none() && p.active.as_deref() == Some("idea.md"));
    assert!(!p.active_dirty && p.folder.is_none());
    assert_eq!(p.active_external.as_deref(), Some("/home/me/notes/idea.md"));
    assert_eq!(p.tabs, ["idea.md"]);
    assert_eq!(*h.fake.saved.lock().unwrap(), vec![("/home/me/notes/idea.md".to_owned(), b"# Idea".to_vec(), false)]);
    h.app.flush_config();
    assert!(!hot.exists(), "on disk now: no copy in the config dir");

    // Edit, cmd+S: the same file again, no dialog.
    h.frames(2);
    h.type_text("!");
    h.until("dirty", |p| p.active_dirty);
    h.key(Key::S, CMD);
    let p = h.until("saved again", |p| !p.active_dirty);
    assert!(p.modal.is_none());
    assert_eq!(h.fake.saved.lock().unwrap().last().unwrap(), &("/home/me/notes/idea.md".to_owned(), b"# Idea!".to_vec(), true));

    // Save As: starts from its path; another name makes another file.
    h.key(Key::S, cs());
    let p = h.until("dialog", |p| p.modal.as_deref() == Some("save-as") && !p.modal_buttons.is_empty());
    assert_eq!(p.save_as_text.as_deref(), Some("/home/me/notes/idea.md"));
    h.frames(2);
    h.key(Key::A, CMD);
    h.type_text("/home/me/notes/idea2.md");
    h.key(Key::Enter, NONE);
    let p = h.until("saved as", |p| p.modal.is_none() && p.active.as_deref() == Some("idea2.md"));
    assert_eq!(p.active_external.as_deref(), Some("/home/me/notes/idea2.md"));

    // Closing it with edits: asks like a file, Save writes it and closes.
    h.frames(2);
    h.type_text("?");
    h.until("dirty", |p| p.active_dirty);
    h.key(Key::W, CMD);
    let p = h.until("ask", |p| !p.modal_buttons.is_empty());
    let labels: Vec<&str> = p.modal_buttons.iter().map(|b| b.0.as_str()).collect();
    assert_eq!(labels, vec!["Save", "Don't Save", "Cancel"]);
    assert_eq!(p.modal_title.as_deref(), Some("Save changes to idea2.md?"));
    click_button(&mut h, "Save");
    h.until("closed", |p| p.tabs.is_empty() && p.modal.is_none());
    assert_eq!(h.fake.saved.lock().unwrap().last().unwrap(), &("/home/me/notes/idea2.md".to_owned(), b"# Idea!?".to_vec(), true));
}

#[test]
fn closing_untitled_with_text_asks_and_discard_unlinks() {
    let mut h = H::new(&sample_files());
    h.opened();
    h.key(Key::N, CMD);
    h.frames(1);
    h.type_text("scratch");
    h.frames(75);
    h.app.flush_config();
    let hot = h.cfg.path().join("untitled/untitled-1.txt");
    assert!(hot.exists());
    h.key(Key::W, CMD);
    let p = h.until("ask", |p| !p.modal_buttons.is_empty());
    let labels: Vec<&str> = p.modal_buttons.iter().map(|b| b.0.as_str()).collect();
    assert_eq!(labels, vec!["Save…", "Discard", "Cancel"]);
    // Save… → Save As with the close queued.
    click_button(&mut h, "Save…");
    assert_eq!(h.probe().modal.as_deref(), Some("save-as"));
    h.frames(1);
    h.type_text("scratch.txt");
    h.key(Key::Enter, NONE);
    h.until("saved and closed", |p| p.tabs.is_empty());
    // Discard path.
    h.key(Key::N, CMD);
    h.frames(1);
    h.type_text("gone");
    h.frames(75);
    h.app.flush_config();
    let hot2 = h.cfg.path().join("untitled/untitled-2.txt");
    assert!(hot2.exists());
    h.key(Key::W, CMD);
    click_button(&mut h, "Discard");
    h.until("closed", |p| p.tabs.is_empty());
    h.app.flush_config();
    assert!(!hot2.exists());
}

// ---- D. menu bar + context menus ----

#[test]
fn menu_bar_click_keys_run_and_disabled() {
    let mut h = H::new(&sample_files());
    h.opened();
    click_id(&mut h, menubar::title_id(0));
    assert_eq!(h.probe().menu_open, Some(0));
    // Hover another title switches.
    let r = h.rect_of(menubar::title_id(2)).unwrap();
    h.shot.move_to([r.center().x, r.center().y]);
    h.frames(2);
    assert_eq!(h.probe().menu_open, Some(2));
    // Left / Right walk the menus; Esc closes.
    h.key(Key::ArrowLeft, NONE);
    assert_eq!(h.probe().menu_open, Some(1));
    h.key(Key::Escape, NONE);
    assert_eq!(h.probe().menu_open, None);
    // Edit menu with no text tab: Undo disabled, a click does nothing.
    click_id(&mut h, menubar::title_id(1));
    h.frames(1);
    click_id(&mut h, menubar::popup_id(1).with(0usize));
    assert!(h.probe().edit_log.is_empty());
    h.key(Key::Escape, NONE);
    // File ▸ New File by keyboard.
    click_id(&mut h, menubar::title_id(0));
    h.key(Key::ArrowDown, NONE);
    assert_eq!(h.probe().menu_sel, Some(0));
    h.key(Key::Enter, NONE);
    let p = h.probe();
    assert_eq!(p.menu_open, None);
    assert_eq!(p.active.as_deref(), Some("Untitled-1"));
    // Edit ▸ Select All goes to the editor as an EditAction.
    h.type_text("abc");
    click_id(&mut h, menubar::title_id(1));
    let i = margin_gui::commands::MENUS[1].1.iter().position(|x| *x == margin_gui::commands::MenuItem::Cmd(margin_gui::commands::Cmd::SelectAll)).unwrap();
    click_id(&mut h, menubar::popup_id(1).with(i));
    assert_eq!(h.probe().edit_log, vec![EditAction::SelectAll]);
    // Help ▸ About.
    click_id(&mut h, menubar::title_id(4));
    let n = margin_gui::commands::MENUS[4].1.len();
    click_id(&mut h, menubar::popup_id(4).with(n - 1));
    let p = h.until("about", |p| p.modal.as_deref() == Some("about"));
    assert_eq!(p.modal_title.as_deref(), Some("Margin"));
    h.key(Key::Escape, NONE);
    // Outside click closes.
    click_id(&mut h, menubar::title_id(0));
    h.click(Pos2::new(900.0, 500.0));
    assert_eq!(h.probe().menu_open, None);
    // Toggle Menu Bar from the palette; persisted.
    h.key(Key::P, cs());
    h.type_text("toggle menu bar");
    h.frames(2);
    h.key(Key::Enter, NONE);
    assert!(!h.probe().menu_bar);
    h.frames(40);
    h.app.flush_config();
    let conf = std::fs::read_to_string(h.cfg.path().join("settings.conf")).unwrap();
    assert!(conf.contains("menu_bar = false"), "{conf}");
}

#[test]
fn tree_and_tab_context_menus() {
    let mut h = H::new(&sample_files());
    h.opened();
    let i = h.probe().tree.iter().position(|(x, _)| x == "src").unwrap();
    let r = h.rect_of(margin_gui::tree::tree_id().with(("row", i))).unwrap();
    right_click(&mut h, r.center());
    assert_eq!(h.probe().ctx_menu, vec!["New File in Folder…", "New Folder…", "Rename", "Delete", "Copy Path", "Copy Relative Path", "Open in New Window"]);
    ctx_pick(&mut h, "Open in New Window");
    assert_eq!(*h.fake.windows.lock().unwrap(), vec![Some(std::path::PathBuf::from("/fake/proj/src"))]);
    right_click(&mut h, r.center());
    ctx_pick(&mut h, "Copy Relative Path");
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("src"));
    // File row: no new window.
    let i = h.probe().tree.iter().position(|(x, _)| x == "README.md").unwrap();
    let r = h.rect_of(margin_gui::tree::tree_id().with(("row", i))).unwrap();
    right_click(&mut h, r.center());
    assert!(!h.probe().ctx_menu.contains(&"Open in New Window".to_owned()));
    ctx_pick(&mut h, "Delete");
    assert_eq!(h.probe().modal.as_deref(), Some("delete README.md"));
    h.key(Key::Escape, NONE);
    // Esc closes a context menu.
    right_click(&mut h, r.center());
    h.key(Key::Escape, NONE);
    assert!(h.probe().ctx_menu.is_empty());
    // Tabs: Close Others / Close All.
    open_file(&mut h, "README.md");
    open_file(&mut h, "src/util.rs");
    h.frames(1);
    let tabs = h.probe().tabs.len();
    assert_eq!(tabs, 2);
    // The active tab id is the last one added; find its strip rect by trying ids.
    let rect = (1..20u64).filter_map(|id| h.rect_of(margin_gui::tabs::tab_id(1, id)).map(|r| (id, r))).last().unwrap().1;
    right_click(&mut h, rect.center());
    assert_eq!(h.probe().ctx_menu, vec!["Close", "Close Others", "Close All", "Split Right", "Split Down", "Move to Next Group", "Move to Previous Group", "Copy Path", "Copy Relative Path"]);
    ctx_pick(&mut h, "Close Others");
    assert_eq!(h.probe().tabs, vec!["util.rs"]);
    h.frames(1);
    let rect = (1..20u64).filter_map(|id| h.rect_of(margin_gui::tabs::tab_id(1, id)).map(|r| (id, r))).last().unwrap().1;
    right_click(&mut h, rect.center());
    ctx_pick(&mut h, "Close All");
    assert!(h.probe().tabs.is_empty());
}

// ---- E. call paths ----

fn site(path: &str, line: u32, text: &str, col: u32, len: u32) -> Site {
    Site { path: RelPath::new(path).unwrap(), line, col, len, text: text.into(), text_col: col, context: "parse_header".into() }
}

fn paths_script(n: usize) -> Script {
    let p = |i: usize| CallPath {
        sink: "memcpy".into(),
        kind: "memory".into(),
        hops: vec![
            Hop { site: site("src/main.c", 4, "    return helper(p);", 11, 6), args: vec![FlowArg { text: "p".into(), from: vec!["p".into()] }] },
            Hop { site: site("src/main.c", 3, "    memcpy(buf, p, 10);", 4, 6), args: vec![FlowArg { text: "buf".into(), from: vec![] }, FlowArg { text: "p".into(), from: vec!["p".into(), format!("x{i}")] }] },
        ],
        flows: i.is_multiple_of(2),
    };
    Script { paths_override: Some(Arc::new((0..n).map(p).collect())), ..Default::default() }
}

#[test]
fn paths_view_renders_navigates_and_opens_hops() {
    let mut h = H::with(&sample_files(), paths_script(3), Some(common::ROOT));
    h.opened();
    open_file(&mut h, "src/main.c");
    let line = sinks_hint(&mut h);
    h.frames(2);
    let id = h.probe().editor_id.unwrap();
    click_id(&mut h, id.with(("lens", line, SEG_SINKS)));
    let p = h.until("paths", |p| p.paths == 3);
    assert_eq!(p.paths_title.as_deref(), Some("Paths from parse_header to sinks · 3"));
    assert_eq!(p.paths_selected.as_deref(), Some("src/main.c:5"));
    h.key(Key::ArrowDown, NONE);
    assert_eq!(h.probe().paths_selected.as_deref(), Some("src/main.c:4"));
    h.key(Key::ArrowDown, NONE);
    assert_eq!(h.probe().paths_selected.as_deref(), Some("src/main.c:5"), "next group");
    h.key(Key::Enter, NONE);
    h.until("cursor at hop", |p| p.cursor.is_some_and(|c| c.0 == 4));
    // Click a hop row (row 1 = first hop of the first path).
    click_id(&mut h, margin_gui::paths_view::panel_id().with(("row", 1usize)));
    h.until("cursor", |p| p.cursor.is_some_and(|c| c.0 == 4));
    // Header click collapses.
    click_id(&mut h, margin_gui::paths_view::panel_id().with(("row", 0usize)));
    h.frames(1);
    // Callers segment still opens the plain results list.
    click_id(&mut h, id.with(("lens", line, SEG_CALLERS)));
    let p = h.until("callers", |p| p.results_title.is_some());
    assert!(p.paths_title.is_none());
}

// ---- F. new window ----

#[test]
fn new_window_calls_the_opener() {
    let mut h = H::new(&sample_files());
    h.opened();
    h.key(Key::N, cs());
    assert_eq!(*h.fake.windows.lock().unwrap(), vec![None]);
}

// ---- G. Markdown preview ----

#[test]
fn md_preview_toggle_links_and_tasks() {
    let files = vec![("docs/a.md", "# Top\n\n- [ ] task one\n- [x] done\n\nsee [b](b.md#part) and [web](https://example.com)\n\n## Second\n"), ("docs/b.md", "# B\n\ntext\n\n## Part\n")];
    let mut h = H::new(&files);
    h.opened();
    open_file(&mut h, "docs/a.md");
    assert!(!h.probe().preview_on, "off by default");
    click_id(&mut h, margin_gui::preview_toggle_id(1));
    let p = h.until("parsed", |p| p.preview_on && p.preview_parsed);
    assert!(p.preview_on);
    // Task toggle: one edit, undoable.
    h.app.inject_preview_output(PreviewOutput { task_toggled: Some(2), ..Default::default() });
    let p = h.until("flipped", |p| p.active_text.as_deref().is_some_and(|t| t.contains("- [x] task one")));
    assert!(p.active_dirty);
    h.app.inject_preview_output(PreviewOutput { task_toggled: Some(3), ..Default::default() });
    h.until("unflipped", |p| p.active_text.as_deref().is_some_and(|t| t.contains("- [ ] done")));
    // Out of range / not a task line: nothing.
    h.app.inject_preview_output(PreviewOutput { task_toggled: Some(9999), ..Default::default() });
    h.app.inject_preview_output(PreviewOutput { task_toggled: Some(0), ..Default::default() });
    h.frames(3);
    // Anchor link: cursor to the heading.
    h.app.inject_preview_output(PreviewOutput { link_clicked: Some("#second".into()), ..Default::default() });
    h.until("anchor", |p| p.cursor == Some((7, 0)));
    // External: copied, never opened.
    h.app.inject_preview_output(PreviewOutput { link_clicked: Some("https://example.com".into()), ..Default::default() });
    let p = h.until("copied", |p| p.toasts.iter().any(|t| t == "Link copied. Margin does not open URLs."));
    assert!(h.shot.copied.iter().any(|c| c == "https://example.com"));
    assert!(h.fake.windows.lock().unwrap().is_empty());
    let _ = p;
    // Leaving the folder: refused.
    h.app.inject_preview_output(PreviewOutput { link_clicked: Some("../../etc/passwd".into()), ..Default::default() });
    h.until("bad", |p| p.toasts.iter().any(|t| t == "Link is not a file in this folder"));
    // Relative file + fragment: opens b.md at `## Part`.
    h.app.inject_preview_output(PreviewOutput { link_clicked: Some("b.md#part".into()), ..Default::default() });
    h.until("b.md at Part", |p| p.active.as_deref() == Some("b.md") && p.cursor == Some((4, 0)));
    // cmd+shift+V toggles.
    h.key(Key::V, cs());
    assert!(h.probe().preview_on);
    h.key(Key::V, cs());
    assert!(!h.probe().preview_on);
}

#[test]
fn md_preview_select_copy_and_menu() {
    let files = vec![("a.md", "# Top\n\nFirst paragraph.\n\nSecond with [web](https://example.com/x).\n")];
    let mut h = H::new(&files);
    h.opened();
    open_file(&mut h, "a.md");
    click_id(&mut h, margin_gui::preview_toggle_id(1));
    let p = h.until("drawn", |p| p.preview_parsed && p.preview_text.len() == 3);
    let (first, second) = (p.preview_text[1], p.preview_text[2]);
    let secondary = |h: &mut H, at: egui::Pos2| {
        h.shot.move_to([at.x, at.y]);
        h.frames(1);
        for pressed in [true, false] {
            h.shot.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: NONE });
            h.frames(1);
        }
    };
    // Nothing selected: Copy is greyed.
    secondary(&mut h, first.center());
    assert_eq!(h.probe().ctx_menu, vec!["(Copy)", "Select All"]);
    h.key(Key::Escape, NONE);

    // Drag over the first paragraph, cmd+C.
    h.shot.press_at([first.left() + 0.5, first.center().y], NONE);
    h.frames(1);
    h.shot.move_to([first.right() + 20.0, first.center().y]);
    h.frames(2);
    h.shot.release();
    h.frames(2);
    assert_eq!(h.probe().preview_selection, "First paragraph.");
    h.shot.event(egui::Event::Copy);
    h.frames(2);
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("First paragraph."));
    assert!(h.probe().edit_log.is_empty(), "the editor was not asked");

    // Edit ▸ Select All, then Edit ▸ Copy: the preview, because it was used last.
    let item = |c| margin_gui::commands::MENUS[1].1.iter().position(|x| *x == margin_gui::commands::MenuItem::Cmd(c)).unwrap();
    click_id(&mut h, menubar::title_id(1));
    click_id(&mut h, menubar::popup_id(1).with(item(margin_gui::commands::Cmd::SelectAll)));
    assert_eq!(h.probe().preview_selection, "Top\n\nFirst paragraph.\n\nSecond with web.");
    click_id(&mut h, menubar::title_id(1));
    click_id(&mut h, menubar::popup_id(1).with(item(margin_gui::commands::Cmd::Copy)));
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("Top\n\nFirst paragraph.\n\nSecond with web."));
    assert!(h.probe().edit_log.is_empty());
    assert_eq!(h.probe().active_text.as_deref(), Some(files[0].1), "source untouched");

    // Context menu on the link: Copy, Copy Link, Select All.
    // (egui keeps Esc for a focused widget from its second focused frame on.)
    h.frames(2);
    h.key(Key::Escape, NONE);
    assert_eq!(h.probe().preview_selection, "", "Esc clears the selection");
    h.shot.press_at([second.left() + 0.5, second.center().y], NONE);
    h.frames(1);
    h.shot.move_to([second.left() + 40.0, second.center().y]);
    h.frames(2);
    h.shot.release();
    h.frames(2);
    let part = h.probe().preview_selection;
    assert!(!part.is_empty() && "Second with web.".starts_with(&part), "{part:?}");
    secondary(&mut h, egui::pos2(second.right() - 12.0, second.center().y));
    assert_eq!(h.probe().ctx_menu, vec!["Copy", "Copy Link", "Select All"]);
    ctx_pick(&mut h, "Copy Link");
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("https://example.com/x"));
    secondary(&mut h, first.center());
    ctx_pick(&mut h, "Copy");
    assert_eq!(h.shot.copied.last(), Some(&part));

    // Back in the editor: Select All is the editor's again.
    let ed = h.probe().editor_id.and_then(|id| h.rect_of(id)).expect("editor");
    h.click(egui::pos2(ed.left() + 120.0, ed.top() + 40.0));
    click_id(&mut h, menubar::title_id(1));
    click_id(&mut h, menubar::popup_id(1).with(item(margin_gui::commands::Cmd::SelectAll)));
    assert_eq!(h.probe().edit_log, vec![EditAction::SelectAll]);
}

#[test]
fn md_preview_setting_opens_it_and_untitled_previews() {
    let files = vec![("n.md", "# N\n")];
    let mut h = H::build(&files, Script::default(), Some(common::ROOT), [1280.0, 820.0], 1.0, &[("settings.conf", "md_preview = true\n")]);
    h.opened();
    open_file(&mut h, "n.md");
    assert!(h.probe().preview_on);
    // Untitled is previewable; narrow window → preview alone (no panic).
    h.key(Key::N, CMD);
    h.key(Key::V, cs());
    assert!(h.probe().preview_on);
    h.shot.resize([600.0, 500.0]);
    h.frames(3);
}

// ---- H. settings ----

#[test]
fn settings_persist_auto_indent_menu_bar_preview_and_sidebar() {
    // Tall window: every settings row on screen.
    let mut h = H::sized(&sample_files(), Script::default(), Some(common::ROOT), [1280.0, 1800.0], 1.0);
    h.opened();
    h.key(Key::Comma, CMD);
    h.frames(2);
    for k in ["autoindent", "menubar", "mdpreview"] {
        // Hiding the menu bar moves the rows: let the layout settle first.
        h.frames(2);
        click_id(&mut h, egui::Id::unique("settings").with(k));
    }
    h.key(Key::B, CMD);
    assert!(!h.probe().sidebar_visible);
    h.frames(40);
    h.app.flush_config();
    let conf = std::fs::read_to_string(h.cfg.path().join("settings.conf")).unwrap();
    assert!(conf.contains("auto_indent = false") && conf.contains("menu_bar = false") && conf.contains("md_preview = true"), "{conf}");
    let h = h.restart(Some(common::ROOT), false);
    let p = h.probe();
    assert!(!p.sidebar_visible && !p.menu_bar, "persisted across runs");
}

// ---- hostile input ----

#[test]
fn hostile_paths_names_and_args_do_not_panic() {
    let bidi = "evil\u{202e}gpj.md";
    let big = "x".repeat(100_000);
    let mut s = paths_script(1);
    let mut cp = (*s.paths_override.take().unwrap())[0].clone();
    cp.sink = format!("{big}\u{202e}");
    cp.hops[0].site.text_col = 9_999_999;
    cp.hops[0].site.len = u32::MAX;
    cp.hops[0].site.text = "é".repeat(5000);
    cp.hops[0].args = (0..10_000).map(|i| FlowArg { text: big.clone(), from: vec![format!("{i}\u{202e}"); 50] }).collect();
    let extra = cp.hops[1].clone();
    cp.hops.extend(std::iter::repeat_n(extra, 5000));
    s.paths_override = Some(Arc::new(vec![cp; 3]));
    let items = vec![FileMatch { path: RelPath::new(bidi).unwrap(), score: 0, positions: vec![u32::MAX, 3, 9_999] }];
    s.files_override = Some(Arc::new(items));
    let mut h = H::with(&[("src/main.c", "int parse_header(char *p) {\n    memcpy(a, p, 1);\n}\n"), (bidi, "# x\n")], s, Some(common::ROOT));
    h.opened();
    h.key(Key::E, cs());
    h.type_text("e");
    let p = h.until("list", |p| p.side_list.len() == 1);
    assert_eq!(p.side_list[0], bidi);
    h.frames(2);
    h.key(Key::Escape, NONE);
    open_file_by_tree(&mut h, "src/main.c");
    h.until("lenses", |p| p.lenses.iter().any(|l| l.1.iter().any(|s| s.starts_with("sinks"))));
    h.app.probe();
    // Ask for paths directly through the lens.
    let p = h.probe();
    let line = p.lenses.iter().find(|l| l.1.iter().any(|s| s.starts_with("sinks"))).unwrap().0;
    h.frames(2);
    let id = h.probe().editor_id.unwrap();
    click_id(&mut h, id.with(("lens", line, SEG_SINKS)));
    let p = h.until("paths", |p| p.paths == 3);
    let t = p.paths_title.unwrap();
    assert!(!t.contains('\u{202e}'));
    for _ in 0..30 {
        h.key(Key::ArrowDown, NONE);
    }
    h.frames(3);
}

fn open_file_by_tree(h: &mut H, path: &str) {
    let parts: Vec<&str> = path.split('/').collect();
    for k in 1..parts.len() {
        let dir = parts[..k].join("/");
        let d = dir.clone();
        h.until("dir row", move |p| p.tree.iter().any(|(x, _)| *x == d));
        let i = h.probe().tree.iter().position(|(x, _)| *x == dir).unwrap();
        click_id(h, margin_gui::tree::tree_id().with(("row", i)));
    }
    let p2 = path.to_owned();
    h.until("file row", move |p| p.tree.iter().any(|(x, _)| *x == p2));
    let i = h.probe().tree.iter().position(|(x, _)| x == path).unwrap();
    click_id(h, margin_gui::tree::tree_id().with(("row", i)));
    let name = parts.last().unwrap().to_string();
    h.until("tab loaded", move |p| p.active.as_deref() == Some(name.as_str()) && p.active_view == "text");
}

// ---- frame times ----

fn timed(h: &mut H, n: usize) -> std::time::Duration {
    h.shot.worst_frame = std::time::Duration::ZERO;
    h.frames(n);
    h.shot.worst_frame
}

#[test]
fn frame_time_names_list_50k_matches() {
    let items: Vec<FileMatch> = (0..50_000).map(|i| FileMatch { path: RelPath::new(&format!("src/mod_{}/file_{i}.rs", i % 300)).unwrap(), score: 0, positions: vec![0, 5, 13] }).collect();
    let script = Script { files_override: Some(Arc::new(items)), ..Default::default() };
    let mut h = H::with(&[("a", "b")], script, Some(common::ROOT));
    h.opened();
    h.key(Key::E, cs());
    h.type_text("file");
    h.until("list", |p| p.side_list.len() == 2000);
    let steady = timed(&mut h, 20);
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..30 {
        h.shot.key(Key::ArrowDown, NONE);
        worst = worst.max(timed(&mut h, 1));
    }
    h.shot.move_to([120.0, 400.0]);
    for _ in 0..10 {
        h.shot.scroll([0.0, -3000.0]);
        worst = worst.max(timed(&mut h, 1));
    }
    eprintln!("frame time, names list (50,000 matches sent, 2,000 shown): steady {steady:?}, keys/scroll worst {worst:?}");
    assert!(steady.as_millis() < 100 && worst.as_millis() < 100);
}

#[test]
fn frame_time_search_tab_100k_hits() {
    let hits: Vec<Site> = (0..100_000u32)
        .map(|i| Site { path: RelPath::new(&format!("dir{}/file{}.rs", i % 50, i % 2000)).unwrap(), line: i, col: 4, len: 5, text: format!("    let needle_{i} = foo(bar);"), text_col: 8, context: String::new() })
        .collect();
    let script = Script { search_override: Some(Arc::new(hits)), ..Default::default() };
    let mut h = H::with(&[("a", "b")], script, Some(common::ROOT));
    h.opened();
    h.key(Key::H, cs());
    h.type_text("needle");
    h.until("100k", |p| p.search_hits == 100_000);
    let steady = timed(&mut h, 20);
    h.shot.move_to([700.0, 500.0]);
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..20 {
        h.shot.scroll([0.0, -4000.0]);
        worst = worst.max(timed(&mut h, 1));
    }
    eprintln!("frame time, search tab with 100,000 hits (replace row on): steady {steady:?}, scrolling worst {worst:?}");
    assert!(steady.as_millis() < 100 && worst.as_millis() < 100);
}

#[test]
fn frame_time_paths_view_500_paths() {
    let mut h = H::with(&sample_files(), paths_script(500), Some(common::ROOT));
    h.opened();
    open_file(&mut h, "src/main.c");
    let line = sinks_hint(&mut h);
    h.frames(2);
    let id = h.probe().editor_id.unwrap();
    click_id(&mut h, id.with(("lens", line, SEG_SINKS)));
    h.until("500 paths", |p| p.paths == 500);
    let steady = timed(&mut h, 20);
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..40 {
        h.shot.key(Key::ArrowDown, NONE);
        worst = worst.max(timed(&mut h, 1));
    }
    eprintln!("frame time, paths view with 500 paths: steady {steady:?}, keys worst {worst:?}");
    assert!(steady.as_millis() < 100 && worst.as_millis() < 100);
}
