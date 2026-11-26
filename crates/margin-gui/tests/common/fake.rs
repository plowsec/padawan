//! Fake engine + fake opener: an in-memory folder answering every request deterministically.
//! Shared by the GUI tests and `examples/app_shot.rs`.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use margin_proto::link::local_pair;
use margin_proto::*;

#[derive(Clone, Debug)]
pub struct FakeFile {
    pub data: Vec<u8>,
    pub mtime: u64,
    pub readonly: bool,
}

#[derive(Default)]
pub struct FakeFs {
    pub files: BTreeMap<String, FakeFile>,
    pub dirs: BTreeSet<String>,
    pub clock: u64,
}

impl FakeFs {
    pub fn add(&mut self, path: &str, data: impl Into<Vec<u8>>) {
        self.clock += 1;
        let mut p = path;
        while let Some(i) = p.rfind('/') {
            p = &p[..i];
            self.dirs.insert(p.to_owned());
        }
        self.files.insert(path.to_owned(), FakeFile { data: data.into(), mtime: 1_000 + self.clock, readonly: false });
    }

    fn children(&self, dir: &str) -> Vec<DirEntry> {
        let pre = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        let direct = |p: &str| p.strip_prefix(pre.as_str()).filter(|r| !r.is_empty() && !r.contains('/')).map(str::to_owned);
        let mut dirs: Vec<DirEntry> = self.dirs.iter().filter_map(|d| direct(d)).map(|n| DirEntry { ignored: n == "target", name: n, kind: EntryKind::Dir, size: 0 }).collect();
        let mut files: Vec<DirEntry> = self
            .files
            .iter()
            .filter_map(|(p, f)| direct(p).map(|n| (n, f)))
            .map(|(n, f)| DirEntry { kind: if n.ends_with(".lnk") { EntryKind::Symlink } else if n.ends_with(".fifo") { EntryKind::Other } else { EntryKind::File }, ignored: n.ends_with(".log"), name: n, size: f.data.len() as u64 })
            .collect();
        dirs.sort_by_key(|e| e.name.to_lowercase());
        files.sort_by_key(|e| e.name.to_lowercase());
        dirs.extend(files);
        dirs
    }
}

/// Knobs for one engine.
#[derive(Clone, Default)]
pub struct Script {
    /// Send only `Fatal { msg }` at start.
    pub fatal_at_start: Option<String>,
    /// Replace real search results by these (streamed in batches of 5,000).
    pub search_override: Option<Arc<Vec<Site>>>,
    /// Replace `FindFiles` results.
    pub files_override: Option<Arc<Vec<FileMatch>>>,
    /// Directory listings to replace (path → entries).
    pub dir_override: HashMap<String, Arc<Vec<DirEntry>>>,
    /// Hold `Index` at `Scanning` (status bar shots).
    pub index_phase: Option<IndexStatus>,
    /// Send `Fatal { msg }` after answering this many requests.
    pub fatal_after: Option<(usize, String)>,
    /// `Paths` reply (default: none).
    pub paths_override: Option<Arc<Vec<CallPath>>>,
    pub paths_truncated: bool,
    /// `FindFiles` total to report (default: files in the fake folder).
    pub total_files: Option<u32>,
}

/// `*` (not `/`), `**` (anything), `?`. A pattern without `/` matches the file name.
pub fn glob(pat: &str, path: &str) -> bool {
    fn go(p: &[char], s: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('*') if p.get(1) == Some(&'*') => {
                let rest = if p.get(2) == Some(&'/') { &p[3..] } else { &p[2..] };
                (0..=s.len()).any(|i| go(rest, &s[i..]))
            }
            Some('*') => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != '/').any(|i| go(&p[1..], &s[i..])),
            Some('?') => !s.is_empty() && s[0] != '/' && go(&p[1..], &s[1..]),
            Some(c) => s.first() == Some(c) && go(&p[1..], &s[1..]),
        }
    }
    let pat = pat.trim();
    let target = if pat.contains('/') { path } else { path.rsplit('/').next().unwrap_or(path) };
    go(&pat.chars().collect::<Vec<_>>(), &target.chars().collect::<Vec<_>>())
}

