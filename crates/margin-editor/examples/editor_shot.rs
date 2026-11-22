//! Showcase render: Rust code, hint rows, a selection, the find bar, a hazard char; dark and light.
//!
//! `cargo run --example editor_shot -- /abs/dark.png [/abs/light.png] [/abs/prose.png] [/abs/small.png] [/abs/large.png]`
//! small: 10 pt, line height 1.2, light. large: 20 pt, line height 2.0, dark, find + replace row.

use margin_code::Lang;
use margin_editor::{show, Document, EditorOptions, EditorState, Lens, LensSeg, LensTone};
use margin_shot::{Shot, ShotError};
use margin_theme::{Look, Mode, Theme};

const CODE: &str = r#"//! Packet parser.
use std::io::{self, Read};

/// One frame on the wire.
#[derive(Debug, Clone)]
pub struct Frame {
    pub kind: u8,
    pub len: usize,
    pub body: Vec<u8>,
}

pub fn parse(buf: &[u8]) -> io::Result<Frame> {
    if buf.len() < 3 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "short"));
    }
    let kind = buf[0];
    let len = u16::from_be_bytes([buf[1], buf[2]]) as usize;
    let body = buf[3..3 + len].to_vec(); // <RLO> } <LRI> if is_admin <PDI> <LRI> begin admins only
    Ok(Frame { kind, len, body })
}

fn copy_into(dst: &mut [u8], src: &[u8]) {
	let n = src.len().min(dst.len());
	dst[..n].copy_from_slice(&src[..n]);
}

pub fn read_frame(r: &mut impl Read) -> io::Result<Frame> {
    let mut head = [0u8; 3];
    r.read_exact(&mut head)?;
    let len = u16::from_be_bytes([head[1], head[2]]) as usize;
    let mut buf = vec![0u8; 3 + len];
    buf[..3].copy_from_slice(&head);
    r.read_exact(&mut buf[3..])?;
    parse(&buf)
}
"#;

const PROSE: &str = "# Notes\n\nMargin wraps prose at word boundaries; continuation rows keep the indentation of their line, so lists stay readable even when an item runs long enough to need several rows on a narrow window.\n\n- A list item that is long enough to wrap onto a second row, aligned under its own text rather than the bullet.\n- Short item.\n\n    Indented paragraph with a zero\u{200B}width space hidden inside it, which shows as a box.\n";

fn lenses() -> Vec<Lens> {
    let seg = |t: &str, tone, id| LensSeg { text: t.into(), tone, id, hover: format!("{t} (click to list)") };
    vec![
        Lens { line: 11, segs: vec![seg("3 callers", LensTone::Normal, 1), seg("2 callees", LensTone::Normal, 2), seg("sinks: copy_from_slice", LensTone::Sink, 3)] },
        Lens { line: 21, segs: vec![seg("1 caller", LensTone::Normal, 4)] },
        Lens { line: 26, segs: vec![seg("indexing…", LensTone::Faint, 5)] },
    ]
}

fn render(path: &str, mode: Mode, text: &str, lang: Lang, size: [f32; 2], look: Look, find: bool) -> Result<(), ShotError> {
    let theme = Theme::new(mode, look);
    let mut shot = Shot::new(size, 2.0);
    apply_theme(shot.ctx(), &theme);
    margin_theme::fonts::install(shot.ctx());
    let text = text.replace("<RLO>", "\u{202E}").replace("<LRI>", "\u{2066}").replace("<PDI>", "\u{2069}");
    let mut doc = Document::new(&text, lang);
    let mut st = EditorState::default();
    let lenses = if lang == Lang::Rust { lenses() } else { Vec::new() };
    let mut opts = EditorOptions { request_focus: true, ..EditorOptions::default() };
    let frame = |shot: &mut Shot, opts: &EditorOptions, st: &mut EditorState, doc: &mut Document| {
        shot.run(1, |ui| {
            show(ui, doc, st, opts, &theme, &lenses);
        });
    };
    frame(&mut shot, &opts, &mut st, &mut doc);
    opts.request_focus = false;
    if lang == Lang::Rust {
        st.select(20, 11, 15); // `src.len().min(` area
        st.goto(27, 8);
        st.select(26, 4, 22);
        if find {
            st.open_find(Some("len"), look.code_size > 15.0);
        }
    } else {
        st.goto(4, 30);
    }
    for _ in 0..6 {
        frame(&mut shot, &opts, &mut st, &mut doc);
    }
    if find {
        st.find_next(&doc);
        for _ in 0..4 {
            frame(&mut shot, &opts, &mut st, &mut doc);
        }
    }
    shot.png(path)
}

/// `Theme::apply`, tolerating a panic inside it (reported upstream: u8 overflow in the dark popup shadow).
fn apply_theme(ctx: &egui::Context, theme: &Theme) {
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| theme.apply(ctx))).is_ok();
    if !ok {
        ctx.set_visuals(if theme.mode == Mode::Dark { egui::Visuals::dark() } else { egui::Visuals::light() });
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: editor_shot /abs/dark.png [/abs/light.png] [/abs/prose.png]");
        std::process::exit(2);
    }
    let look = |code_size, line_height| Look { code_size, line_height, ..Look::default() };
    let jobs: Vec<(Mode, &str, Lang, [f32; 2], Look)> = vec![
        (Mode::Dark, CODE, Lang::Rust, [980.0, 760.0], look(13.0, 1.5)),
        (Mode::Light, CODE, Lang::Rust, [980.0, 760.0], look(13.0, 1.5)),
        (Mode::Light, PROSE, Lang::Markdown, [560.0, 420.0], look(14.0, 1.6)),
        (Mode::Light, CODE, Lang::Rust, [760.0, 560.0], look(10.0, 1.2)),
        (Mode::Dark, CODE, Lang::Rust, [1200.0, 900.0], look(20.0, 2.0)),
    ];
    for (path, (mode, text, lang, size, lk)) in args.iter().zip(jobs) {
        match render(path, mode, text, lang, size, lk, lang == Lang::Rust) {
            Ok(()) => eprintln!("wrote {path}"),
            Err(ShotError::NoGpu(why)) => eprintln!("skip (no GPU): {why}"),
            Err(e) => {
                eprintln!("{path}: {e}");
                std::process::exit(1);
            }
        }
    }
}
