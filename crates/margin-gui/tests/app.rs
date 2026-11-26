//! Headless GUI tests: `App::ui` driven through `margin_shot::Shot` against the fake engine.

mod common;

use std::sync::Arc;

use common::{sample_files, Script, H};
use egui::{Event, Id, Key, Modifiers};
use margin_gui::hints::{SEG_CALLERS, SEG_SINKS};
use margin_proto::{DirEntry, EntryKind, FileMatch, RelPath, Request, Site};

const CMD: Modifiers = Modifiers::COMMAND;

fn cmd_shift() -> Modifiers {
    Modifiers::COMMAND | Modifiers::SHIFT
}

/// Click tree row showing `path`.
fn click_tree(h: &mut H, path: &str) {
    let p = h.probe();
    let i = p.tree.iter().position(|(x, _)| x == path).unwrap_or_else(|| panic!("{path} not in tree: {:?}", p.tree));
    let r = h.rect_of(margin_gui::tree::tree_id().with(("row", i))).expect("row rect");
    h.click(r.center());
}

fn open(h: &mut H, path: &str) {
    let parts: Vec<&str> = path.split('/').collect();
    for k in 1..parts.len() {
        let dir = parts[..k].join("/");
        if !h.probe().tree.iter().any(|(x, _)| x == path) {
            let d = dir.clone();
            h.until("dir row", move |p| p.tree.iter().any(|(x, _)| *x == d));
            click_tree(h, &dir);
        }
    }
    let p2 = path.to_owned();
    h.until("file row", move |p| p.tree.iter().any(|(x, _)| *x == p2));
    click_tree(h, path);
    let name = parts.last().unwrap().to_string();
    h.until("tab loaded", move |p| p.active.as_deref() == Some(name.as_str()) && p.active_view != "loading");
}

#[test]
fn folder_opens_and_tree_lists_entries() {
    let mut h = H::new(&sample_files());
    let p = h.opened();
    let top: Vec<&str> = p.tree.iter().map(|(x, _)| x.as_str()).collect();
    assert_eq!(top, vec!["docs", "src", "build.log", "README.md"], "dirs first, case-insensitive");
    assert_eq!(p.folder.as_deref(), Some("proj"));
    click_tree(&mut h, "src");
    let p = h.until("src expanded", |p| p.tree.iter().any(|(x, d)| x == "src/main.c" && *d == 1));
    assert!(p.tree.iter().any(|(x, _)| x == "src/lib"));
    // Collapse again.
    click_tree(&mut h, "src");
    h.until("collapsed", |p| p.tree.len() == 4);
}

#[test]
fn click_file_opens_tab_type_save() {
    let mut h = H::new(&sample_files());
    h.opened();
    open(&mut h, "src/util.rs");
    let p = h.probe();
    assert_eq!(p.active_text.as_deref(), Some("fn util() {\n    println!(\"x\");\n}\n"));
    assert!(!p.active_dirty);
    let mtime = h.fake.mtime("src/util.rs");
    h.frames(2);
    h.type_text("// hi");
    h.key(Key::Enter, Modifiers::NONE);
    let p = h.until("dirty", |p| p.active_dirty);
    let t = p.active_text.unwrap();
    assert!(t.starts_with("// hi\n"), "{t:?}");
    h.key(Key::S, CMD);
    h.until("saved", |p| !p.active_dirty);
    let w = h.fake.writes();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].0, "src/util.rs");
    assert!(String::from_utf8(w[0].1.clone()).unwrap().starts_with("// hi\nfn util()"));
    assert_eq!(w[0].2, Some(mtime), "expect_mtime_ns = the mtime we read");
    // Second save uses the mtime the first write returned.
    let after_first = h.fake.mtime("src/util.rs");
    assert_ne!(after_first, mtime);
    h.type_text("x");
    h.until("dirty again", |p| p.active_dirty);
    h.key(Key::S, CMD);
    h.until("saved again", |p| !p.active_dirty);
    let w = h.fake.writes();
    assert_eq!(w[1].2, Some(after_first));
}

