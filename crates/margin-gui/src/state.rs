//! `state.conf` (what the app remembers) and the background writer for both config files.
//!
//! | Key | Meaning |
//! |---|---|
//! | `recent = <abs path>` | recent folders, most recent first, max `MAX_RECENT` |
//! | `sidebar_width = <pt>` / `sidebar = on/off` | sidebar |
//! | `folder = <abs path>` | starts a folder block; the `tab` / `active` lines after it belong to it |
//! | `tab = <rel path>` / `active = <rel path>` | open tabs of that folder, in order |
//! | `group = <n>` | the `tab` / `active` lines after it belong to the next editor group |
//! | `layout = r(0:500,d(1:500,2:500):500)` / `focus = <n>` | how the groups are split (`tabs::Node::write`), the focused one |
//!
//! Paths with control characters are never written (they would break the line format).
//! Writes: temp file + rename, on the writer thread; only `settings.conf`, `state.conf` (+ `.tmp`) in an absolute config dir,
//! and hot-exit buffers `untitled/untitled-<n>.txt` (+ `.tmp`). The only delete: one `untitled-<n>.txt` by name.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use crate::settings::parse_conf;

pub const MAX_RECENT: usize = 12;
const MAX_FOLDERS: usize = 32;
const MAX_TABS: usize = 64;
/// Editor groups saved per folder (`tabs::MAX_GROUPS`).
const MAX_GROUPS: usize = crate::tabs::MAX_GROUPS;
pub const SIDEBAR_W: std::ops::RangeInclusive<f32> = 160.0..=640.0;
/// Hot-exit buffers kept on disk at most.
pub const MAX_UNTITLED: usize = 32;
/// Bytes of one hot-exit buffer at most.
pub const MAX_UNTITLED_BYTES: usize = 4 << 20;
const UNTITLED_DIR: &str = "untitled";

/// Tabs of one editor group.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GroupTabs {
    pub tabs: Vec<String>,
    pub active: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FolderTabs {
    pub root: String,
    /// First group.
    pub tabs: Vec<String>,
    pub active: Option<String>,
    /// The other groups.
    pub more: Vec<GroupTabs>,
    /// `tabs::Node::write` over group indices (0 = first group); empty: one group / a row.
    pub layout: String,
    /// Index of the focused group.
    pub focus: usize,
}

impl FolderTabs {
    fn count(&self) -> usize {
        self.tabs.len() + self.more.iter().map(|g| g.tabs.len()).sum::<usize>()
    }
}

