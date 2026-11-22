//! End to end with the real engine in-process (`margin_engine::spawn_local`) on a TempDir.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
use margin_engine::EngineConfig;
use margin_gui::{App, Probe, RunOpts};
use margin_proto::{Client, DirListing, IndexOpts, Opener, Waker};
use margin_shot::Shot;
use margin_testutil::TempDir;

struct LocalOpener;

impl Opener for LocalOpener {
    fn open(&mut self, root: &Path, opts: &IndexOpts, waker: Waker) -> Client {
        margin_engine::spawn_local(EngineConfig { root: root.to_path_buf(), cache_dir: None, opts: opts.clone(), threads: 2 }, waker)
    }
    fn sandbox(&self) -> &str {
        "none"
    }
    fn browse(&mut self, _path: Option<&Path>, _waker: Waker) {}
    fn poll_browse(&mut self) -> Option<DirListing> {
        None
    }
}

struct E {
    shot: Shot,
    app: App,
}

impl E {
    fn frames(&mut self, n: usize) {
        let app = &mut self.app;
        self.shot.run(n, |ui| app.ui(ui));
    }
    fn until(&mut self, what: &str, pred: impl Fn(&Probe) -> bool) -> Probe {
        let t0 = Instant::now();
        loop {
            self.frames(1);
            let p = self.app.probe();
            if pred(&p) {
                return p;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "timed out waiting for {what}: {p:#?}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn key(&mut self, k: Key, m: Modifiers) {
        self.shot.key(k, m);
        self.frames(1);
    }
    fn text(&mut self, s: &str) {
        self.shot.text(s);
        self.frames(1);
    }
}

#[test]
fn real_engine_open_edit_save_search() {
    let proj = TempDir::new("gui-e2e-proj");
    proj.write("src/lib.rs", "pub fn alpha() -> u32 {\n    beta()\n}\n\nfn beta() -> u32 {\n    1\n}\n");
    proj.write("README.md", "# e2e\n");
    let cfg = TempDir::new("gui-e2e-cfg");
    let shot = Shot::new([1280.0, 820.0], 1.0);
    margin_theme::fonts::install(shot.ctx());
    let root: PathBuf = proj.path().to_path_buf();
    let opts = RunOpts { opener: Box::new(LocalOpener), folder: Some(root.clone()), file: None, config_dir: Some(cfg.path().to_path_buf()), gui_sandbox: "none".into(), fresh: false };
    let app = App::new(shot.ctx(), opts);
    let mut e = E { shot, app };
    let p = e.until("tree", |p| p.folder.is_some() && p.tree.len() == 2);
    assert_eq!(p.tree.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), vec!["src", "README.md"]);

    // Open via Go to File (FindFiles on the real index).
    e.key(Key::T, Modifiers::COMMAND);
    e.text("lib");
    e.until("file match", |p| p.palette_selected.as_deref() == Some("file:src/lib.rs"));
    e.key(Key::Enter, Modifiers::NONE);
    e.until("tab", |p| p.active.as_deref() == Some("lib.rs") && p.active_view == "text");
    e.frames(2);

    // Edit + save; the file on disk changes.
    e.text("// gamma_marker");
    e.key(Key::Enter, Modifiers::NONE);
    e.until("dirty", |p| p.active_dirty);
    e.key(Key::S, Modifiers::COMMAND);
    e.until("saved", |p| !p.active_dirty);
    let disk = std::fs::read_to_string(proj.join("src/lib.rs")).unwrap();
    assert!(disk.starts_with("// gamma_marker\npub fn alpha()"), "{disk}");

    // Code hints from the real outline + engine lens.
    e.until("lenses", |p| !p.lenses.is_empty());

    // Search finds the saved text.
    e.key(Key::F, Modifiers::COMMAND | Modifiers::SHIFT);
    e.text("gamma_marker");
    let p = e.until("search hit", |p| p.search_hits == 1);
    assert_eq!(p.search_files, 1);
    e.app.on_exit();
}
