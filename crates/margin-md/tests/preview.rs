//! Renderer through `show` in a headless egui (`margin_shot::Shot`): no panic, clicks, scroll sync,
//! idle, screenshots, frame time. Prints numbers with `--nocapture`.

use std::time::{Duration, Instant};

use egui::{Modifiers, Pos2};
use margin_md::{parse, show, Hit, MdDoc, PreviewOutput, PreviewState};
use margin_shot::{Shot, ShotError};
use margin_theme::{Look, Mode, Theme};

const SHOTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/agent-md/shots");
const SAMPLE: &str = include_str!("sample.md");

struct H {
    shot: Shot,
    doc: MdDoc,
    st: PreviewState,
    theme: Theme,
    out: PreviewOutput,
    outs: Vec<PreviewOutput>,
}

impl H {
    fn new(text: &str, size: [f32; 2], ppp: f32, mode: Mode) -> H {
        let shot = Shot::new(size, ppp);
        // Not `Theme::apply` (see margin-editor tests); the preview reads `Theme` directly.
        margin_theme::fonts::install(shot.ctx());
        let mut h = H { shot, doc: parse(text), st: PreviewState::default(), theme: Theme::new(mode, Look::default()), out: PreviewOutput::default(), outs: Vec::new() };
        h.frames(2);
        h
    }

    fn frame(&mut self) -> &PreviewOutput {
        let H { shot, doc, st, theme, out, outs } = self;
        shot.run(1, |ui| *out = show(ui, doc, st, theme));
        outs.push(out.clone());
        &self.out
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame();
        }
    }

    fn click(&mut self, at: Pos2) {
        self.outs.clear();
        self.shot.move_to([at.x, at.y]);
        self.frame();
        self.shot.press_at([at.x, at.y], Modifiers::NONE);
        self.frame();
        self.shot.release();
        self.frames(2);
    }

    fn hit(&self, f: impl Fn(&Hit) -> bool) -> Option<egui::Rect> {
        self.st.hits().iter().find(|(_, h)| f(h)).map(|(r, _)| *r)
    }

    fn png(&mut self, name: &str) {
        let _ = std::fs::create_dir_all(SHOTS);
        match self.shot.png(format!("{SHOTS}/{name}")) {
            Err(ShotError::NoGpu(why)) => eprintln!("skip png: {why}"),
            r => r.unwrap(),
        }
    }
}

#[test]
fn renders_at_many_sizes_and_scales() {
    for mode in [Mode::Dark, Mode::Light] {
        for ppp in [1.0, 2.0] {
            for w in [320.0, 480.0, 900.0, 1600.0] {
                let mut h = H::new(SAMPLE, [w, 700.0], ppp, mode);
                for _ in 0..12 {
                    h.shot.move_to([w / 2.0, 300.0]);
                    h.shot.scroll([0.0, -400.0]);
                    h.frame();
                }
                assert!(h.out.top_line > 0, "scrolled at {w}");
            }
        }
    }
    // Tiny and empty.
    let mut h = H::new("", [10.0, 10.0], 1.0, Mode::Dark);
    h.frames(2);
    let mut h = H::new(SAMPLE, [40.0, 30.0], 1.0, Mode::Light);
    h.frames(2);
}

#[test]
fn link_click_returns_destination() {
    let mut h = H::new("# T\n\nSee [the spec](https://example.org/spec \"Spec\") now.\n", [800.0, 600.0], 1.0, Mode::Dark);
    let r = h.hit(|x| matches!(x, Hit::Link(_))).expect("link drawn");
    h.click(r.center());
    assert!(h.outs.iter().any(|o| o.link_clicked.as_deref() == Some("https://example.org/spec")), "{:?}", h.outs);
    // Nothing else reports a click.
    h.outs.clear();
    h.click(Pos2::new(5.0, 590.0));
    assert!(h.outs.iter().all(|o| o.link_clicked.is_none() && o.task_toggled.is_none()));
}

#[test]
fn task_click_returns_its_line() {
    let mut h = H::new("Intro\n\n- [ ] one\n- [x] two\n- [ ] three\n", [800.0, 600.0], 2.0, Mode::Light);
    let r = h.hit(|x| *x == Hit::Task(3)).expect("task box on line 3");
    h.click(r.center());
    assert!(h.outs.iter().any(|o| o.task_toggled == Some(3)), "{:?}", h.outs);
    assert!(h.outs.iter().all(|o| o.task_toggled != Some(2) && o.task_toggled != Some(4)));
}

