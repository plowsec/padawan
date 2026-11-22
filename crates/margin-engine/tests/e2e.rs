//! End to end: `run` on a joined thread, `spawn_local`, and `server_over_io` with in-memory pipes.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use margin_engine::EngineConfig;
use margin_proto::link::{client_over_io, server_over_io};
use margin_proto::{
    Client, EntryKind, ErrCode, Event, IndexOpts, IndexPhase, IndexStatus, LensFn, Out, RelPath, Reply, ReqId, Request, SearchQuery, SearchStats, Site,
};
use margin_testutil::TempDir;

const TIMEOUT: Duration = Duration::from_secs(20);

struct H {
    c: Client,
    buf: VecDeque<Out>,
    events: Vec<Event>,
}

impl H {
    fn new(c: Client) -> Self {
        Self { c, buf: VecDeque::new(), events: Vec::new() }
    }

    fn pump(&mut self) {
        let t = Instant::now();
        loop {
            if let Some(o) = self.c.poll() {
                if let Out::Event(e) = &o {
                    self.events.push(e.clone());
                }
                self.buf.push_back(o);
                return;
            }
            assert!(t.elapsed() < TIMEOUT, "engine timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn hello(&mut self) -> Event {
        self.pump();
        match self.buf.pop_front() {
            Some(Out::Event(e)) => e,
            o => panic!("expected an event first, got {o:?}"),
        }
    }

    /// Waits for a `Ready` that came after `after` previous Ready events.
    fn ready(&mut self, after: usize) -> IndexStatus {
        loop {
            let ready: Vec<IndexStatus> = self.events.iter().filter_map(|e| if let Event::Index(s) = e { (s.phase == IndexPhase::Ready).then_some(*s) } else { None }).collect();
            if ready.len() > after {
                return ready[after];
            }
            self.pump();
        }
    }

    fn reply(&mut self, id: ReqId) -> Reply {
        loop {
            if let Some(i) = self.buf.iter().position(|o| matches!(o, Out::Reply(r, _) if *r == id)) {
                if let Some(Out::Reply(_, r)) = self.buf.remove(i) {
                    return r;
                }
            }
            self.pump();
        }
    }

    fn call(&mut self, req: Request) -> Reply {
        let id = self.c.request(req);
        self.reply(id)
    }

    fn search(&mut self, q: SearchQuery) -> Result<(Vec<Site>, SearchStats), ErrCode> {
        let id = self.c.request(Request::Search { query: q });
        let mut hits = Vec::new();
        loop {
            match self.reply(id) {
                Reply::SearchBatch { hits: h } => hits.extend(h),
                Reply::SearchDone(s) => return Ok((hits, s)),
                Reply::Err { code, .. } => return Err(code),
                r => panic!("unexpected {r:?}"),
            }
        }
    }
}

/// Engine on a thread we can join (so it is done writing its cache before the `TempDir`s go).
fn start(cfg: EngineConfig) -> (H, std::thread::JoinHandle<()>) {
    let (client, rx, out) = margin_proto::link::local_pair(Arc::new(|| {}));
    let j = std::thread::spawn(move || margin_engine::run(cfg, rx, out));
    (H::new(client), j)
}

fn stop(h: H, j: std::thread::JoinHandle<()>) {
    drop(h);
    j.join().unwrap();
}

fn p(s: &str) -> RelPath {
    RelPath::new(s).unwrap()
}

fn q(pat: &str) -> SearchQuery {
    SearchQuery { pattern: pat.into(), max_hits: 1000, ..Default::default() }
}

fn fixture() -> TempDir {
    let t = TempDir::new("e2e");
    t.write(".gitignore", "*.log\n");
    t.write("src/main.rs", "fn main() {\n    run_cmd();\n}\n");
    t.write("src/lib.rs", "pub fn helper() {}\n");
    t.write("notes.md", "alpha beta\n");
    t.write("debug.log", "alpha in a log\n");
    t.mkdir("zdir");
    t
}

fn cfg(root: &std::path::Path, cache: Option<&std::path::Path>) -> EngineConfig {
    EngineConfig { root: root.to_path_buf(), cache_dir: cache.map(|c| c.join("cache")), opts: IndexOpts::default(), threads: 4 }
}

fn flow(h: &mut H, t: &TempDir) {
    match h.hello() {
        Event::Hello { protocol, root, name } => {
            assert_eq!(protocol, margin_proto::PROTOCOL);
            assert_eq!(std::path::PathBuf::from(&root), std::fs::canonicalize(t.path()).unwrap());
            assert!(root.ends_with(&name));
        }
        e => panic!("{e:?}"),
    }
    let st = h.ready(0);
    assert_eq!(st.files_total, 4, "{st:?}");

    let Reply::Dir { entries, truncated } = h.call(Request::ListDir { path: RelPath::root() }) else { panic!() };
    assert!(!truncated);
    let names: Vec<(&str, EntryKind, bool)> = entries.iter().map(|e| (e.name.as_str(), e.kind, e.ignored)).collect();
    assert_eq!(
        names,
        vec![
            ("src", EntryKind::Dir, false),
            ("zdir", EntryKind::Dir, false),
            (".gitignore", EntryKind::File, false),
            ("debug.log", EntryKind::File, true),
            ("notes.md", EntryKind::File, false)
        ]
    );

    let Reply::File { data, meta, binary } = h.call(Request::ReadFile { path: p("notes.md") }) else { panic!() };
    assert_eq!((data.as_slice(), binary), (&b"alpha beta\n"[..], false));

    let (v, _) = h.search(q("alpha")).unwrap();
    assert_eq!(v.len(), 1, "ignored file is not searched");
    assert!(h.search(q("gamma")).unwrap().0.is_empty());

    // Save → search sees it at once.
    let r = h.call(Request::WriteFile { path: p("notes.md"), data: b"alpha gamma\n".to_vec(), expect_mtime_ns: Some(meta.mtime_ns), create_new: false });
    assert!(matches!(r, Reply::Meta(_)), "{r:?}");
    let (v, _) = h.search(q("gamma")).unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!((v[0].path.as_str(), v[0].col), ("notes.md", 6));
    let r = h.call(Request::WriteFile { path: p("notes.md"), data: b"stale".to_vec(), expect_mtime_ns: Some(meta.mtime_ns), create_new: false });
    assert!(matches!(r, Reply::Err { code: ErrCode::Conflict, .. }), "{r:?}");

    // New file, rename, delete: the finder follows.
    h.call(Request::WriteFile { path: p("zdir/fresh_needle.txt"), data: b"delta".to_vec(), expect_mtime_ns: None, create_new: true });
    let Reply::FileMatches { items, total_files } = h.call(Request::FindFiles { query: "frneedle".into(), include: String::new(), exclude: String::new(), limit: 10 }) else { panic!() };
    assert_eq!(total_files, 5);
    assert_eq!(items[0].path.as_str(), "zdir/fresh_needle.txt");
    assert!(matches!(h.call(Request::Rename { from: p("zdir/fresh_needle.txt"), to: p("moved.txt") }), Reply::Done));
    assert_eq!(h.search(q("delta")).unwrap().0[0].path.as_str(), "moved.txt");
    assert!(matches!(h.call(Request::Delete { path: p("moved.txt") }), Reply::Done));
    assert!(h.search(q("delta")).unwrap().0.is_empty());
    assert!(matches!(h.call(Request::Delete { path: p("src") }), Reply::Err { code: ErrCode::Invalid, .. }));
    assert!(matches!(h.call(Request::CreateDir { path: p("zdir/sub") }), Reply::Done));
    assert!(matches!(h.call(Request::Stat { path: p("zdir/sub") }), Reply::Meta(m) if m.kind == EntryKind::Dir));

    let Reply::FileMatches { items, .. } = h.call(Request::FindFiles { query: "main".into(), include: String::new(), exclude: String::new(), limit: 10 }) else { panic!() };
    assert_eq!(items[0].path.as_str(), "src/main.rs");
    let Reply::Lens { fns } = h.call(Request::Lens { fns: vec![LensFn { name: "main".into(), callees: vec!["run_cmd".into()] }] }) else { panic!() };
    assert_eq!(fns.len(), 1);
    assert!(matches!(h.call(Request::Callers { name: "x".into(), limit: 5 }), Reply::Sites { .. }));
    assert!(matches!(h.call(Request::Defs { name: "x".into(), limit: 5 }), Reply::Sites { .. }));
    assert!(matches!(h.call(Request::Reach { name: "x".into(), limit: 5 }), Reply::Sites { .. }));
    assert_eq!(h.search(SearchQuery { regex: true, ..q("(") }).unwrap_err(), ErrCode::Invalid);
    assert!(matches!(h.call(Request::ReadFile { path: p("missing") }), Reply::Err { code: ErrCode::NotFound, .. }));

    // Changed on disk → Reindex picks it up.
    t.write("src/lib.rs", "pub fn helper() { epsilon_on_disk(); }\n");
    t.write("added.txt", "epsilon_on_disk too\n");
    assert!(matches!(h.call(Request::Reindex { opts: IndexOpts::default() }), Reply::Done));
    let st = h.ready(1);
    assert_eq!(st.files_total, 5);
    assert_eq!(h.search(q("epsilon_on_disk")).unwrap().0.len(), 2);
    // Options change: gitignore off → the log file is searched.
    h.call(Request::Reindex { opts: IndexOpts { respect_gitignore: false, ..Default::default() } });
    h.ready(2);
    assert_eq!(h.search(q("alpha")).unwrap().0.len(), 2);
}

#[test]
fn local_engine_flow_and_warm_cache() {
    let t = fixture();
    let c = TempDir::new("e2e-cache");
    let (mut h, j) = start(cfg(t.path(), Some(c.path())));
    flow(&mut h, &t);
    h.c.request(Request::Shutdown);
    stop(h, j);
    assert!(c.path().join("cache/index.bin").is_file());

    let warm = EngineConfig { opts: IndexOpts { respect_gitignore: false, ..Default::default() }, ..cfg(t.path(), Some(c.path())) };
    let (mut h, j) = start(warm);
    h.hello();
    let st = h.ready(0);
    assert_eq!(st.from_cache, st.files_total, "{st:?}");
    assert!(st.files_total >= 6);
    assert_eq!(h.search(q("epsilon_on_disk")).unwrap().0.len(), 2);
    stop(h, j);
    let names: Vec<_> = std::fs::read_dir(c.path().join("cache")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, vec![std::ffi::OsString::from("index.bin")], "no temp files left");
}

struct PipeR(Receiver<Vec<u8>>, Vec<u8>);
struct PipeW(Sender<Vec<u8>>);

impl Read for PipeR {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.1.is_empty() {
            match self.0.recv() {
                Ok(v) => self.1 = v,
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.1.len());
        buf[..n].copy_from_slice(&self.1[..n]);
        self.1.drain(..n);
        Ok(n)
    }
}

impl Write for PipeW {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.send(buf.to_vec()).map_err(|_| std::io::ErrorKind::BrokenPipe)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn pipe() -> (PipeR, PipeW) {
    let (tx, rx) = channel();
    (PipeR(rx, Vec::new()), PipeW(tx))
}

#[test]
fn engine_over_pipes() {
    let t = fixture();
    let (c2s_r, c2s_w) = pipe();
    let (s2c_r, s2c_w) = pipe();
    let client = client_over_io(s2c_r, c2s_w, Arc::new(|| {}));
    let (rx, out) = server_over_io(c2s_r, s2c_w);
    let c = cfg(t.path(), None);
    let engine = std::thread::spawn(move || margin_engine::run(c, rx, out));
    let mut h = H::new(client);
    flow(&mut h, &t);
    h.c.request(Request::Shutdown);
    drop(h);
    engine.join().unwrap();
}

#[test]
fn cancel_and_fatal() {
    let t = TempDir::new("e2e-cancel");
    for i in 0..300 {
        t.write(&format!("f{i}.txt"), "zz ".repeat(20_000));
    }
    let (mut h, j) = start(cfg(t.path(), None));
    h.hello();
    h.ready(0);
    let id = h.c.request(Request::Search { query: SearchQuery { regex: true, ..q("z+") } });
    h.c.cancel(id);
    h.c.cancel(9999);
    loop {
        match h.reply(id) {
            Reply::SearchBatch { .. } => {}
            Reply::SearchDone(_) | Reply::Err { code: ErrCode::Cancelled, .. } => break,
            r => panic!("{r:?}"),
        }
    }

    stop(h, j);

    // `spawn_local`: a folder that does not exist → `Fatal` first.
    let gone = t.path().join("does-not-exist");
    let mut h = H::new(margin_engine::spawn_local(cfg(&gone, None), Arc::new(|| {})));
    assert!(matches!(h.hello(), Event::Fatal { .. }));
}

/// Outlines come from margin-code (built in parallel); here they are planted through the cache, so the
/// graph queries run against hand-built outlines with the real line reads.
#[test]
fn graph_queries_over_cached_outlines() {
    use margin_engine::index::{CallSum, FileRec, FnSum};
    let t = TempDir::new("e2e-graph");
    let a = "fn handler() {\n    run_cmd(x);\n}\nfn run_cmd() {\n    system(cmd);\n}\n";
    let b = "fn main() {\n    handler();\n    handler();\n}\n";
    t.write("a.rs", a);
    t.write("b.rs", b);
    let root = std::fs::canonicalize(t.path()).unwrap();
    let c = TempDir::new("e2e-graph-cache");
    let rec = |name: &str, text: &str, fns: Vec<FnSum>| {
        let md = std::fs::metadata(root.join(name)).unwrap();
        let mt = md.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
        Arc::new(FileRec { path: name.into(), size: md.len(), mtime_ns: mt, flags: 0, bloom: margin_engine::bloom::build(text.as_bytes()), fns: fns.into() })
    };
    let call = |n: &str, line, col, sink| CallSum { name: n.into(), line, col, sink };
    let f = |n: &str, line, calls: Vec<CallSum>| FnSum { name: n.into(), line, col: 3, calls: calls.into() };
    let recs = vec![
        rec("a.rs", a, vec![f("handler", 0, vec![call("run_cmd", 1, 4, 0)]), f("run_cmd", 3, vec![call("system", 4, 4, 1)])]),
        rec("b.rs", b, vec![f("main", 0, vec![call("handler", 1, 4, 0), call("handler", 2, 4, 0)])]),
    ];
    margin_engine::cache::save(&c.path().join("cache"), &root.to_string_lossy(), &recs).unwrap();
    let (mut h, j) = start(cfg(t.path(), Some(c.path())));
    h.hello();
    let st = h.ready(0);
    assert_eq!((st.from_cache, st.symbols), (2, 3), "{st:?}");

    let Reply::Lens { fns } = h.call(Request::Lens { fns: vec![LensFn { name: "handler".into(), callees: vec!["run_cmd".into()] }] }) else { panic!() };
    assert_eq!((fns[0].callers, fns[0].defs, fns[0].callees, fns[0].reach.clone()), (2, 1, 1, vec!["system".to_owned()]));

    let Reply::Sites { sites, truncated } = h.call(Request::Callers { name: "handler".into(), limit: 0 }) else { panic!() };
    assert!(!truncated);
    assert_eq!(sites.len(), 2);
    assert_eq!((sites[0].path.as_str(), sites[0].line, sites[0].text.as_str(), sites[0].context.as_str()), ("b.rs", 1, "    handler();", "main"));
    assert_eq!(&sites[0].text[sites[0].text_col as usize..][..sites[0].len as usize], "handler");
    let Reply::Sites { sites, truncated } = h.call(Request::Callers { name: "handler".into(), limit: 1 }) else { panic!() };
    assert!(truncated && sites.len() == 1);

    let Reply::Sites { sites, .. } = h.call(Request::Defs { name: "run_cmd".into(), limit: 10 }) else { panic!() };
    assert_eq!((sites[0].line, sites[0].text.as_str()), (3, "fn run_cmd() {"));

    let Reply::Sites { sites, .. } = h.call(Request::Reach { name: "main".into(), limit: 10 }) else { panic!() };
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].context, "main → handler → run_cmd → system");
    assert_eq!(&sites[0].text[sites[0].text_col as usize..][..sites[0].len as usize], "system");

    // Saving a file drops its stale outline (margin-code's outline replaces it) without a rescan.
    h.call(Request::WriteFile { path: p("b.rs"), data: b"fn main() {}\n".to_vec(), expect_mtime_ns: None, create_new: false });
    let Reply::Lens { fns } = h.call(Request::Lens { fns: vec![LensFn { name: "handler".into(), callees: vec![] }] }) else { panic!() };
    assert_eq!(fns[0].callers, 0);
    stop(h, j);
}

#[test]
fn spawn_local_serves() {
    let t = fixture();
    let mut h = H::new(margin_engine::spawn_local(cfg(t.path(), None), Arc::new(|| {})));
    assert!(matches!(h.hello(), Event::Hello { .. }));
    h.ready(0);
    let Reply::FileMatches { items, total_files } = h.call(Request::FindFiles { query: "notes".into(), include: String::new(), exclude: String::new(), limit: 5 }) else { panic!() };
    assert_eq!((items[0].path.as_str(), total_files), ("notes.md", 4));
    assert_eq!(h.search(q("alpha")).unwrap().0.len(), 1);
}