#[test]
fn conflict_banner_and_overwrite() {
    let mut h = H::new(&sample_files());
    h.opened();
    open(&mut h, "README.md");
    h.frames(2);
    h.type_text("A");
    h.until("dirty", |p| p.active_dirty);
    // Someone else writes the file.
    h.fake.fs.lock().unwrap().add("README.md", b"other".to_vec());
    h.key(Key::S, CMD);
    let p = h.until("conflict", |p| p.banner == Some("conflict"));
    assert!(p.active_dirty);
}

#[test]
fn focus_regained_reloads_clean_tabs_and_flags_dirty_ones() {
    let mut h = H::new(&sample_files());
    h.opened();
    open(&mut h, "docs/guide.md");
    h.frames(130); // Reindex is throttled to once per 2 s after open.
    h.fake.fs.lock().unwrap().add("docs/guide.md", b"changed outside\n".to_vec());
    h.shot.event(Event::WindowFocused(false));
    h.frames(2);
    h.shot.event(Event::WindowFocused(true));
    h.until("reloaded", |p| p.active_text.as_deref() == Some("changed outside\n"));
    assert!(h.fake.requests().iter().any(|r| matches!(r, Request::Reindex { .. })));
    // Dirty → banner, no reload.
    h.type_text("mine ");
    h.until("dirty", |p| p.active_dirty);
    h.fake.fs.lock().unwrap().add("docs/guide.md", b"again\n".to_vec());
    h.shot.event(Event::WindowFocused(false));
    h.frames(2);
    h.shot.event(Event::WindowFocused(true));
    let p = h.until("banner", |p| p.banner == Some("changed"));
    assert!(p.active_text.unwrap().starts_with("mine "));
}

#[test]
fn palette_opens_filters_and_runs() {
    let mut h = H::new(&sample_files());
    h.opened();
    h.key(Key::P, CMD);
    assert!(h.probe().palette_open);
    h.key(Key::Escape, Modifiers::NONE);
    assert!(!h.probe().palette_open);
    if cfg!(target_os = "macos") {
        h.key(Key::P, Modifiers::CTRL);
        assert!(h.probe().palette_open, "literal ctrl+P on macOS");
        h.key(Key::Escape, Modifiers::NONE);
    }
    h.key(Key::P, cmd_shift());
    assert!(h.probe().palette_open);
    h.type_text("toggle sidebar");
    h.frames(3);
    h.key(Key::Enter, Modifiers::NONE);
    let p = h.probe();
    assert!(!p.palette_open && !p.sidebar_visible, "{p:?}");
    // Files section: debounced FindFiles, Enter on a file row.
    h.key(Key::P, CMD);
    h.type_text("guide");
    let p = h.until("files", |p| p.palette_files > 0);
    assert_eq!(p.palette_query, "guide");
    h.frames(1);
    // Move down past any matching actions to the file row.
    for _ in 0..40 {
        if h.probe().palette_selected.as_deref() == Some("file:docs/guide.md") {
            break;
        }
        h.key(Key::ArrowDown, Modifiers::NONE);
    }
    assert_eq!(h.probe().palette_selected.as_deref(), Some("file:docs/guide.md"));
    h.key(Key::Enter, Modifiers::NONE);
    h.until("guide open", |p| p.active.as_deref() == Some("guide.md"));
    // Go to File shows files only.
    h.key(Key::T, CMD);
    h.type_text("util");
    // Results for "" stay on screen until the ones for "util" arrive.
    h.until("util results", |p| p.palette_files == 1 && p.palette_selected.as_deref() == Some("file:src/util.rs"));
    h.key(Key::Enter, Modifiers::NONE);
    h.until("util open", |p| p.active.as_deref() == Some("util.rs") && p.active_view == "text");
}

#[test]
fn go_to_line_jumps_and_back_returns() {
    let mut h = H::new(&sample_files());
    h.opened();
    open(&mut h, "src/main.c");
    h.frames(2);
    h.key(Key::P, CMD);
    h.type_text(":12");
    h.frames(2);
    h.key(Key::Enter, Modifiers::NONE);
    let p = h.until("cursor", |p| p.cursor.map(|c| c.0) == Some(11));
    assert_eq!(p.back, 1);
    if cfg!(target_os = "macos") {
        h.key(Key::Minus, Modifiers::CTRL);
        h.until("back", |p| p.cursor.map(|c| c.0) == Some(0));
    }
    // Ctrl+G opens the palette with `:`.
    h.key(Key::G, Modifiers::CTRL);
    assert_eq!(h.probe().palette_query, ":");
}