#[test]
fn footnote_click_scrolls_inside() {
    let mut text = String::from("Ref[^a] here.\n\n");
    for i in 0..200 {
        text.push_str(&format!("Filler paragraph {i}.\n\n"));
    }
    text.push_str("[^a]: The note.\n");
    let mut h = H::new(&text, [800.0, 600.0], 1.0, Mode::Dark);
    let r = h.hit(|x| *x == Hit::Footnote(1)).expect("footnote ref");
    h.click(r.center());
    assert!(h.out.top_line >= 400, "top {}", h.out.top_line);
    assert!(h.outs.iter().all(|o| o.link_clicked.is_none()));
}

#[test]
fn scroll_to_line_round_trips() {
    let mut text = String::new();
    for i in 0..400 {
        text.push_str(&format!("## Section {i}\n\nBody text of section {i}, long enough to wrap maybe.\n\n"));
    }
    let mut h = H::new(&text, [700.0, 500.0], 1.0, Mode::Dark);
    for line in [0u32, 2, 800, 1200, 1596] {
        h.st.scroll_to_line(line);
        h.frame();
        assert_eq!(h.out.top_line, line, "scroll_to_line({line})");
        h.frame();
        assert_eq!(h.out.top_line, line, "stable after a frame");
    }
    // A line inside a block maps to the block's first line.
    h.st.scroll_to_line(803);
    h.frame();
    assert_eq!(h.out.top_line, 802);
    // Anchors.
    let doc = parse(&text);
    assert_eq!(doc.anchor_line("#section-300"), Some(1200));
}

#[test]
fn reparse_keeps_position() {
    let mut text = String::new();
    for i in 0..300 {
        text.push_str(&format!("Paragraph {i}.\n\n"));
    }
    let mut h = H::new(&text, [700.0, 500.0], 1.0, Mode::Dark);
    h.st.scroll_to_line(300);
    h.frame();
    assert_eq!(h.out.top_line, 300);
    // Edit far below: same top.
    text.push_str("New paragraph.\n");
    h.doc = parse(&text);
    h.frame();
    assert_eq!(h.out.top_line, 300);
}

#[test]
fn idle_frames_request_no_repaint() {
    let mut h = H::new(SAMPLE, [900.0, 800.0], 2.0, Mode::Dark);
    h.frames(3);
    h.frame();
    assert!(!h.shot.ctx().has_requested_repaint(), "repaint requested while idle");
}

#[test]
fn hostile_text_is_masked() {
    let doc = parse("a\u{202e}b \u{200b}[x](https://e.org/\u{2066}p) `c\u{feff}d`\n\n```\nk\u{202d}v\n```\n");
    let d = doc.dump();
    assert!(!d.contains('\u{202e}') && !d.contains('\u{200b}') && !d.contains('\u{feff}') && !d.contains('\u{202d}'), "{d}");
    assert!(d.contains("a·b") && d.contains("c·d") && d.contains("k·v"), "{d}");
    // The destination stays as written (returned to the app); its tooltip is masked at draw time.
    assert!(d.contains("https://e.org/\u{2066}p"));
    let mut h = H::new("[x](https://e.org/\u{2066}p)", [400.0, 300.0], 1.0, Mode::Dark);
    let r = h.hit(|x| matches!(x, Hit::Link(_))).expect("link");
    h.click(r.center());
    assert!(h.outs.iter().any(|o| o.link_clicked.as_deref() == Some("https://e.org/\u{2066}p")));
}

#[test]
fn screenshots() {
    for (mode, name) in [(Mode::Dark, "sample-dark.png"), (Mode::Light, "sample-light.png")] {
        let mut h = H::new(SAMPLE, [900.0, 1100.0], 2.0, mode);
        h.frames(2);
        h.png(name);
        for k in 1..=3 {
            h.shot.move_to([450.0, 500.0]);
            h.shot.scroll([0.0, -1000.0]);
            h.frames(3);
            h.png(&format!("{}-p{k}.png", name.trim_end_matches(".png")));
        }
    }
    let mut h = H::new(SAMPLE, [380.0, 1100.0], 2.0, Mode::Light);
    h.png("sample-narrow.png");
    for k in 1..=2 {
        h.shot.move_to([190.0, 500.0]);
        h.shot.scroll([0.0, -1000.0]);
        h.frames(3);
        h.png(&format!("sample-narrow-p{k}.png"));
    }
    // Hovered link (underline + tooltip).
    let mut h = H::new(SAMPLE, [900.0, 700.0], 2.0, Mode::Dark);
    if let Some(r) = h.hit(|x| matches!(x, Hit::Link(_))) {
        h.shot.move_to([r.center().x, r.center().y]);
        h.frames(40);
        h.png("hover-link.png");
    }
}

