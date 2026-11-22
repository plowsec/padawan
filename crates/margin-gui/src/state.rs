//! `state.conf` (what the app remembers) and the background writer for both config files.
//!
//! | Key | Meaning |
//! |---|---|
//! | `recent = <abs path>` | recent folders, most recent first, max `MAX_RECENT` |
//! | `sidebar_width = <pt>` / `sidebar = on/off` | sidebar |
//! | `folder = <abs path>` | starts a folder block; the `tab` / `active` lines after it belong to it |
//! | `tab = <rel path>` / `active = <rel path>` | open tabs of that folder, in order |
//!
//! Paths with control characters are never written (they would break the line format).
//! Writes: temp file + rename, on the writer thread; only `settings.conf`, `state.conf` (+ `.tmp`) in an absolute config dir.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use crate::settings::parse_conf;

pub const MAX_RECENT: usize = 12;
const MAX_FOLDERS: usize = 32;
const MAX_TABS: usize = 64;
pub const SIDEBAR_W: std::ops::RangeInclusive<f32> = 160.0..=640.0;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FolderTabs {
    pub root: String,
    pub tabs: Vec<String>,
    pub active: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedState {
    pub recent: Vec<String>,
    pub sidebar_width: f32,
    pub sidebar_visible: bool,
    /// Most recently used first.
    pub folders: Vec<FolderTabs>,
}

impl Default for SavedState {
    fn default() -> Self {
        Self { recent: Vec::new(), sidebar_width: 260.0, sidebar_visible: true, folders: Vec::new() }
    }
}

fn line_safe(s: &str) -> bool {
    !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control) && s.trim() == s
}

