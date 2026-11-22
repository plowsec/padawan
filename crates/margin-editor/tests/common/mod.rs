//! Test harness: one editor in a headless egui (`margin_shot::Shot`).
#![allow(dead_code)]

use egui::{Key, Modifiers, Pos2, Rect};
use margin_code::Lang;
use margin_editor::{show, Document, EditorOptions, EditorOutput, EditorState, Lens};
use margin_shot::Shot;
use margin_theme::Theme;

/// Screenshot output directory (absolute, inside `target/`).
pub const SHOTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/agent-editor/shots");

pub struct H {
    pub shot: Shot,
    pub doc: Document,
    pub st: EditorState,
    pub opts: EditorOptions,
    pub theme: Theme,
    pub lenses: Vec<Lens>,
    pub out: EditorOutput,
    /// Every output since the last `take_outputs`.
    pub outs: Vec<EditorOutput>,
    pub editor_id: Option<egui::Id>,
}

impl H {
    pub fn new(text: &str, lang: Lang) -> H {
        H::with_size(text, lang, [800.0, 600.0])
    }

    pub fn with_size(text: &str, lang: Lang, size: [f32; 2]) -> H {
        let shot = Shot::new(size, 2.0);
        // Not `Theme::apply`: it overflows a u8 in debug builds (dark popup shadow); the editor reads `Theme` directly.
        margin_theme::fonts::install(shot.ctx());
        let mut h = H {
            shot,
            doc: Document::new(text, lang),
            st: EditorState::default(),
            opts: EditorOptions { request_focus: true, ..Default::default() },
            theme: Theme::default(),
            lenses: Vec::new(),
            out: EditorOutput::default(),
            outs: Vec::new(),
            editor_id: None,
        };
        h.frames(2);
        h.opts.request_focus = false;
        h.frames(1);
        h
    }

    pub fn frame(&mut self) -> &EditorOutput {
        let H { shot, doc, st, opts, theme, lenses, out, outs, editor_id } = self;
        shot.run(1, |ui| {
            *editor_id = Some(ui.make_persistent_id("margin-editor"));
            *out = show(ui, doc, st, opts, theme, lenses);
        });
        outs.push(out.clone());
        &self.out
    }

    pub fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame();
        }
    }

    pub fn key(&mut self, k: Key, m: Modifiers) {
        self.shot.key(k, m);
        self.frame();
    }

    pub fn typ(&mut self, s: &str) {
        self.shot.text(s);
        self.frame();
    }

    pub fn ev(&mut self, e: egui::Event) {
        self.shot.event(e);
        self.frame();
    }

    /// Screen rect of the caret after moving it to `p` (one frame).
    pub fn caret_at(&mut self, p: (u32, u32)) -> Rect {
        self.st.goto(p.0, p.1);
        self.frames(2);
        self.st.cursor_rect().expect("caret visible")
    }

    pub fn click(&mut self, at: Pos2, m: Modifiers) {
        self.shot.press_at([at.x, at.y], m);
        self.frame();
        self.shot.button(false, m);
        self.frame();
    }

    pub fn widget_rect(&self, salt: &str) -> Option<Rect> {
        let id = self.editor_id?.with(salt);
        self.shot.ctx().read_response(id).map(|r| r.rect)
    }

    pub fn any_out(&self, f: impl Fn(&EditorOutput) -> bool) -> bool {
        self.outs.iter().any(f)
    }
}

pub fn cmd() -> Modifiers {
    Modifiers::COMMAND
}

pub fn shift() -> Modifiers {
    Modifiers::SHIFT
}
