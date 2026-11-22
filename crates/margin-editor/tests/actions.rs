//! Edit actions, line shortcuts, clipboard paths and auto-indent through `show`.

mod common;

use std::time::Instant;

use common::{alt, cmd, cmd_shift, shift, H};
use egui::{Event, Key, Modifiers};
use margin_code::Lang;
use margin_editor::{show, Document, EditAction, EditorOptions, EditorState};
use margin_theme::Theme;

const NONE: Modifiers = Modifiers::NONE;

fn undo(h: &mut H) {
    h.key(Key::Z, cmd());
}

#[test]
fn queue_applies_in_order_and_takes_focus() {
    let mut h = H::new("a\nb\nc", Lang::Plain);
    h.st.goto(1, 0);
    h.frame();
    // Esc drops egui focus: the next queued action must take it back.
    h.key(Key::Escape, NONE);
    assert!(!h.out.focused);
    h.st.run(EditAction::MoveLineDown);
    h.st.run(EditAction::DuplicateLine);
    h.frame();
    assert_eq!(h.doc.text(), "a\nc\nb\nb");
    assert_eq!(h.st.cursor(), (3, 0));
    assert!(h.out.changed);
    h.frame();
    assert!(h.out.focused, "a queued action takes keyboard focus");
    // One action = one undo step.
    h.act(EditAction::Undo);
    assert_eq!(h.doc.text(), "a\nc\nb");
    h.act(EditAction::Undo);
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("a\nb\nc", (1, 0)));
    h.act(EditAction::Redo);
    assert_eq!(h.doc.text(), "a\nc\nb");
    h.act(EditAction::Undo);
    // Delete, move up, select all.
    h.st.goto(2, 1);
    h.act(EditAction::MoveLineUp);
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("a\nc\nb", (1, 1)));
    h.act(EditAction::DeleteLine);
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("a\nb", (1, 1)));
    h.act(EditAction::SelectAll);
    assert_eq!(h.st.selected_text(&h.doc), "a\nb");
}

#[test]
fn indent_outdent_comment_actions_on_multi_line_selections() {
    let mut h = H::new("fn a() {\nx();\ny();\n}", Lang::Rust);
    h.st.set_selection((1, 0), (2, 2));
    h.act(EditAction::Indent);
    assert_eq!(h.doc.text(), "fn a() {\n    x();\n    y();\n}");
    assert_eq!(h.st.selection(), Some(((1, 4), (2, 6))));
    h.act(EditAction::ToggleComment);
    assert_eq!(h.doc.text(), "fn a() {\n    // x();\n    // y();\n}");
    h.act(EditAction::ToggleComment);
    h.act(EditAction::Outdent);
    assert_eq!(h.doc.text(), "fn a() {\nx();\ny();\n}");
    // Undo: one step per action.
    for want in ["fn a() {\n    x();\n    y();\n}", "fn a() {\n    // x();\n    // y();\n}", "fn a() {\n    x();\n    y();\n}", "fn a() {\nx();\ny();\n}"] {
        h.act(EditAction::Undo);
        assert_eq!(h.doc.text(), want);
    }
    // Indent with a caret indents the line, not the caret position.
    h.st.goto(1, 2);
    h.act(EditAction::Indent);
    assert_eq!(h.doc.line(1), "    x();");
    // Multi-line duplicate / move / delete keep the block together.
    h.st.set_selection((1, 0), (3, 0));
    h.act(EditAction::DuplicateLine);
    assert_eq!(h.doc.text(), "fn a() {\n    x();\ny();\n    x();\ny();\n}");
    assert_eq!(h.st.selection(), Some(((3, 0), (5, 0))));
    h.act(EditAction::MoveLineUp);
    h.act(EditAction::DeleteLine);
    assert_eq!(h.doc.text(), "fn a() {\n    x();\ny();\n}");
}

#[test]
fn clipboard_actions_and_paste_request() {
    let mut h = H::new("one\ntwo", Lang::Plain);
    h.st.select(0, 0, 3);
    h.act(EditAction::Copy);
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("one"));
    h.st.goto(1, 1);
    h.act(EditAction::Copy);
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("two\n"), "no selection: the whole line");
    h.act(EditAction::Cut);
    assert_eq!((h.shot.copied.last().map(String::as_str), h.doc.text().as_str()), (Some("two\n"), "one"));
    h.st.select(0, 1, 2);
    h.act(EditAction::Cut);
    assert_eq!((h.shot.copied.last().map(String::as_str), h.doc.text().as_str()), (Some("ne"), "o"));
    // Paste action: focus now, the text arrives as Event::Paste in a later frame.
    h.key(Key::Escape, NONE);
    assert!(!h.out.focused);
    h.act(EditAction::Paste);
    assert!(h.out.focused);
    h.shot.paste("K");
    h.frame();
    assert_eq!(h.doc.text(), "oK");
}