fn ms(d: Duration) -> String {
    format!("{:.2} ms", d.as_secs_f64() * 1000.0)
}

fn big_doc(bytes: usize) -> String {
    let mut s = String::with_capacity(bytes + 4096);
    let mut i = 0usize;
    while s.len() < bytes {
        s.push_str(&format!("## Chapter {i}\n\nThe keeper wrote entry {i} with *emphasis*, `code`, and a [link](https://example.org/{i}). It goes on long enough to wrap twice at a normal width, describing weather, wind and the lamp.\n\n"));
        s.push_str(&format!("- item {i} one\n- item two\n  - nested **three**\n\n"));
        if i.is_multiple_of(4) {
            s.push_str("```rust\nfn main() {\n    let x = 42; // answer\n    println!(\"{x}\");\n}\n```\n\n");
        }
        if i.is_multiple_of(7) {
            s.push_str("| a | b | c |\n|---|:-:|--:|\n| 1 | two | 3.0 |\n| 4 | five | 6.0 |\n\n");
        }
        if i.is_multiple_of(5) {
            s.push_str("> Quoted wisdom about the sea, nothing more.\n\n");
        }
        i += 1;
    }
    s
}

#[test]
fn frame_time_on_5mb() {
    let text = big_doc(5 << 20);
    let t0 = Instant::now();
    let doc = parse(&text);
    let t_parse = t0.elapsed();
    let blocks = doc.block_count();
    let shot = Shot::new([1200.0, 900.0], 2.0);
    margin_theme::fonts::install(shot.ctx());
    let mut h = H { shot, doc, st: PreviewState::default(), theme: Theme::default(), out: PreviewOutput::default(), outs: Vec::new() };
    h.frame();
    let first = h.shot.last_frame;
    h.frame();
    let second = h.shot.last_frame;
    assert!(!h.st.hits().is_empty(), "visible links were drawn");
    // Idle (cached) frame.
    let mut idle = Duration::ZERO;
    for _ in 0..5 {
        h.frame();
        idle = idle.max(h.shot.last_frame);
    }
    // Wheel scrolling.
    let mut wheel = Duration::ZERO;
    let mut wheel_sum = Duration::ZERO;
    for _ in 0..60 {
        h.shot.move_to([600.0, 400.0]);
        h.shot.scroll([0.0, -700.0]);
        h.frame();
        wheel = wheel.max(h.shot.last_frame);
        wheel_sum += h.shot.last_frame;
    }
    // Jumps (scroll_to_line), then the next frame (cached).
    let mut jump = Duration::ZERO;
    let mut after_jump = Duration::ZERO;
    let lines = text.lines().count() as u32;
    for k in 1..=10u32 {
        h.st.scroll_to_line(lines / 11 * k);
        h.frame();
        jump = jump.max(h.shot.last_frame);
        h.frame();
        after_jump = after_jump.max(h.shot.last_frame);
    }
    assert!(h.out.top_line > lines / 2);
    let mode = if cfg!(debug_assertions) { "debug" } else { "release" };
    eprintln!("5 MB markdown ({mode}), {blocks} blocks:");
    eprintln!("  parse                {}", ms(t_parse));
    eprintln!("  first frame          {}", ms(first));
    eprintln!("  second frame         {}", ms(second));
    eprintln!("  idle frame (max)     {}", ms(idle));
    eprintln!("  wheel frame (max)    {}  avg {}", ms(wheel), ms(wheel_sum / 60));
    eprintln!("  jump frame (max)     {}", ms(jump));
    eprintln!("  after jump (max)     {}", ms(after_jump));
    let bound = if cfg!(debug_assertions) { Duration::from_millis(1500) } else { Duration::from_millis(30) };
    assert!(first < bound, "first frame {}", ms(first));
    let idle_bound = if cfg!(debug_assertions) { Duration::from_millis(100) } else { Duration::from_millis(4) };
    assert!(idle < idle_bound, "idle frame {}", ms(idle));
}

// ---- selection ----

const SEL_DOC: &str = "# Title\n\nFirst paragraph here.\n\nSecond one with [a link](x.md).\n\n- item one\n- item two\n\n```\nlet x = 1;\nlet y = 2;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