#[test]
fn settings_change_font_and_write_conf() {
    let mut h = H::new(&sample_files());
    h.opened();
    h.key(Key::Comma, CMD);
    h.frames(2);
    assert_eq!(h.probe().active.as_deref(), Some("Settings"));
    let plus = h.rect_of(Id::unique("settings").with("code_size").with(1)).expect("stepper");
    h.click(plus.center());
    let p = h.probe();
    assert_eq!(p.look.code_size, 13.5);
    // Saved 500 ms after the last change (egui time: 1/60 s per frame).
    h.frames(40);
    h.app.flush_config();
    let text = std::fs::read_to_string(h.cfg.path().join("settings.conf")).expect("settings.conf written");
    assert!(text.contains("code_size = 13.5"), "{text}");
    // Zoom shortcut.
    h.key(Key::Equals, CMD);
    assert!((h.probe().zoom - 1.1).abs() < 1e-4);
    h.key(Key::Num0, CMD);
    assert_eq!(h.probe().zoom, 1.0);
}

#[test]
fn search_streams_sorted_hits_and_opens_one() {
    let mut h = H::new(&sample_files());
    h.opened();
    // cmd+shift+F: sidebar field in Content mode; Enter opens a search tab.
    h.key(Key::F, cmd_shift());
    assert_eq!(h.probe().side_mode, "content");
    h.type_text("parse_header");
    h.key(Key::Enter, Modifiers::NONE);
    let p = h.until("hits", |p| p.search_hits == 3);
    assert_eq!(p.search_files, 1);
    assert_eq!(p.active.as_deref(), Some("Search: parse_header"));
    let tab = p.search_tab.unwrap();
    // Row 0 = file header, row 1 = first hit (line 3).
    h.frames(1);
    let r = h.rect_of(margin_gui::search_panel::results_id(tab).with(1usize)).expect("hit row");
    h.click(r.center());
    let p = h.until("opened at hit", |p| p.active.as_deref() == Some("main.c") && p.cursor.is_some_and(|c| c.0 == 2));
    assert_eq!(p.active_view, "text");
    // Bad regex: error inline in the new tab.
    h.key(Key::F, cmd_shift());
    h.type_text("(");
    let t = h.rect_of(margin_gui::side::side_toggle_id(".*")).expect("regex toggle");
    h.click(t.center());
    let f = h.rect_of(margin_gui::side::field_id()).unwrap();
    h.click(f.center());
    h.key(Key::Enter, Modifiers::NONE);
    h.until("regex error", |p| p.search_error.is_some() && p.search_tabs == 2);
}

#[test]
fn hints_appear_and_callers_open_results() {
    let mut h = H::new(&sample_files());
    h.opened();
    open(&mut h, "src/main.c");
    let p = h.until("hints with engine numbers", |p| p.lenses.iter().any(|l| l.1.iter().any(|s| s.ends_with("callers"))));
    let (line, segs) = p.lenses.iter().find(|l| l.1.iter().any(|s| s.ends_with("callers"))).cloned().unwrap();
    assert_eq!(line, 2, "parse_header is on line 3");
    assert_eq!(segs[0], "2 callers");
    assert!(segs.iter().any(|s| s == "sinks: memcpy"), "{segs:?}");
    h.frames(2);
    let id = h.probe().editor_id.unwrap();
    let r = h.rect_of(id.with(("lens", line, SEG_CALLERS))).expect("lens segment");
    h.click(r.center());
    let p = h.until("callers panel", |p| p.results == 2);
    assert!(p.results_title.unwrap().starts_with("Callers of parse_header"));
    // Esc closes it (results panel has focus).
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.probe().results_title.is_none());
    // Sinks segment: call paths to sinks (bottom paths view).
    let r = h.rect_of(id.with(("lens", line, SEG_SINKS))).expect("sinks segment");
    h.click(r.center());
    let p = h.until("paths panel", |p| p.paths_title.as_deref().is_some_and(|t| !t.ends_with('…')));
    assert_eq!(p.paths_title.as_deref(), Some("Paths from parse_header to sinks · 0"));
    assert!(p.results_title.is_none());
    assert!(h.fake.requests().iter().any(|r| matches!(r, Request::Paths { name, sink, limit: 200 } if name == "parse_header" && sink.is_empty())));
}

