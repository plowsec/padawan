//! Frame-time checks on a 1,000,000-line document. Prints the numbers (`--nocapture`).
//! Bounds: generous in debug, ~16 ms in release (target: < 4 ms).

mod common;

use std::time::{Duration, Instant};

use common::H;
use egui::{Key, Modifiers};
use margin_code::Lang;
use margin_editor::Document;

fn bound() -> Duration {
    if cfg!(debug_assertions) {
        Duration::from_millis(400)
    } else {
        Duration::from_millis(16)
    }
}

fn ms(d: Duration) -> String {
    format!("{:.2} ms", d.as_secs_f64() * 1000.0)
}

#[test]
fn million_lines() {
    let lines = 1_000_000;
    let mut text = String::with_capacity(lines * 41);
    for i in 0..lines {
        let l = format!("let v{i:07} = compute(the, input, {});", i % 97);
        text.push_str(&format!("{l:<40}\n"));
    }
    let t0 = Instant::now();
    let doc = Document::new(&text, Lang::Rust);
    let t_new = t0.elapsed();
    assert_eq!(doc.line_count() as usize, lines + 1);
    drop(doc);

    let mut h = H::with_size(&text, Lang::Rust, [1200.0, 900.0]);
    h.frame();
    let first = h.shot.worst_frame;
    h.shot.worst_frame = Duration::ZERO;

    // Scroll: wheel at the top, then a scrollbar-sized jump.
    h.shot.move_to([500.0, 400.0]);
    h.shot.scroll([0.0, -3000.0]);
    let mut scroll = Duration::ZERO;
    for _ in 0..10 {
        h.frame();
        scroll = scroll.max(h.shot.last_frame);
    }
    h.st.set_top_line(500_000);
    h.frame();
    scroll = scroll.max(h.shot.last_frame);

    // Typing in the middle (char, newline, backspace).
    h.st.goto(500_010, 5);
    h.frames(2);
    let mut typing = Duration::ZERO;
    for s in ["a", "b", "c"] {
        h.typ(s);
        typing = typing.max(h.shot.last_frame);
    }
    h.key(Key::Enter, Modifiers::NONE);
    let enter = h.shot.last_frame;
    h.key(Key::Backspace, Modifiers::NONE);
    typing = typing.max(h.shot.last_frame);
    let orig = format!("{:<40}", format!("let v{:07} = compute(the, input, {});", 500_010, 500_010 % 97));
    assert_eq!(h.doc.line(500_010), format!("{}abc{}", &orig[..5], &orig[5..]));

    // Goto the end: frames while the lexer catches up stay in budget.
    h.st.goto(u32::MAX, 0);
    h.frame();
    let goto = h.shot.last_frame;
    let mut lex_frames = 0;
    let mut lex_worst = Duration::ZERO;
    while h.shot.ctx().has_requested_repaint() && lex_frames < 20_000 {
        h.frame();
        lex_frames += 1;
        lex_worst = lex_worst.max(h.shot.last_frame);
    }
    assert!(h.st.top_line() > 999_900);

    // Find a common word: counter scans with a budget per frame.
    h.st.open_find(Some("compute"), false);
    let mut find_frames = 0;
    let mut find_worst = Duration::ZERO;
    let t0 = Instant::now();
    loop {
        h.frame();
        find_frames += 1;
        find_worst = find_worst.max(h.shot.last_frame);
        let s = h.st.find_status();
        if !s.ends_with('…') || find_frames > 50_000 {
            break;
        }
    }
    let find_total = t0.elapsed();
    let status = h.st.find_status();

    eprintln!("1M lines × 40 B ({}):", if cfg!(debug_assertions) { "debug" } else { "release" });
    eprintln!("  Document::new      {}", ms(t_new));
    eprintln!("  first frame        {}", ms(first));
    eprintln!("  scroll frame (max) {}", ms(scroll));
    eprintln!("  typing frame (max) {}", ms(typing));
    eprintln!("  enter frame        {}", ms(enter));
    eprintln!("  goto end frame     {}", ms(goto));
    eprintln!("  lex catch-up       {lex_frames} frames, worst {}", ms(lex_worst));
    eprintln!("  find 'compute'     {find_frames} frames, worst {}, total {}, status {status:?}", ms(find_worst), ms(find_total));

    let b = bound();
    for (what, d) in [("first", first), ("scroll", scroll), ("typing", typing), ("enter", enter), ("goto", goto), ("lex", lex_worst), ("find", find_worst)] {
        assert!(d < b, "{what} frame {} over {}", ms(d), ms(b));
    }
    assert!(status.contains("100000+"), "{status}");
}