impl H {
    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.shot.press_at([from.x, from.y], Modifiers::NONE);
        self.frame();
        for k in 1..=4 {
            let t = k as f32 / 4.0;
            self.shot.move_to([from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t]);
            self.frame();
        }
        self.shot.release();
        self.frames(2);
    }

    fn copy(&mut self) -> Option<String> {
        let n = self.shot.copied.len();
        self.shot.event(egui::Event::Copy);
        self.frames(2);
        self.shot.copied.get(n..).and_then(|c| c.last().cloned())
    }

    fn sel(&self) -> String {
        self.st.selected_text(&self.doc)
    }
}

#[test]
fn drag_selects_across_blocks_and_copies() {
    let mut h = H::new(SEL_DOC, [800.0, 700.0], 1.0, Mode::Dark);
    let r = h.st.text_rects();
    assert!(r.len() >= 9, "title, 2 paragraphs, 2 items, code, 4 cells: {r:?}");
    assert!(!h.st.has_selection(&h.doc));
    assert_eq!(h.copy(), None, "nothing selected, no focus: nothing copied");

    // Start of "First…" to the end of "Second…".
    h.drag(r[1].left_center() + egui::vec2(0.5, 0.0), r[2].right_center() + egui::vec2(30.0, 0.0));
    assert_eq!(h.sel(), "First paragraph here.\n\nSecond one with a link.");
    assert!(h.out.focused, "a press in the preview focuses it");
    assert_eq!(h.copy().as_deref(), Some("First paragraph here.\n\nSecond one with a link."));
    assert!(h.outs.iter().all(|o| o.link_clicked.is_none()), "a drag ending on a link is not a click");

    // Backwards drag gives the same text.
    h.drag(r[2].right_center() + egui::vec2(30.0, 0.0), r[1].left_center() + egui::vec2(0.5, 0.0));
    assert_eq!(h.sel(), "First paragraph here.\n\nSecond one with a link.");

    // Down to the bottom of the page: everything from the title on.
    h.drag(r[0].left_center() + egui::vec2(0.5, 0.0), Pos2::new(780.0, 690.0));
    assert_eq!(h.sel(), "Title\n\nFirst paragraph here.\n\nSecond one with a link.\n\n- item one\n- item two\n\nlet x = 1;\nlet y = 2;\n\na\tb\n1\t2");

    // A plain click drops the selection.
    h.click(r[1].center());
    assert!(!h.st.has_selection(&h.doc));
    assert_eq!(h.sel(), "");
}

#[test]
fn double_and_triple_click_shift_click_select_all() {
    let mut h = H::new(SEL_DOC, [800.0, 700.0], 2.0, Mode::Light);
    let r = h.st.text_rects();
    let at = r[1].left_center() + egui::vec2(8.0, 0.0); // inside "First"
    h.click(at);
    h.click(at);
    assert_eq!(h.sel(), "First");
    h.click(at);
    assert_eq!(h.sel(), "First paragraph here.");

    // Shift+click extends from the anchor.
    h.click(r[1].left_center() + egui::vec2(0.5, 0.0));
    let to = r[3].right_center() + egui::vec2(20.0, 0.0);
    h.shot.press_at([to.x, to.y], Modifiers::SHIFT);
    h.frame();
    h.shot.release();
    h.frames(2);
    assert_eq!(h.sel(), "First paragraph here.\n\nSecond one with a link.\n\n- item one");

    // Select all, copy, Esc.
    h.shot.key(egui::Key::A, Modifiers::COMMAND);
    h.frames(2);
    let all = h.sel();
    assert!(all.starts_with("Title\n\nFirst") && all.ends_with("a\tb\n1\t2"), "{all:?}");
    assert_eq!(h.copy().as_deref(), Some(all.as_str()));
    h.shot.key(egui::Key::Escape, Modifiers::NONE);
    h.frames(2);
    assert!(!h.st.has_selection(&h.doc));
}