/// Comma-separated globs: include (empty = all) and exclude (empty = none).
pub fn filtered(path: &str, include: &str, exclude: &str) -> bool {
    let list = |s: &str| s.split(',').map(str::trim).filter(|g| !g.is_empty()).map(str::to_owned).collect::<Vec<_>>();
    let inc = list(include);
    let exc = list(exclude);
    (inc.is_empty() || inc.iter().any(|g| glob(g, path))) && !exc.iter().any(|g| glob(g, path))
}

pub type SavedCall = (String, Vec<u8>, bool);

#[derive(Clone)]
pub struct Fake {
    pub fs: Arc<Mutex<FakeFs>>,
    pub log: Arc<Mutex<Vec<(ReqId, Request)>>>,
    pub script: Arc<Mutex<Script>>,
    pub opened: Arc<Mutex<Vec<String>>>,
    /// `Opener::new_window` calls.
    pub windows: Arc<Mutex<Vec<Option<PathBuf>>>>,
    /// `Opener::save` calls that wrote: `(path, data, replace)`.
    pub saved: Arc<Mutex<Vec<SavedCall>>>,
}

impl Fake {
    pub fn new(files: &[(&str, &str)]) -> Fake {
        let mut fs = FakeFs::default();
        for (p, d) in files {
            fs.add(p, d.as_bytes().to_vec());
        }
        Fake { fs: Arc::new(Mutex::new(fs)), log: Default::default(), script: Default::default(), opened: Default::default(), windows: Default::default(), saved: Default::default() }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap().iter().map(|(_, r)| r.clone()).collect()
    }

    pub fn writes(&self) -> Vec<(String, Vec<u8>, Option<u64>)> {
        self.requests()
            .into_iter()
            .filter_map(|r| match r {
                Request::WriteFile { path, data, expect_mtime_ns, .. } => Some((path.to_string(), data, expect_mtime_ns)),
                _ => None,
            })
            .collect()
    }

    pub fn mtime(&self, p: &str) -> u64 {
        self.fs.lock().unwrap().files[p].mtime
    }

    /// Engine on a thread.
    pub fn spawn(&self, root: &Path, waker: Waker) -> Client {
        let (client, rx, out) = local_pair(waker);
        let me = self.clone();
        let root_s = root.to_string_lossy().into_owned();
        self.opened.lock().unwrap().push(root_s.clone());
        std::thread::spawn(move || me.serve(root_s, rx, out));
        client
    }

    fn status(&self, phase: IndexPhase) -> IndexStatus {
        let n = self.fs.lock().unwrap().files.len() as u32;
        IndexStatus { phase, files_done: n, files_total: n, bytes: 0, symbols: 0, skipped: 0, from_cache: 0, elapsed_ms: 3 }
    }

    fn serve(self, root: String, rx: std::sync::mpsc::Receiver<(ReqId, Request)>, out: Outbox) {
        let script = self.script.lock().unwrap().clone();
        if let Some(msg) = script.fatal_at_start {
            out.event(Event::Fatal { msg });
            return;
        }
        let name = root.rsplit('/').find(|s| !s.is_empty()).unwrap_or("folder").to_owned();
        out.event(Event::Hello { protocol: PROTOCOL, root: root.clone(), name });
        out.event(Event::Index(self.status(IndexPhase::Scanning)));
        match script.index_phase {
            Some(s) => out.event(Event::Index(s)),
            None => out.event(Event::Index(self.status(IndexPhase::Ready))),
        }
        for (id, req) in rx {
            self.log.lock().unwrap().push((id, req.clone()));
            if req == Request::Shutdown {
                break;
            }
            let script = self.script.lock().unwrap().clone();
            self.answer(id, req, &out, &script);
            if let Some((n, msg)) = &script.fatal_after {
                if self.log.lock().unwrap().len() >= *n {
                    out.event(Event::Fatal { msg: msg.clone() });
                    break;
                }
            }
        }
    }