/// `EditAction::Paste` emits `ViewportCommand::RequestPaste` (only when editable).
fn paste_requests(read_only: bool) -> usize {
    let ctx = egui::Context::default();
    let mut doc = Document::new("x", Lang::Plain);
    doc.set_read_only(read_only);
    let mut st = EditorState::default();
    st.run(EditAction::Paste);
    let (opts, theme) = (EditorOptions::default(), Theme::default());
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        show(ui, &mut doc, &mut st, &opts, &theme, &[]);
    });
    out.textures_delta.clear();
    out.viewport_output.values().flat_map(|v| v.commands.iter()).filter(|c| matches!(c, egui::ViewportCommand::RequestPaste)).count()
}

#[test]
fn paste_action_sends_request_paste() {
    assert_eq!(paste_requests(false), 1);
    assert_eq!(paste_requests(true), 0);
}

#[test]
fn read_only_ignores_mutating_actions() {
    let text = "fn a() {\n    x();\n}";
    let mut h = H::new(text, Lang::Rust);
    h.doc.set_read_only(true);
    h.st.set_selection((0, 0), (1, 3));
    for a in [
        EditAction::Cut,
        EditAction::Paste,
        EditAction::ToggleComment,
        EditAction::Indent,
        EditAction::Outdent,
        EditAction::DuplicateLine,
        EditAction::DeleteLine,
        EditAction::MoveLineUp,
        EditAction::MoveLineDown,
        EditAction::Undo,
        EditAction::Redo,
    ] {
        let n = h.shot.copied.len();
        h.act(a);
        assert_eq!(h.doc.text(), text, "{a:?}");
        assert!(!h.out.changed, "{a:?}");
        assert_eq!(h.shot.copied.len(), n, "{a:?} copied something");
    }
    for k in [(Key::ArrowUp, alt()), (Key::ArrowDown, Modifiers { shift: true, ..alt() }), (Key::K, cmd_shift()), (Key::CloseBracket, cmd()), (Key::Enter, NONE)] {
        h.key(k.0, k.1);
        assert_eq!(h.doc.text(), text, "{k:?}");
    }
    h.act(EditAction::Copy);
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("fn a() {\n   "));
    h.act(EditAction::SelectAll);
    assert_eq!(h.st.selected_text(&h.doc), text);
}

#[test]
fn line_shortcuts() {
    let mut h = H::new("a\nb\nc", Lang::Plain);
    h.st.goto(0, 1);
    h.frame();
    h.key(Key::ArrowDown, alt());
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("b\na\nc", (1, 1)));
    h.key(Key::ArrowUp, alt());
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("a\nb\nc", (0, 1)));
    h.key(Key::ArrowDown, Modifiers { shift: true, ..alt() });
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("a\na\nb\nc", (1, 1)), "shift+alt+down: copy below, cursor on the copy");
    h.key(Key::ArrowUp, Modifiers { shift: true, ..alt() });
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("a\na\na\nb\nc", (1, 1)), "shift+alt+up: copy, cursor stays");
    h.key(Key::D, cmd_shift());
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("a\na\na\na\nb\nc", (2, 1)));
    h.key(Key::K, cmd_shift());
    h.key(Key::K, cmd_shift());
    h.st.goto(1, 1);
    h.frame();
    h.key(Key::K, cmd_shift());
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("a\nb\nc", (1, 1)));
    h.key(Key::CloseBracket, cmd());
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("a\n    b\nc", (1, 5)));
    h.key(Key::OpenBracket, cmd());
    assert_eq!(h.doc.text(), "a\nb\nc");
    // Tab / shift+Tab with a multi-line selection.
    h.st.set_selection((0, 0), (2, 1));
    h.frame();
    h.key(Key::Tab, NONE);
    assert_eq!(h.doc.text(), "    a\n    b\n    c");
    h.key(Key::Tab, shift());
    assert_eq!(h.doc.text(), "a\nb\nc");
    // Each shortcut was one undo step.
    for want in ["    a\n    b\n    c", "a\nb\nc", "a\n    b\nc", "a\nb\nc", "a\na\nb\nc", "a\na\na\nb\nc", "a\na\na\na\nb\nc", "a\na\na\nb\nc"] {
        undo(&mut h);
        assert_eq!(h.doc.text(), want);
    }
}