fn layout_safe(s: &str) -> bool {
    !s.is_empty() && s.len() <= 512 && s.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'r' | b'd' | b'(' | b')' | b',' | b':'))
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
                "group" => {
                    if let Some(f) = cur.as_mut() {
                        if f.more.len() + 1 < MAX_GROUPS {
                            f.more.push(GroupTabs::default());
                        }
                    }
                }
                "tab" => {
                    if let Some(f) = cur.as_mut() {
                        let room = f.count() < MAX_TABS;
                        let tabs = match f.more.last_mut() {
                            Some(g) => &mut g.tabs,
                            None => &mut f.tabs,
                        };
                        if room && margin_proto::RelPath::new(&v).is_ok() && !v.is_empty() && !tabs.contains(&v) {
                            tabs.push(v);
                        }
                    }
                }
                "active" => {
                    if let Some(f) = cur.as_mut() {
                        let (tabs, active) = match f.more.last_mut() {
                            Some(g) => (&g.tabs, &mut g.active),
                            None => (&f.tabs, &mut f.active),
                        };
                        if tabs.contains(&v) {
                            *active = Some(v);
                        }
                    }
                }
                "layout" => {
                    if let Some(f) = cur.as_mut() {
                        if layout_safe(&v) {
                            f.layout = v;
                        }
                    }
                }
                "focus" => {
                    if let Some(f) = cur.as_mut() {
                        f.focus = v.parse::<usize>().ok().filter(|n| *n < MAX_GROUPS).unwrap_or(0);
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

    fn push_folder(&mut self, mut f: FolderTabs) {
        if f.focus > f.more.len() {
            f.focus = 0;
        }
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
            let mut left = MAX_TABS;
            let mut group = |out: &mut String, tabs: &[String], active: &Option<String>| {
                for t in tabs.iter().filter(|t| line_safe(t)).take(left) {
                    *out += &format!("tab = {t}\n");
                    left -= 1;
                }
                if let Some(a) = active.as_ref().filter(|a| line_safe(a)) {
                    *out += &format!("active = {a}\n");
                }
            };
            group(&mut out, &f.tabs, &f.active);
            for (i, g) in f.more.iter().take(MAX_GROUPS - 1).enumerate() {
                out += &format!("group = {}\n", i + 1);
                group(&mut out, &g.tabs, &g.active);
            }
            if layout_safe(&f.layout) {
                out += &format!("layout = {}\n", f.layout);
            }
            if f.focus > 0 {
                out += &format!("focus = {}\n", f.focus);
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
    WriteUntitled(u32, String),
    RemoveUntitled(u32),
    Flush(Sender<()>),
}

/// File name of hot-exit buffer `n`.
pub fn untitled_name(n: u32) -> String {
    format!("untitled-{n}.txt")
}

/// `untitled-<n>.txt` → n.
pub fn untitled_number(name: &str) -> Option<u32> {
    let d = name.strip_prefix("untitled-")?.strip_suffix(".txt")?;
    (!d.is_empty() && d.len() <= 9 && d.bytes().all(|b| b.is_ascii_digit())).then(|| d.parse().ok()).flatten()
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

    /// Write hot-exit buffer `n` (caller checks `MAX_UNTITLED_BYTES`).
    pub fn save_untitled(&self, n: u32, text: String) {
        self.send(Job::WriteUntitled(n, text));
    }

    /// Unlink hot-exit buffer `n` (if there).
    pub fn remove_untitled(&self, n: u32) {
        self.send(Job::RemoveUntitled(n));
    }

    /// Numbers of the hot-exit buffers on disk, sorted (start-up only).
    pub fn untitled_numbers(&self) -> Vec<u32> {
        let Some(d) = &self.dir else { return Vec::new() };
        let mut v: Vec<u32> = std::fs::read_dir(d.join(UNTITLED_DIR))
            .map(|it| it.filter_map(Result::ok).take(10_000).filter_map(|e| e.file_name().to_str().and_then(untitled_number)).collect())
            .unwrap_or_default();
        v.sort_unstable();
        v
    }

    /// Hot-exit buffers on disk: `(n, text, was valid UTF-8)`, first `MAX_UNTITLED` by number, each read capped
    /// (start-up only, before the first frame).
    pub fn load_untitled(&self) -> Vec<(u32, String, bool)> {
        let Some(d) = &self.dir else { return Vec::new() };
        use std::io::Read;
        let mut out = Vec::new();
        for n in self.untitled_numbers().into_iter().take(MAX_UNTITLED) {
            let mut buf = Vec::new();
            if let Ok(f) = std::fs::File::open(d.join(UNTITLED_DIR).join(untitled_name(n))) {
                let _ = f.take(MAX_UNTITLED_BYTES as u64).read_to_end(&mut buf);
            }
            let (text, ok) = match String::from_utf8(buf) {
                Ok(s) => (s, true),
                Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), false),
            };
            out.push((n, text, ok));
        }
        out
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
            Job::WriteUntitled(n, text) => {
                let ud = dir.join(UNTITLED_DIR);
                if !ud.is_dir() {
                    let _ = std::fs::create_dir(&ud);
                }
                let name = untitled_name(n);
                let tmp = ud.join(format!("{name}.tmp"));
                let dst = ud.join(&name);
                if std::fs::write(&tmp, text.as_bytes()).is_ok() && std::fs::rename(&tmp, &dst).is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
            }
            Job::RemoveUntitled(n) => {
                let ud = dir.join(UNTITLED_DIR);
                let name = untitled_name(n);
                let f = ud.join(&name);
                // Exactly that file: absolute, directly inside `<config>/untitled`, the expected name.
                let ok = f.is_absolute() && f.parent() == Some(ud.as_path()) && f.file_name().and_then(|x| x.to_str()).and_then(untitled_number) == Some(n);
                if ok && f.is_file() {
                    let _ = std::fs::remove_file(&f);
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
        s.set_tabs(FolderTabs { root: "/a".into(), tabs: vec!["x.rs".into(), "d/y.md".into()], active: Some("d/y.md".into()), ..Default::default() });
        s.set_tabs(FolderTabs { root: "/b c".into(), ..Default::default() });
        assert_eq!(SavedState::parse(&s.write()), s);
        // Groups: the same file may sit in two of them.
        let g = |tabs: &[&str], active: &str| GroupTabs { tabs: tabs.iter().map(|t| (*t).to_owned()).collect(), active: Some(active.to_owned()) };
        s.set_tabs(FolderTabs { root: "/g".into(), tabs: vec!["x.rs".into()], active: Some("x.rs".into()), more: vec![g(&["x.rs", "z.c"], "z.c"), GroupTabs::default()], layout: "r(0:600,d(1:500,2:500):400)".into(), focus: 1 });
        let text = s.write();
        assert!(text.contains("group = 1\ntab = x.rs\ntab = z.c\nactive = z.c\ngroup = 2\nlayout = r(0:600,d(1:500,2:500):400)\nfocus = 1\n"), "{text}");
        assert_eq!(SavedState::parse(&text), s);
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
        // Groups: capped, a bad layout or focus is dropped, tabs are capped over all groups.
        let mut text = String::from("folder = /g\nlayout = r(0:1,$(1:1))\nfocus = 99\ntab = a\n");
        for i in 0..20 {
            text += &format!("group = x\ntab = a\ntab = t{i}\nactive = nope\n");
        }
        for i in 0..100 {
            text += &format!("tab = u{i}\n");
        }
        let s = SavedState::parse(&text);
        let f = &s.folders[0];
        assert_eq!((f.more.len(), f.layout.as_str(), f.focus), (MAX_GROUPS - 1, "", 0));
        assert_eq!(f.more[0].tabs, vec!["a", "t0"]);
        assert_eq!(f.count(), MAX_TABS);
        assert_eq!(SavedState::parse("folder = /g\ntab = a\nfocus = 3\n").folders[0].focus, 0, "no such group");
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

    #[test]
    fn untitled_buffers_round_trip_and_unlink() {
        let t = margin_testutil::TempDir::new("gui-untitled");
        let p = Persist::new(Some(t.path()));
        p.save_untitled(3, "three".into());
        p.save_untitled(1, "one".into());
        p.flush(Duration::from_secs(5));
        t.write("untitled/notes.txt", "not ours");
        t.write("untitled/untitled-7.txt", "seven");
        std::fs::write(t.path().join("untitled/untitled-9.txt"), b"ok\xff\xfe").unwrap();
        let got = p.load_untitled();
        assert_eq!(got.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1, 3, 7, 9]);
        assert_eq!(got[0].1, "one");
        assert!(!got[3].2, "lossy");
        p.remove_untitled(3);
        p.remove_untitled(42);
        p.flush(Duration::from_secs(5));
        assert!(!t.path().join("untitled/untitled-3.txt").exists());
        assert!(t.path().join("untitled/notes.txt").exists(), "foreign files stay");
        assert_eq!(untitled_number("untitled-12.txt"), Some(12));
        for bad in ["untitled-.txt", "untitled-1.txt.tmp", "untitled--1.txt", "untitled-1234567890.txt", "x-1.txt"] {
            assert_eq!(untitled_number(bad), None, "{bad}");
        }
    }
}
