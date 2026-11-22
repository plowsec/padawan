//! Test harness: `margin_shot::Shot` + `App` + fake engine; pump frames until a predicate holds.
#![allow(dead_code)]

pub mod fake;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
use margin_gui::{App, Probe, RunOpts};
use margin_shot::{Shot, ShotError};
use margin_testutil::TempDir;

pub use fake::{Fake, FakeOpener, Script};

pub const ROOT: &str = "/fake/proj";

pub fn shots_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/agent-gui/shots");
    let _ = std::fs::create_dir_all(&d);
    d.canonicalize().unwrap_or(d)
}

pub struct H {
    pub shot: Shot,
    pub app: App,
    pub fake: Fake,
    pub cfg: TempDir,
}

pub fn sample_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("README.md", "# Proj\n\nNotes.\n"),
        ("src/main.c", "#include <stdio.h>\n\nint parse_header(char *p) {\n    memcpy(buf, p, 10);\n    return helper(p);\n}\n\nint helper(char *p) {\n    return 0;\n}\n\nint main(void) {\n    parse_header(0);\n    parse_header(1);\n    return 0;\n}\n"),
        ("src/util.rs", "fn util() {\n    println!(\"x\");\n}\n"),
        ("src/lib/deep.py", "def run_it(x):\n    eval(x)\n"),
        ("docs/guide.md", "Guide text\nmore text\n"),
        ("build.log", "ignored log\n"),
    ]
}

impl H {
    pub fn new(files: &[(&str, &str)]) -> H {
        Self::with(files, Script::default(), Some(ROOT))
    }

    pub fn with(files: &[(&str, &str)], script: Script, folder: Option<&str>) -> H {
        Self::sized(files, script, folder, [1280.0, 820.0], 1.0)
    }

    pub fn sized(files: &[(&str, &str)], script: Script, folder: Option<&str>, size: [f32; 2], ppp: f32) -> H {
        Self::build(files, script, folder, size, ppp, &[])
    }

    /// `conf`: files written into the config dir first (`("state.conf", "...")`).
    pub fn build(files: &[(&str, &str)], script: Script, folder: Option<&str>, size: [f32; 2], ppp: f32, conf: &[(&str, &str)]) -> H {
        let fake = Fake::new(files);
        *fake.script.lock().unwrap() = script;
        let cfg = TempDir::new("gui-cfg");
        for (name, text) in conf {
            cfg.write(name, text);
        }
        let shot = Shot::new(size, ppp);
        margin_theme::fonts::install(shot.ctx());
        let opts = RunOpts { opener: Box::new(FakeOpener::new(fake.clone())), folder: folder.map(PathBuf::from), file: None, config_dir: Some(cfg.path().to_path_buf()), gui_sandbox: "seatbelt".into() };
        let app = App::new(shot.ctx(), opts);
        let mut h = H { shot, app, fake, cfg };
        h.frames(2);
        h
    }

    pub fn frames(&mut self, n: usize) {
        let app = &mut self.app;
        self.shot.run(n, |ui| app.ui(ui));
    }

    pub fn probe(&self) -> Probe {
        self.app.probe()
    }

    /// Run frames until `pred` holds (5 s real time max).
    pub fn until(&mut self, what: &str, pred: impl Fn(&Probe) -> bool) -> Probe {
        let t0 = Instant::now();
        loop {
            self.frames(1);
            let p = self.probe();
            if pred(&p) {
                return p;
            }
            if t0.elapsed() > Duration::from_secs(5) {
                panic!("timed out waiting for {what}: {p:#?}");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn key(&mut self, k: Key, m: Modifiers) {
        self.shot.key(k, m);
        self.frames(1);
    }

    pub fn type_text(&mut self, s: &str) {
        self.shot.text(s);
        self.frames(1);
    }

    pub fn click(&mut self, pos: egui::Pos2) {
        self.click_mod(pos, Modifiers::NONE);
    }

    /// Hover, press, release: three frames, like a real click.
    pub fn click_mod(&mut self, pos: egui::Pos2, m: Modifiers) {
        self.shot.move_to([pos.x, pos.y]);
        self.frames(1);
        self.shot.button(true, m);
        self.frames(1);
        self.shot.release();
        self.frames(1);
    }

    /// Last frame's rect of widget `id`.
    pub fn rect_of(&self, id: egui::Id) -> Option<egui::Rect> {
        self.shot.ctx().read_response(id).map(|r| r.rect)
    }

    pub fn png(&mut self, name: &str) {
        let p = shots_dir().join(name);
        self.png_to(&p);
    }

    pub fn png_to(&mut self, p: &std::path::Path) {
        match self.shot.png(p) {
            Ok(()) => {}
            Err(ShotError::NoGpu(why)) => eprintln!("skip screenshot: {why}"),
            Err(e) => panic!("{e}"),
        }
    }

    /// Wait for the folder + root listing.
    pub fn opened(&mut self) -> Probe {
        self.until("folder open", |p| p.folder.is_some() && !p.tree.is_empty())
    }
}