#[test]
fn keyboard_clipboard_paths() {
    let mut h = H::new("alpha beta\r\ngamma\r\n", Lang::Plain);
    assert!(h.doc.crlf());
    h.st.select(0, 6, 4);
    h.frame();
    h.ev(Event::Copy);
    assert_eq!(h.shot.copied.last().map(String::as_str), Some("beta"));
    h.ev(Event::Cut);
    assert_eq!((h.shot.copied.last().map(String::as_str), h.doc.line(0)), (Some("beta"), "alpha "));
    // Paste replaces the selection; CRLF and lone CR become the document's line ending.
    h.st.select(1, 0, 5);
    h.frame();
    h.shot.paste("x\ry\r\nz");
    h.frame();
    assert_eq!(h.doc.text(), "alpha \r\nx\r\ny\r\nz\r\n");
    assert_eq!(h.st.cursor(), (3, 1));
    // One undo step restores the selection's text.
    undo(&mut h);
    assert_eq!(h.doc.text(), "alpha \r\ngamma\r\n");
    // Paste after typing is its own undo step.
    h.st.goto(0, 0);
    h.frame();
    h.typ("q");
    h.shot.paste("P");
    h.frame();
    assert_eq!(h.doc.line(0), "qPalpha ");
    undo(&mut h);
    assert_eq!(h.doc.line(0), "qalpha ");
}

#[test]
fn pasted_hazards_stay_visible() {
    let mut h = H::new("x\n", Lang::Plain);
    h.st.goto(0, 1);
    h.frame();
    let nasty = " \u{202E}evil\u{2066} \u{200B}\u{0}\u{1b}[0m";
    h.shot.paste(nasty);
    h.frame();
    assert_eq!(h.doc.line(0), format!("x{nasty}"), "pasted bytes kept as is");
    h.png("paste-hazards.png");
}

#[test]
fn paste_5mb_does_not_freeze() {
    let mut big = String::with_capacity(5 << 20);
    let mut i = 0;
    while big.len() < 5 << 20 {
        big.push_str(&format!("let value_{i} = compute(alpha, beta, gamma); // filler text\r\n"));
        i += 1;
    }
    let mut h = H::new("start\nend", Lang::Rust);
    h.st.goto(0, 5);
    h.frame();
    let t0 = Instant::now();
    h.shot.paste(&big);
    h.frame();
    let paste = h.shot.last_frame;
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..10 {
        h.typ("x");
        worst = worst.max(h.shot.last_frame);
    }
    let total = t0.elapsed();
    assert_eq!(h.doc.line_count() as usize, i + 2);
    undo(&mut h);
    let t1 = Instant::now();
    undo(&mut h);
    let undo_t = t1.elapsed();
    assert_eq!(h.doc.text(), "start\nend");
    eprintln!(
        "paste {} MB ({} lines, {}): paste frame {:.1} ms, typing after (max) {:.1} ms, total {:.1} ms, undo {:.1} ms",
        big.len() >> 20,
        i,
        if cfg!(debug_assertions) { "debug" } else { "release" },
        paste.as_secs_f64() * 1e3,
        worst.as_secs_f64() * 1e3,
        total.as_secs_f64() * 1e3,
        undo_t.as_secs_f64() * 1e3
    );
    let bound = if cfg!(debug_assertions) { 3000.0 } else { 300.0 };
    assert!(paste.as_secs_f64() * 1e3 < bound, "paste frame {paste:?}");
    assert!(worst.as_secs_f64() * 1e3 < 100.0, "typing after a big paste {worst:?}");
}

/// Type `keys` (text, or "\n" for Enter) into a fresh editor at the end of `text`.
fn typed(text: &str, lang: Lang, keys: &[&str]) -> H {
    let mut h = H::new(text, lang);
    h.st.goto(u32::MAX, u32::MAX);
    h.frame();
    for k in keys {
        if *k == "\n" {
            h.key(Key::Enter, NONE);
        } else {
            h.typ(k);
        }
    }
    h
}