    fn answer(&self, id: ReqId, req: Request, out: &Outbox, script: &Script) {
        let err = |code, msg: &str| Reply::Err { code, msg: msg.to_owned() };
        let reply = match req.clone() {
            Request::ListDir { path } => match script.dir_override.get(path.as_str()) {
                Some(e) => Reply::Dir { entries: (**e).clone(), truncated: false },
                None => {
                    let fs = self.fs.lock().unwrap();
                    if path.is_root() || fs.dirs.contains(path.as_str()) {
                        Reply::Dir { entries: fs.children(path.as_str()), truncated: false }
                    } else {
                        err(ErrCode::NotFound, "no such directory")
                    }
                }
            },
            Request::ReadFile { path } => {
                let fs = self.fs.lock().unwrap();
                match fs.files.get(path.as_str()) {
                    Some(f) if f.data.len() > 8 << 20 => err(ErrCode::TooLarge, "too large"),
                    Some(f) => {
                        let binary = f.data.iter().take(8000).any(|&b| b == 0);
                        Reply::File { data: if binary { Vec::new() } else { f.data.clone() }, meta: Meta { kind: EntryKind::File, size: f.data.len() as u64, mtime_ns: f.mtime, readonly: f.readonly }, binary }
                    }
                    None => err(ErrCode::NotFound, "no such file"),
                }
            }
            Request::WriteFile { path, data, expect_mtime_ns, create_new } => {
                let mut fs = self.fs.lock().unwrap();
                let cur = fs.files.get(path.as_str()).cloned();
                match (cur, expect_mtime_ns) {
                    (Some(_), _) if create_new => err(ErrCode::Exists, "already exists"),
                    (Some(f), Some(m)) if f.mtime != m => err(ErrCode::Conflict, "changed on disk"),
                    _ => {
                        fs.add(path.as_str(), data.clone());
                        let f = &fs.files[path.as_str()];
                        Reply::Meta(Meta { kind: EntryKind::File, size: f.data.len() as u64, mtime_ns: f.mtime, readonly: false })
                    }
                }
            }
            Request::CreateDir { path } => {
                let mut fs = self.fs.lock().unwrap();
                if fs.files.contains_key(path.as_str()) {
                    err(ErrCode::Exists, "a file with that name exists")
                } else if fs.dirs.insert(path.to_string()) {
                    Reply::Done
                } else {
                    err(ErrCode::Exists, "already exists")
                }
            }
            Request::Rename { from, to } => {
                let mut fs = self.fs.lock().unwrap();
                match fs.files.remove(from.as_str()) {
                    Some(f) => {
                        fs.files.insert(to.to_string(), f);
                        Reply::Done
                    }
                    None => err(ErrCode::NotFound, "no such file"),
                }
            }
            Request::Delete { path } => {
                let mut fs = self.fs.lock().unwrap();
                let pre = format!("{path}/");
                if fs.files.keys().any(|k| k.starts_with(&pre)) {
                    err(ErrCode::Invalid, "directory not empty")
                } else if fs.files.remove(path.as_str()).is_some() || fs.dirs.remove(path.as_str()) {
                    Reply::Done
                } else {
                    err(ErrCode::NotFound, "no such file")
                }
            }
            Request::Stat { path } => {
                let fs = self.fs.lock().unwrap();
                match fs.files.get(path.as_str()) {
                    Some(f) => Reply::Meta(Meta { kind: EntryKind::File, size: f.data.len() as u64, mtime_ns: f.mtime, readonly: f.readonly }),
                    None => err(ErrCode::NotFound, "no such file"),
                }
            }
            Request::FindFiles { query, limit, include, exclude } => match &script.files_override {
                Some(v) => Reply::FileMatches { items: (**v).clone(), total_files: script.total_files.unwrap_or(v.len() as u32) },
                None => {
                    let fs = self.fs.lock().unwrap();
                    let q: Vec<char> = query.to_lowercase().chars().collect();
                    let is_glob = query.contains(['*', '?']);
                    let mut items = Vec::new();
                    for p in fs.files.keys() {
                        if !filtered(p, &include, &exclude) {
                            continue;
                        }
                        if is_glob {
                            if glob(&query, p) {
                                items.push(FileMatch { path: RelPath::new(p).unwrap(), score: -(p.len() as i32), positions: Vec::new() });
                            }
                            continue;
                        }
                        let mut pos = Vec::new();
                        let mut qi = 0;
                        for (i, c) in p.char_indices() {
                            if qi < q.len() && c.to_lowercase().next() == Some(q[qi]) {
                                pos.push(i as u32);
                                qi += 1;
                            }
                        }
                        if qi == q.len() {
                            items.push(FileMatch { path: RelPath::new(p).unwrap(), score: -(p.len() as i32), positions: pos });
                        }
                    }
                    items.sort_by_key(|m| -m.score);
                    items.truncate(limit as usize);
                    Reply::FileMatches { items, total_files: script.total_files.unwrap_or(fs.files.len() as u32) }
                }
            },
            Request::Search { query } => {
                let hits: Vec<Site> = match &script.search_override {
                    Some(v) => (**v).clone(),
                    None => {
                        if query.pattern.contains('(') && query.regex {
                            out.reply(id, err(ErrCode::Invalid, "regex parse error: unclosed group"));
                            return;
                        }
                        let fs = self.fs.lock().unwrap();
                        let mut v = Vec::new();
                        for (p, f) in &fs.files {
                            let Ok(text) = std::str::from_utf8(&f.data) else { continue };
                            if !filtered(p, &query.include, &query.exclude) {
                                continue;
                            }
                            for (ln, line) in text.lines().enumerate() {
                                let hay = if query.case_sensitive { line.to_owned() } else { line.to_lowercase() };
                                let needle = if query.case_sensitive { query.pattern.clone() } else { query.pattern.to_lowercase() };
                                if let Some(c) = hay.find(&needle) {
                                    v.push(Site { path: RelPath::new(p).unwrap(), line: ln as u32, col: c as u32, len: needle.len() as u32, text: line.to_owned(), text_col: c as u32, context: String::new() });
                                }
                            }
                        }
                        // Reverse file order: the GUI must sort.
                        v.reverse();
                        v
                    }
                };
                let n = hits.len() as u32;
                let files: BTreeSet<&RelPath> = hits.iter().map(|h| &h.path).collect();
                let nf = files.len() as u32;
                for chunk in hits.chunks(5000) {
                    out.reply(id, Reply::SearchBatch { hits: chunk.to_vec() });
                }
                Reply::SearchDone(SearchStats { files_scanned: 10, files_matched: nf, hits: n, truncated: n >= query.max_hits, elapsed_ms: 12 })
            }
            Request::Cancel { .. } => return,
            Request::Lens { fns } => {
                let fs = self.fs.lock().unwrap();
                let infos = fns
                    .iter()
                    .map(|f| {
                        let needle = format!("{}(", f.name);
                        let mut n = 0u32;
                        for file in fs.files.values() {
                            if let Ok(t) = std::str::from_utf8(&file.data) {
                                n += t.matches(&needle).count() as u32;
                            }
                        }
                        LensInfo { callers: n.saturating_sub(1), defs: 1, callees: f.callees.len() as u32 / 2, reach: if f.callees.iter().any(|c| c.starts_with("run")) { vec!["system".into()] } else { vec![] }, reach_more: false }
                    })
                    .collect();
                Reply::Lens { fns: infos }
            }
            Request::Callers { name, .. } | Request::Defs { name, .. } | Request::Reach { name, .. } => {
                let defs = matches!(req, Request::Defs { .. });
                let fs = self.fs.lock().unwrap();
                let mut sites = Vec::new();
                let call = format!("{name}(");
                for (p, f) in &fs.files {
                    let Ok(t) = std::str::from_utf8(&f.data) else { continue };
                    for (ln, line) in t.lines().enumerate() {
                        let Some(c) = line.find(&call) else { continue };
                        let is_def = ["fn ", "def ", "int ", "void ", "static "].iter().any(|k| line.trim_start().starts_with(k));
                        if is_def == defs {
                            sites.push(Site { path: RelPath::new(p).unwrap(), line: ln as u32, col: c as u32, len: name.len() as u32, text: line.to_owned(), text_col: c as u32, context: if matches!(req, Request::Reach { .. }) { format!("{name} → helper → system") } else { "main".into() } });
                        }
                    }
                }
                Reply::Sites { sites, truncated: false }
            }
            Request::Reindex { .. } => {
                out.event(Event::Index(self.status(IndexPhase::Ready)));
                Reply::Done
            }
            Request::Paths { .. } => Reply::Paths { paths: script.paths_override.as_ref().map(|v| (**v).clone()).unwrap_or_default(), truncated: script.paths_truncated },
            Request::Replace { query, replacement, only, skip } => {
                // Literal, line by line (like the fake search).
                let mut fs = self.fs.lock().unwrap();
                let (mut files, mut n, mut changed, mut skipped) = (0u32, 0u32, Vec::new(), Vec::new());
                let paths: Vec<String> = fs.files.keys().cloned().collect();
                for p in paths {
                    let rp = RelPath::new(&p).unwrap();
                    if (!only.is_empty() && !only.contains(&rp)) || !filtered(&p, &query.include, &query.exclude) {
                        continue;
                    }
                    let Ok(text) = String::from_utf8(fs.files[&p].data.clone()) else { continue };
                    let needle = if query.case_sensitive { query.pattern.clone() } else { query.pattern.to_lowercase() };
                    let hay = if query.case_sensitive { text.clone() } else { text.to_lowercase() };
                    let count = hay.matches(&needle).count() as u32;
                    if count == 0 || needle.is_empty() {
                        continue;
                    }
                    if skip.contains(&rp) {
                        skipped.push(Skipped { path: rp, why: "open with unsaved edits".into() });
                        continue;
                    }
                    let mut out_s = String::new();
                    let mut last = 0;
                    for (i, _) in hay.match_indices(&needle) {
                        out_s.push_str(&text[last..i]);
                        out_s.push_str(&replacement);
                        last = i + needle.len();
                    }
                    out_s.push_str(&text[last..]);
                    fs.add(&p, out_s.into_bytes());
                    files += 1;
                    n += count;
                    changed.push(rp);
                }
                Reply::Replaced { files, replacements: n, changed, skipped, cancelled: false }
            }
            Request::Shutdown => return,
        };
        out.reply(id, reply);
    }
}