#[test]
fn unsaved_close_asks() {
    let mut h = H::new(&sample_files());
    h.opened();
    open(&mut h, "README.md");
    h.frames(2);
    h.type_text("Z");
    h.until("dirty", |p| p.active_dirty);
    h.key(Key::W, CMD);
    assert_eq!(h.probe().modal.as_deref(), Some("unsaved"));
    // Typing while the modal is open does not reach the editor.
    h.type_text("Q");
    assert!(!h.probe().active_text.unwrap().contains('Q'));
    h.key(Key::Escape, Modifiers::NONE);
    let p = h.probe();
    assert!(p.modal.is_none() && p.tabs.len() == 1);
    h.key(Key::W, CMD);
    h.key(Key::Enter, Modifiers::NONE);
    h.until("saved and closed", |p| p.tabs.is_empty());
    assert!(String::from_utf8(h.fake.writes()[0].1.clone()).unwrap().starts_with('Z'));
}

#[test]
fn fatal_banner_after_hello() {
    let script = Script { fatal_after: Some((1, "worker crashed".into())), ..Default::default() };
    let mut h = H::with(&sample_files(), script, Some(common::ROOT));
    let p = h.until("fatal", |p| p.fatal.is_some());
    assert_eq!(p.fatal.as_deref(), Some("worker crashed"));
}

#[test]
fn fatal_at_open_shows_on_welcome() {
    let script = Script { fatal_at_start: Some("refusing to open the home directory".into()), ..Default::default() };
    let mut h = H::with(&sample_files(), script, Some(common::ROOT));
    let p = h.until("open error", |p| p.open_error.is_some());
    assert!(p.folder.is_none());
}

#[test]
fn hostile_names_are_sanitised() {
    let mut h = H::new(&[("evil\u{202e}gpj.exe", "x"), ("ok.txt", "y")]);
    h.opened();
    // A name with a newline cannot come over the wire; through the in-process link it must not panic either.
    let bad = vec![
        DirEntry { name: "line\nbreak".into(), kind: EntryKind::File, size: 1, ignored: false },
        DirEntry { name: "evil\u{202e}gpj.exe".into(), kind: EntryKind::File, size: 1, ignored: false },
    ];
    h.fake.script.lock().unwrap().dir_override.insert(String::new(), Arc::new(bad));
    h.shot.event(Event::WindowFocused(false));
    h.frames(1);
    h.shot.event(Event::WindowFocused(true));
    h.until("listing", |p| p.tree.len() == 1);
    click_tree(&mut h, "evil\u{202e}gpj.exe");
    let p = h.until("tab", |p| p.active_view == "text");
    let title = p.active.unwrap();
    assert_eq!(title, "evil·gpj.exe");
    assert!(!title.contains('\u{202e}'));
}

#[test]
fn picker_browses_and_opens() {
    let mut h = H::with(&sample_files(), Script::default(), None);
    h.key(Key::O, CMD);
    h.until("picker", |p| p.picker_open);
    h.frames(3);
    // Rows: `..`, Documents, Downloads, dev, notes; the first dir is selected.
    for _ in 0..2 {
        h.key(Key::ArrowDown, Modifiers::NONE);
    }
    h.key(Key::Enter, Modifiers::NONE);
    h.frames(3);
    h.key(Key::Enter, CMD);
    // The picker closes when the engine answers, a frame or more after the folder is set.
    let p = h.until("opened", |p| p.folder.is_some() && !p.picker_open);
    assert_eq!(p.folder.as_deref(), Some("dev"));
    assert_eq!(p.recent.first().map(String::as_str), Some("/home/me/dev"));
}