#[test]
fn auto_indent_per_language_family() {
    // C-like: level after `{`, closer dedents, Enter between a pair splits it.
    let h = typed("fn a() {", Lang::Rust, &["\n", "x();", "\n", "}"]);
    assert_eq!(h.doc.text(), "fn a() {\n    x();\n}");
    let h = typed("\tif (x) {", Lang::C, &["\n", "y;"]);
    assert_eq!(h.doc.text(), "\tif (x) {\n\t    y;", "leading tab kept verbatim, soft-tab level added");
    let mut h = H::new("  f({})", Lang::JavaScript);
    h.st.goto(0, 5);
    h.frame();
    h.key(Key::Enter, NONE);
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("  f({\n      \n  })", (1, 6)));
    undo(&mut h);
    assert_eq!(h.doc.text(), "  f({})", "one Enter = one undo step");
    // Python / YAML.
    let h = typed("def f(x):  # doc", Lang::Python, &["\n", "return x"]);
    assert_eq!(h.doc.text(), "def f(x):  # doc\n    return x");
    let h = typed("root:", Lang::Yaml, &["\n", "k: v", "\n"]);
    assert_eq!(h.doc.text(), "root:\n    k: v\n    ");
    // Lua / Ruby / Shell.
    let h = typed("for i = 1, 3 do", Lang::Lua, &["\n", "print(i)", "\n"]);
    assert_eq!(h.doc.text(), "for i = 1, 3 do\n    print(i)\n    ");
    let h = typed("def go(x)", Lang::Ruby, &["\n"]);
    assert_eq!(h.doc.text(), "def go(x)\n    ");
    let h = typed("if [ -f a ]; then", Lang::Shell, &["\n"]);
    assert_eq!(h.doc.text(), "if [ -f a ]; then\n    ");
    // `=>` without a brace: no level.
    let h = typed("  const f = x =>", Lang::TypeScript, &["\n"]);
    assert_eq!(h.doc.text(), "  const f = x =>\n  ");
}

#[test]
fn closer_dedent_and_undo() {
    let mut h = typed("fn a() {\n    if x {\n        y();\n    }\n        ", Lang::Rust, &["}"]);
    assert_eq!(h.doc.text(), "fn a() {\n    if x {\n        y();\n    }\n}");
    undo(&mut h);
    assert_eq!(h.doc.text(), "fn a() {\n    if x {\n        y();\n    }\n        ");
    // Not first non-blank: plain insert.
    let h = typed("{\n    x", Lang::Rust, &["}"]);
    assert_eq!(h.doc.text(), "{\n    x}");
    // Pasted text is not re-indented.
    let mut h = H::new("fn a() {\n        ", Lang::Rust);
    h.st.goto(1, 8);
    h.frame();
    h.shot.paste("}");
    h.frame();
    assert_eq!(h.doc.text(), "fn a() {\n        }");
    h.shot.paste("\n  z(");
    h.frame();
    assert_eq!(h.doc.text(), "fn a() {\n        }\n  z(");
}

#[test]
fn markdown_lists_through_the_widget() {
    let mut h = typed("1. one", Lang::Markdown, &["\n", "two", "\n"]);
    assert_eq!(h.doc.text(), "1. one\n2. two\n3. ");
    h.key(Key::Enter, NONE);
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("1. one\n2. two\n", (2, 0)), "Enter on an empty item ends the list");
    undo(&mut h);
    assert_eq!(h.doc.text(), "1. one\n2. two\n3. ");
    let h = typed("- [x] done", Lang::Markdown, &["\n", "next"]);
    assert_eq!(h.doc.text(), "- [x] done\n- [ ] next");
    let h = typed("> quote", Lang::Markdown, &["\n", "more", "\n", "\n"]);
    assert_eq!(h.doc.text(), "> quote\n> more\n");
    // Tab / shift+Tab on a list item indent / outdent the item.
    let mut h = typed("- a\n- b", Lang::Markdown, &[]);
    h.key(Key::Tab, NONE);
    assert_eq!((h.doc.text().as_str(), h.st.cursor()), ("- a\n    - b", (1, 7)));
    h.key(Key::Tab, shift());
    assert_eq!(h.doc.text(), "- a\n- b");
    // Not a list item: Tab inserts spaces at the caret.
    let mut h = typed("ab", Lang::Markdown, &[]);
    h.key(Key::Tab, NONE);
    assert_eq!(h.doc.text(), "ab  ");
}

#[test]
fn auto_indent_off_inserts_bare_newlines() {
    let mut h = H::new("    fn a() {}", Lang::Rust);
    h.opts.auto_indent = false;
    h.st.goto(0, 12);
    h.frame();
    h.key(Key::Enter, NONE);
    assert_eq!(h.doc.text(), "    fn a() {\n}");
    h.typ("    ");
    h.typ("}");
    assert_eq!(h.doc.text(), "    fn a() {\n    }}", "no dedent when off");
    let mut h = typed("- a", Lang::Markdown, &[]);
    h.opts.auto_indent = false;
    h.key(Key::Enter, NONE);
    h.key(Key::Tab, NONE);
    assert_eq!(h.doc.text(), "- a\n    ");
}