#[test]
fn selection_in_code_and_table_cells() {
    let mut h = H::new(SEL_DOC, [800.0, 700.0], 1.0, Mode::Dark);
    let r = h.st.text_rects();
    let code = r[5];
    // Second code line only.
    h.drag(Pos2::new(code.left() + 0.5, code.bottom() - 4.0), Pos2::new(code.right() + 40.0, code.bottom() - 4.0));
    assert_eq!(h.sel(), "let y = 2;");
    // Both lines.
    h.drag(Pos2::new(code.left() + 0.5, code.top() + 4.0), Pos2::new(code.right() + 40.0, code.bottom() - 4.0));
    assert_eq!(h.sel(), "let x = 1;\nlet y = 2;");
    // Header cell "b" to body cell "1": row-major order.
    h.drag(r[7].left_center() + egui::vec2(0.5, 0.0), r[8].right_center() + egui::vec2(3.0, 0.0));
    assert_eq!(h.sel(), "b\n1");
    // One cell by double click.
    h.click(r[9].center());
    h.click(r[9].center());
    assert_eq!(h.sel(), "2");
}

#[test]
fn selection_survives_scroll_and_dies_with_the_document() {
    let mut text = String::new();
    for i in 0..300 {
        text.push_str(&format!("Paragraph number {i}.\n\n"));
    }
    let mut h = H::new(&text, [800.0, 600.0], 1.0, Mode::Dark);
    let r = h.st.text_rects();
    h.drag(r[0].left_center() + egui::vec2(0.5, 0.0), r[1].right_center() + egui::vec2(10.0, 0.0));
    assert_eq!(h.sel(), "Paragraph number 0.\n\nParagraph number 1.");
    // Scrolled far away: still selected (positions are in the document, not on screen).
    for _ in 0..10 {
        h.shot.move_to([400.0, 300.0]);
        h.shot.scroll([0.0, -600.0]);
        h.frame();
    }
    assert!(h.out.top_line > 20);
    assert_eq!(h.sel(), "Paragraph number 0.\n\nParagraph number 1.");

    // Dragging below the bottom edge scrolls and extends.
    let top0 = h.out.top_line;
    let r = h.st.text_rects();
    h.shot.press_at([r[0].left() + 0.5, r[0].center().y], Modifiers::NONE);
    h.frame();
    h.shot.move_to([400.0, 640.0]);
    h.frames(30);
    h.shot.release();
    h.frames(2);
    assert!(h.out.top_line > top0 + 10, "auto-scroll: {} -> {}", top0, h.out.top_line);
    assert!(h.sel().matches("Paragraph number").count() > 15, "{}", h.sel().len());

    // Select all on a big document copies all of it, laid out or not.
    h.shot.key(egui::Key::A, Modifiers::COMMAND);
    h.frames(2);
    let all = h.sel();
    assert!(all.starts_with("Paragraph number 0.") && all.ends_with("Paragraph number 299."), "{}", all.len());

    // New document: the selection is gone.
    h.doc = parse("Other text.\n");
    h.frames(2);
    assert!(!h.st.has_selection(&h.doc));
    assert_eq!(h.sel(), "");
}

#[test]
fn context_press_reports_position_and_link() {
    let mut h = H::new(SEL_DOC, [800.0, 700.0], 1.0, Mode::Dark);
    let r = h.st.text_rects();
    let link = h.hit(|x| matches!(x, Hit::Link(_))).expect("link");
    let secondary = |h: &mut H, at: Pos2| {
        h.outs.clear();
        h.shot.move_to([at.x, at.y]);
        h.frame();
        h.shot.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
        h.frame();
        h.shot.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
        h.frames(2);
        h.outs.iter().find(|o| o.context_menu.is_some()).cloned()
    };
    // Select a paragraph, then right-click it: the selection stays.
    h.click(r[1].center());
    h.click(r[1].center());
    h.click(r[1].center());
    let o = secondary(&mut h, r[1].center()).expect("menu");
    assert_eq!(o.context_menu, Some(r[1].center()));
    assert_eq!(o.context_link, None);
    assert_eq!(h.sel(), "First paragraph here.");
    let o = secondary(&mut h, link.center()).expect("menu on the link");
    assert_eq!(o.context_link.as_deref(), Some("x.md"));
    assert!(h.outs.iter().all(|o| o.link_clicked.is_none()), "secondary press does not follow the link");
}

#[test]
fn selection_screenshot() {
    for (mode, name) in [(Mode::Dark, "dark"), (Mode::Light, "light")] {
        let mut h = H::new(SEL_DOC, [800.0, 520.0], 2.0, mode);
        let r = h.st.text_rects();
        h.drag(r[1].left_center() + egui::vec2(60.0, 0.0), r[8].right_center() + egui::vec2(3.0, 0.0));
        assert!(h.sel().contains("Second one with a link.\n\n- item one") && h.sel().ends_with("a\tb\n1"), "{:?}", h.sel());
        h.png(&format!("selection-{name}.png"));
    }
}