#[test]
fn tabs_are_restored_with_the_folder() {
    let mut h = H::new(&sample_files());
    h.opened();
    open(&mut h, "README.md");
    open(&mut h, "src/util.rs");
    h.frames(40);
    h.app.on_exit();
    let state = std::fs::read_to_string(h.cfg.path().join("state.conf")).unwrap();
    assert!(state.contains("tab = README.md") && state.contains("active = src/util.rs"), "{state}");
    let mut h2 = H::build(&sample_files(), Script::default(), Some(common::ROOT), [1280.0, 820.0], 1.0, &[("state.conf", &state)]);
    let p = h2.until("restored", |p| p.tabs.len() == 2 && p.active.as_deref() == Some("util.rs"));
    assert_eq!(p.tabs, vec!["README.md", "util.rs"]);
}

// ---- frame times (printed; generous bounds for debug builds) ----

fn timed(h: &mut H, n: usize) -> std::time::Duration {
    h.shot.worst_frame = std::time::Duration::ZERO;
    h.frames(n);
    h.shot.worst_frame
}

#[test]
fn frame_time_50k_tree_entries() {
    let entries: Vec<DirEntry> = (0..50_000).map(|i| DirEntry { name: format!("file_{i:05}.txt"), kind: EntryKind::File, size: 10, ignored: i % 7 == 0 }).collect();
    let mut script = Script::default();
    script.dir_override.insert(String::new(), Arc::new(entries));
    let mut h = H::with(&[("a", "b")], script, Some(common::ROOT));
    h.until("50k rows", |p| p.tree.len() == 50_000);
    let steady = timed(&mut h, 20);
    h.shot.move_to([150.0, 300.0]);
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..20 {
        h.shot.scroll([0.0, -4000.0]);
        worst = worst.max(timed(&mut h, 1));
    }
    eprintln!("frame time, 50,000-entry tree: steady {steady:?}, scrolling worst {worst:?}");
    assert!(steady.as_millis() < 100 && worst.as_millis() < 100);
}

#[test]
fn frame_time_100k_search_hits() {
    let hits: Vec<Site> = (0..100_000u32)
        .map(|i| Site { path: RelPath::new(&format!("dir{}/file{}.rs", i % 50, i % 2000)).unwrap(), line: i, col: 4, len: 5, text: format!("    let needle_{i} = foo(bar);"), text_col: 8, context: String::new() })
        .collect();
    let script = Script { search_override: Some(Arc::new(hits)), ..Default::default() };
    let mut h = H::with(&[("a", "b")], script, Some(common::ROOT));
    h.opened();
    h.key(Key::F, cmd_shift());
    h.type_text("needle");
    h.key(Key::Enter, Modifiers::NONE);
    let t0 = std::time::Instant::now();
    h.until("100k hits", |p| p.search_hits == 100_000);
    let arrive = t0.elapsed();
    let steady = timed(&mut h, 20);
    eprintln!("frame time, 100,000 search hits: steady {steady:?} (all batches in after {arrive:?})");
    assert!(steady.as_millis() < 100);
}

#[test]
fn frame_time_palette_1000_files() {
    let items: Vec<FileMatch> = (0..1000).map(|i| FileMatch { path: RelPath::new(&format!("src/module_{i}/file_{i}.rs")).unwrap(), score: 1000 - i, positions: vec![0, 4, 11] }).collect();
    let script = Script { files_override: Some(Arc::new(items)), ..Default::default() };
    let mut h = H::with(&[("a", "b")], script, Some(common::ROOT));
    h.opened();
    h.key(Key::P, CMD);
    h.type_text("file");
    h.until("1000 files", |p| p.palette_files == 1000);
    let steady = timed(&mut h, 20);
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..30 {
        h.shot.key(Key::PageDown, Modifiers::NONE);
        worst = worst.max(timed(&mut h, 1));
    }
    eprintln!("frame time, palette with 1,000 file matches: steady {steady:?}, paging worst {worst:?}");
    assert!(steady.as_millis() < 100 && worst.as_millis() < 100);
}

#[test]
fn idle_frames_request_no_repaint() {
    let mut h = H::new(&sample_files());
    h.opened();
    open(&mut h, "README.md");
    h.frames(60);
    // After settling, a frame asks for no immediate repaint.
    let ctx = h.shot.ctx().clone();
    h.frames(1);
    assert!(!ctx.has_requested_repaint(), "idle repaint requested");
}