pub struct FakeOpener {
    pub fake: Fake,
    pub sandbox: String,
    pub listings: HashMap<String, DirListing>,
    queue: VecDeque<DirListing>,
    saved: VecDeque<SavedFile>,
}

impl FakeOpener {
    pub fn new(fake: Fake) -> FakeOpener {
        let mut listings = HashMap::new();
        let dirs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        listings.insert("/home/me".into(), DirListing { path: "/home/me".into(), parent: Some("/home".into()), dirs: dirs(&["Documents", "Downloads", "dev", "notes"]), error: String::new() });
        listings.insert("/home/me/dev".into(), DirListing { path: "/home/me/dev".into(), parent: Some("/home/me".into()), dirs: dirs(&["margin", "parser-c", "website"]), error: String::new() });
        FakeOpener { fake, sandbox: "seatbelt".into(), listings, queue: VecDeque::new(), saved: VecDeque::new() }
    }
}

impl Opener for FakeOpener {
    fn open(&mut self, root: &Path, _opts: &IndexOpts, waker: Waker) -> Client {
        self.fake.spawn(root, waker)
    }

    fn sandbox(&self) -> &str {
        &self.sandbox
    }

    fn browse(&mut self, path: Option<&Path>, waker: Waker) {
        let key = path.map_or("/home/me".to_owned(), |p| p.to_string_lossy().into_owned());
        let l = self.listings.get(&key).cloned().unwrap_or(DirListing { path: key.clone(), parent: Some("/home/me".into()), dirs: vec![], error: "No such directory".into() });
        self.queue.push_back(l);
        waker();
    }