impl SavedState {
    pub fn parse(text: &str) -> SavedState {
        let mut s = SavedState::default();
        let mut cur: Option<FolderTabs> = None;
        for (k, v) in parse_conf(text) {
            match k.as_str() {
                "recent" if line_safe(&v) && s.recent.len() < MAX_RECENT && !s.recent.contains(&v) => s.recent.push(v),
                "sidebar_width" => {
                    if let Ok(w) = v.parse::<f32>() {
                        if w.is_finite() {
                            s.sidebar_width = w.clamp(*SIDEBAR_W.start(), *SIDEBAR_W.end());
                        }
                    }
                }
                "sidebar" => s.sidebar_visible = v != "off",
                "folder" => {
                    if let Some(f) = cur.take() {
                        s.push_folder(f);
                    }
                    cur = line_safe(&v).then(|| FolderTabs { root: v, ..Default::default() });
                }
                "tab" => {
                    if let Some(f) = cur.as_mut() {
                        if f.tabs.len() < MAX_TABS && margin_proto::RelPath::new(&v).is_ok() && !v.is_empty() && !f.tabs.contains(&v) {
                            f.tabs.push(v);
                        }
                    }
                }
                "active" => {
                    if let Some(f) = cur.as_mut() {
                        if f.tabs.contains(&v) {
                            f.active = Some(v);
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(f) = cur.take() {
            s.push_folder(f);
        }
        s
    }

    fn push_folder(&mut self, f: FolderTabs) {
        if self.folders.len() < MAX_FOLDERS && !self.folders.iter().any(|g| g.root == f.root) {
            self.folders.push(f);
        }
    }

    pub fn write(&self) -> String {
        let mut out = String::from("# Margin state. Rewritten by the app.\n");
        out += &format!("sidebar_width = {}\nsidebar = {}\n", self.sidebar_width.round(), if self.sidebar_visible { "on" } else { "off" });
        for r in self.recent.iter().filter(|r| line_safe(r)).take(MAX_RECENT) {
            out += &format!("recent = {r}\n");
        }
        for f in self.folders.iter().filter(|f| line_safe(&f.root)).take(MAX_FOLDERS) {
            out += &format!("folder = {}\n", f.root);
            for t in f.tabs.iter().filter(|t| line_safe(t)).take(MAX_TABS) {
                out += &format!("tab = {t}\n");
            }
            if let Some(a) = f.active.as_ref().filter(|a| line_safe(a)) {
                out += &format!("active = {a}\n");
            }
        }
        out
    }

    /// Move `root` to the front of the recent list.
    pub fn touch_recent(&mut self, root: &str) {
        if !line_safe(root) {
            return;
        }
        self.recent.retain(|r| r != root);
        self.recent.insert(0, root.to_owned());
        self.recent.truncate(MAX_RECENT);
    }

    pub fn tabs_of(&self, root: &str) -> Option<&FolderTabs> {
        self.folders.iter().find(|f| f.root == root)
    }

    pub fn set_tabs(&mut self, f: FolderTabs) {
        self.folders.retain(|g| g.root != f.root);
        self.folders.insert(0, f);
        self.folders.truncate(MAX_FOLDERS);
    }
}

// ---- writer thread ----

enum Job {
    Write(&'static str, String),
    Flush(Sender<()>),
}

/// Writes config files off the UI thread. `None` dir (or a relative/empty one): writes nothing.
pub struct Persist {
    dir: Option<PathBuf>,
    tx: Option<Sender<Job>>,
}

/// Config dir if it is safe to write into.
fn usable(dir: Option<&Path>) -> Option<PathBuf> {
    let d = dir?;
    (d.is_absolute() && !d.as_os_str().is_empty() && d.parent().is_some()).then(|| d.to_path_buf())
}

impl Persist {
    pub fn new(dir: Option<&Path>) -> Persist {
        let dir = usable(dir);
        let tx = dir.as_ref().and_then(|d| {
            let (tx, rx) = channel();
            let d = d.clone();
            std::thread::Builder::new().name("margin-config".into()).spawn(move || writer(d, rx)).ok().map(|_| tx)
        });
        Persist { dir, tx }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Read a config file (start-up only, before the first frame). Capped; missing → empty.
    pub fn load(&self, name: &str) -> String {
        let Some(d) = &self.dir else { return String::new() };
        use std::io::Read;
        let mut buf = Vec::new();
        if let Ok(f) = std::fs::File::open(d.join(name)) {
            let _ = f.take(crate::settings::MAX_CONF as u64).read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    pub fn save_settings(&self, text: String) {
        self.send(Job::Write("settings.conf", text));
    }

    pub fn save_state(&self, text: String) {
        self.send(Job::Write("state.conf", text));
    }

    fn send(&self, j: Job) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(j);
        }
    }

    /// Wait (bounded) until every queued write is done. Quit and tests only.
    pub fn flush(&self, timeout: Duration) {
        let (tx, rx) = channel();
        self.send(Job::Flush(tx));
        if self.tx.is_some() {
            let _ = rx.recv_timeout(timeout);
        }
    }
}

fn writer(dir: PathBuf, rx: Receiver<Job>) {
    for job in rx {
        match job {
            Job::Write(name, text) => {
                debug_assert!(name == "settings.conf" || name == "state.conf");
                let tmp = dir.join(format!("{name}.tmp"));
                let dst = dir.join(name);
                if std::fs::write(&tmp, text.as_bytes()).is_ok() && std::fs::rename(&tmp, &dst).is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
            }
            Job::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut s = SavedState { sidebar_width: 300.0, sidebar_visible: false, ..Default::default() };
        s.touch_recent("/a");
        s.touch_recent("/b c");
        s.touch_recent("/a");
        assert_eq!(s.recent, vec!["/a", "/b c"]);
        s.set_tabs(FolderTabs { root: "/a".into(), tabs: vec!["x.rs".into(), "d/y.md".into()], active: Some("d/y.md".into()) });
        s.set_tabs(FolderTabs { root: "/b c".into(), tabs: vec![], active: None });
        assert_eq!(SavedState::parse(&s.write()), s);
        for i in 0..20 {
            s.touch_recent(&format!("/r{i}"));
        }
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0], "/r19");
    }

    #[test]
    fn hostile_state() {
        let s = SavedState::parse("tab = orphan\nfolder = /f\ntab = ../etc/passwd\ntab = ok.rs\nactive = nope\nrecent = \u{7}x\nsidebar_width = 1e30\nfolder = \nfolder = /f\n");
        assert_eq!(s.folders.len(), 1);
        assert_eq!(s.folders[0].tabs, vec!["ok.rs"]);
        assert_eq!(s.folders[0].active, None);
        assert!(s.recent.is_empty());
        assert_eq!(s.sidebar_width, 640.0);
        let mut w = SavedState::default();
        w.touch_recent("/evil\nrecent = /x");
        assert!(w.recent.is_empty());
    }

    #[test]
    fn writes_atomically_into_the_config_dir() {
        let t = margin_testutil::TempDir::new("gui-persist");
        let p = Persist::new(Some(t.path()));
        p.save_state("sidebar = off\n".into());
        p.save_settings("ui_size = 11\n".into());
        p.flush(Duration::from_secs(5));
        assert_eq!(std::fs::read_to_string(t.path().join("state.conf")).unwrap(), "sidebar = off\n");
        assert_eq!(p.load("settings.conf"), "ui_size = 11\n");
        assert!(!t.path().join("state.conf.tmp").exists());
        // Relative or missing dirs write nothing.
        assert!(Persist::new(Some(Path::new("rel"))).dir().is_none());
        assert!(Persist::new(Some(Path::new(""))).dir().is_none());
        Persist::new(None).flush(Duration::from_millis(1));
    }
}