    fn poll_browse(&mut self) -> Option<DirListing> {
        self.queue.pop_front()
    }

    /// The launcher's rules in short: `~` is `/home/me`, `~/.ssh` is refused, a file is new or was written here.
    fn save(&mut self, id: u32, path: &str, data: Vec<u8>, replace: bool, waker: Waker) {
        let abs = path.strip_prefix("~/").map_or(path.to_owned(), |rest| format!("/home/me/{rest}"));
        let name = abs.rsplit('/').next().unwrap_or_default().to_owned();
        let mut saved = self.fake.saved.lock().unwrap();
        let mine = saved.iter().any(|s| s.0 == abs);
        let error = if abs.starts_with("/home/me/.ssh/") {
            "refusing to write in /home/me/.ssh: it holds credentials or start-up items (~/.ssh)".to_owned()
        } else if abs == "/home/me/notes/theirs.md" || (mine && !replace) {
            format!("{name} already exists")
        } else {
            String::new()
        };
        if error.is_empty() {
            saved.push((abs.clone(), data, replace));
            self.saved.push_back(SavedFile { id, path: abs, error });
        } else {
            self.saved.push_back(SavedFile { id, path: String::new(), error });
        }
        waker();
    }

    fn poll_saved(&mut self) -> Option<SavedFile> {
        self.saved.pop_front()
    }

    fn new_window(&mut self, folder: Option<&Path>) {
        self.fake.windows.lock().unwrap().push(folder.map(Path::to_path_buf));
    }
}
