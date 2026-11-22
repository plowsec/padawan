//! Request loop, pools, cancellation, incremental index updates, events.
//!
//! | Thread / pool | Runs |
//! |---|---|
//! | caller of `run` | request loop: dispatch only, never slow work |
//! | `io` pool (4) | ListDir, ReadFile, WriteFile, CreateDir, Rename, Delete, Stat |
//! | `query` pool (`threads`) | FindFiles, Search, Lens, Callers, Defs, Reach |
//! | `index` thread + pool (`threads`) | walk, index, cache; rescans coalesced |
//! | `graph` thread | rebuilds a big call graph (> 300k defs + calls) after a save; smaller ones rebuild inline |
//!
//! Readers take `Arc<Snapshot>` (swapped under a short lock). File operations update the snapshot
//! directly (logged while a rescan runs, replayed on its commit). Cache: written after a rescan
//! that changed something, and at shutdown if dirty.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use margin_proto::limits::MAX_ITEMS;
use margin_proto::{
    DirEntry, EntryKind, ErrCode, Event, IndexOpts, IndexPhase, IndexStatus, Outbox, RelPath, Reply, ReqId, Request, Site,
};
use rayon::prelude::*;

use crate::cache;
use crate::graph::{Graph, Loc};
use crate::ignore;
use crate::index::{content_cap, index_file, FileRec, F_PENDING};
use crate::jail::{Jail, JailErr};
use crate::snippet::{line_range, make_site};
use crate::walk;
use crate::EngineConfig;

const IO_THREADS: usize = 4;
const TICK: Duration = Duration::from_millis(100);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(10);
/// Above this graph size (defs + call sites) a save rebuilds the graph in the background.
const SYNC_GRAPH: usize = 300_000;

pub struct Snapshot {
    /// Sorted by path.
    pub files: Arc<Vec<Arc<FileRec>>>,
    pub graph: Arc<Graph>,
    pub max_file_bytes: u64,
    /// A full index finished at least once.
    pub ready: bool,
}

#[derive(Clone, Debug)]
enum Update {
    Put(Arc<FileRec>),
    Remove(Box<str>),
    /// Everything under `dir/`.
    RemovePrefix(Box<str>),
}

struct State {
    snap: Arc<Snapshot>,
    seq: u64,
    log: Vec<(u64, Update)>,
    rescanning: bool,
    dirty: bool,
}

enum Job {
    Rescan(IndexOpts),
}

struct Shared {
    jail: Jail,
    root_str: String,
    out: Outbox,
    cache_dir: Option<PathBuf>,
    query: rayon::ThreadPool,
    io: rayon::ThreadPool,
    index: rayon::ThreadPool,
    state: Mutex<State>,
    opts: Mutex<IndexOpts>,
    cancels: Mutex<HashMap<ReqId, Arc<AtomicBool>>>,
    stop: AtomicBool,
    inflight: (Mutex<usize>, Condvar),
    jobs: Mutex<Option<Sender<Job>>>,
    regraph: Mutex<Option<Sender<()>>>,
}

/// Decrements the in-flight count on drop (also on panic).
struct Flight(Arc<Shared>);

impl Drop for Flight {
    fn drop(&mut self) {
        let (m, cv) = &self.0.inflight;
        if let Ok(mut n) = m.lock() {
            *n = n.saturating_sub(1);
            cv.notify_all();
        }
    }
}

fn pool(n: usize, name: &'static str) -> Result<rayon::ThreadPool, String> {
    rayon::ThreadPoolBuilder::new().num_threads(n).thread_name(move |i| format!("engine-{name}-{i}")).build().map_err(|e| format!("thread pool: {e}"))
}

fn err(code: ErrCode, msg: impl Into<String>) -> Reply {
    Reply::Err { code, msg: msg.into() }
}

fn jerr(e: JailErr) -> Reply {
    Reply::Err { code: e.code, msg: e.msg }
}

impl Shared {
    fn snap(&self) -> Arc<Snapshot> {
        match self.state.lock() {
            Ok(s) => s.snap.clone(),
            Err(e) => e.into_inner().snap.clone(),
        }
    }

    fn opts(&self) -> IndexOpts {
        self.opts.lock().map(|o| o.clone()).unwrap_or_default()
    }

    fn rescan(&self) {
        let o = self.opts();
        if let Ok(j) = self.jobs.lock() {
            if let Some(tx) = j.as_ref() {
                let _ = tx.send(Job::Rescan(o));
            }
        }
    }

    fn reply(&self, id: ReqId, r: Reply) {
        self.out.reply(id, r);
    }
}

pub fn run(cfg: EngineConfig, requests: Receiver<(ReqId, Request)>, out: Outbox) {
    let jail = match Jail::new(&cfg.root) {
        Ok(j) => j,
        Err(e) => {
            out.event(Event::Fatal { msg: format!("cannot open {}: {e}", cfg.root.display()) });
            return;
        }
    };
    let threads = cfg.threads;
    let pools = (pool(threads, "query"), pool(IO_THREADS, "io"), pool(threads, "index"));
    let (query, io, index) = match pools {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
            out.event(Event::Fatal { msg: e });
            return;
        }
    };
    let root_str = jail.root().to_string_lossy().into_owned();
    let name = jail.root().file_name().map_or_else(|| root_str.clone(), |n| n.to_string_lossy().into_owned());
    out.event(Event::Hello { protocol: margin_proto::PROTOCOL, root: root_str.clone(), name });

    let empty = Snapshot { files: Arc::new(Vec::new()), graph: Arc::new(Graph::default()), max_file_bytes: cfg.opts.max_file_bytes, ready: false };
    let (jtx, jrx) = channel();
    let (gtx, grx) = channel();
    let sh = Arc::new(Shared {
        jail,
        root_str,
        out,
        cache_dir: cfg.cache_dir.clone().filter(|d| d.is_absolute()),
        query,
        io,
        index,
        state: Mutex::new(State { snap: Arc::new(empty), seq: 0, log: Vec::new(), rescanning: false, dirty: false }),
        opts: Mutex::new(cfg.opts.clone()),
        cancels: Mutex::new(HashMap::new()),
        stop: AtomicBool::new(false),
        inflight: (Mutex::new(0), Condvar::new()),
        jobs: Mutex::new(Some(jtx)),
        regraph: Mutex::new(Some(gtx)),
    });
    let gsh = sh.clone();
    let grapher = std::thread::Builder::new().name("engine-graph".into()).spawn(move || graph_loop(&gsh, &grx));

    let ish = sh.clone();
    let first = cfg.opts.clone();
    let indexer = std::thread::Builder::new().name("engine-indexer".into()).spawn(move || index_loop(&ish, first, &jrx));

    for (id, req) in requests.iter() {
        if !dispatch(&sh, id, req) {
            break;
        }
    }

    // Shutdown: stop the indexer and every stream, wait for in-flight work, persist.
    sh.stop.store(true, Ordering::Relaxed);
    if let Ok(c) = sh.cancels.lock() {
        for f in c.values() {
            f.store(true, Ordering::Relaxed);
        }
    }
    if let Ok(mut j) = sh.jobs.lock() {
        *j = None;
    }
    if let Ok(mut j) = sh.regraph.lock() {
        *j = None;
    }
    {
        let (m, cv) = &sh.inflight;
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        if let Ok(mut n) = m.lock() {
            while *n > 0 {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                match cv.wait_timeout(n, left) {
                    Ok((g, _)) => n = g,
                    Err(_) => break,
                }
            }
        }
    }
    if let Ok(h) = indexer {
        let _ = h.join();
    }
    if let Ok(h) = grapher {
        let _ = h.join();
    }
    let (snap, dirty) = match sh.state.lock() {
        Ok(s) => (s.snap.clone(), s.dirty),
        Err(_) => return,
    };
    if dirty && snap.ready {
        save_cache(&sh, &snap.files);
    }
}

/// `false`: stop serving.
fn dispatch(sh: &Arc<Shared>, id: ReqId, req: Request) -> bool {
    match req {
        Request::Shutdown => return false,
        Request::Cancel { target } => {
            if let Ok(c) = sh.cancels.lock() {
                if let Some(f) = c.get(&target) {
                    f.store(true, Ordering::Relaxed);
                }
            }
        }
        Request::Reindex { opts } => {
            if let Ok(mut o) = sh.opts.lock() {
                *o = opts;
            }
            sh.reply(id, Reply::Done);
            sh.rescan();
        }
        r @ (Request::ListDir { .. }
        | Request::ReadFile { .. }
        | Request::WriteFile { .. }
        | Request::CreateDir { .. }
        | Request::Rename { .. }
        | Request::Delete { .. }
        | Request::Stat { .. }) => {
            let f = flight(sh);
            sh.io.spawn(move || {
                let sh = f.0.clone();
                let rep = file_op(&sh, r);
                sh.reply(id, rep);
                drop(f);
            });
        }
        r => {
            let flag = Arc::new(AtomicBool::new(false));
            if let Ok(mut c) = sh.cancels.lock() {
                c.insert(id, flag.clone());
            }
            let f = flight(sh);
            sh.query.spawn(move || {
                let sh = f.0.clone();
                query_op(&sh, id, r, &flag);
                if let Ok(mut c) = sh.cancels.lock() {
                    c.remove(&id);
                }
                drop(f);
            });
        }
    }
    true
}

fn flight(sh: &Arc<Shared>) -> Flight {
    if let Ok(mut n) = sh.inflight.0.lock() {
        *n += 1;
    }
    Flight(sh.clone())
}

// ---- file operations ----

fn file_op(sh: &Shared, req: Request) -> Reply {
    let j = &sh.jail;
    match req {
        Request::ListDir { path } => list_dir(sh, &path),
        Request::ReadFile { path } => match j.read_file(&path) {
            Ok((data, meta, binary)) => Reply::File { data, meta, binary },
            Err(e) => jerr(e),
        },
        Request::Stat { path } => match j.stat(&path) {
            Ok(m) => Reply::Meta(m),
            Err(e) => jerr(e),
        },
        Request::WriteFile { path, data, expect_mtime_ns, create_new } => match j.write_file(&path, &data, expect_mtime_ns, create_new) {
            Ok(m) => {
                // Through a link: refresh the target inside the folder.
                let target = j.resolve_existing(&path).ok().and_then(|c| rel_of(j.root(), &c)).unwrap_or(path);
                refresh(sh, &target);
                Reply::Meta(m)
            }
            Err(e) => jerr(e),
        },
        Request::CreateDir { path } => match j.create_dir(&path) {
            Ok(()) => Reply::Done,
            Err(e) => jerr(e),
        },
        Request::Rename { from, to } => match j.rename(&from, &to) {
            Ok(()) => {
                let is_dir = std::fs::symlink_metadata(j.lexical(&to)).is_ok_and(|m| m.is_dir());
                if is_dir {
                    update(sh, Update::RemovePrefix(from.as_str().into()));
                    sh.rescan();
                } else {
                    update(sh, Update::Remove(from.as_str().into()));
                    refresh(sh, &to);
                    if from.name() == ".gitignore" {
                        sh.rescan();
                    }
                }
                Reply::Done
            }
            Err(e) => jerr(e),
        },
        Request::Delete { path } => match j.delete(&path) {
            Ok(kind) => {
                if kind == EntryKind::Dir {
                    update(sh, Update::RemovePrefix(path.as_str().into()));
                } else {
                    update(sh, Update::Remove(path.as_str().into()));
                }
                if path.name() == ".gitignore" {
                    sh.rescan();
                }
                Reply::Done
            }
            Err(e) => jerr(e),
        },
        _ => err(ErrCode::Invalid, "not a file operation"),
    }
}

fn rel_of(root: &Path, p: &Path) -> Option<RelPath> {
    let r = p.strip_prefix(root).ok()?;
    let parts: Vec<&str> = r.components().map(|c| c.as_os_str().to_str()).collect::<Option<_>>()?;
    RelPath::new(&parts.join("/")).ok()
}

fn list_dir(sh: &Shared, path: &RelPath) -> Reply {
    let (mut entries, truncated, _) = match sh.jail.list_dir(path) {
        Ok(x) => x,
        Err(e) => return jerr(e),
    };
    let opts = sh.opts();
    let mut comps: Vec<&str> = path.components().collect();
    let (stack, dir_ignored) = ignore::stack_for_dir(sh.jail.root(), &comps, opts.respect_gitignore);
    for e in &mut entries {
        comps.push(&e.name);
        e.ignored = dir_ignored || ignore::always_skip(&e.name) || (opts.respect_gitignore && ignore::is_ignored(&stack, &comps, e.kind == EntryKind::Dir));
        comps.pop();
    }
    sort_entries(&mut entries);
    Reply::Dir { entries, truncated }
}

pub fn sort_entries(v: &mut [DirEntry]) {
    v.sort_by_cached_key(|e| (e.kind != EntryKind::Dir, e.name.to_lowercase(), e.name.clone()));
}

/// Should `rel` be in the file table under the current options?
fn wanted(sh: &Shared, rel: &RelPath, opts: &IndexOpts) -> bool {
    let comps: Vec<&str> = rel.components().collect();
    let Some((_, dir)) = comps.split_last() else { return false };
    if comps.iter().any(|c| ignore::always_skip(c) || (!opts.index_hidden && c.starts_with('.'))) {
        return false;
    }
    let (stack, dir_ignored) = ignore::stack_for_dir(sh.jail.root(), dir, opts.respect_gitignore);
    !(dir_ignored || (opts.respect_gitignore && ignore::is_ignored(&stack, &comps, false)))
}

/// Re-index one path after a file operation.
fn refresh(sh: &Shared, rel: &RelPath) {
    let opts = sh.opts();
    let lex = sh.jail.lexical(rel);
    let regular = std::fs::symlink_metadata(&lex).is_ok_and(|m| m.is_file());
    // No symlinked directory on the way (the walker would not have entered it).
    let plain = std::fs::canonicalize(&lex).is_ok_and(|c| c == lex);
    if regular && plain && wanted(sh, rel, &opts) {
        let md = std::fs::symlink_metadata(&lex).ok();
        let (size, mt) = md.map_or((0, 0), |m| (m.len(), crate::jail::mtime_ns(&m)));
        let mut buf = Vec::new();
        let rec = index_file(&lex, rel.as_str(), size, mt, opts.max_file_bytes, &mut buf);
        update(sh, Update::Put(Arc::new(rec)));
    } else {
        update(sh, Update::Remove(rel.as_str().into()));
    }
    if rel.name() == ".gitignore" {
        sh.rescan();
    }
}

fn apply(files: &mut Vec<Arc<FileRec>>, u: &Update) -> bool {
    let had_fns = |f: &Arc<FileRec>| !f.fns.is_empty();
    match u {
        Update::Put(rec) => match files.binary_search_by(|f| f.path.cmp(&rec.path)) {
            Ok(i) => {
                let g = had_fns(&files[i]) || !rec.fns.is_empty();
                files[i] = rec.clone();
                g
            }
            Err(i) => {
                files.insert(i, rec.clone());
                !rec.fns.is_empty()
            }
        },
        Update::Remove(p) => match files.binary_search_by(|f| (*f.path).cmp(p)) {
            Ok(i) => had_fns(&files.remove(i)),
            Err(_) => false,
        },
        Update::RemovePrefix(d) => {
            let pre = format!("{d}/");
            let mut g = false;
            files.retain(|f| {
                let keep = !f.path.starts_with(&pre);
                g |= !keep && had_fns(f);
                keep
            });
            g
        }
    }
}

/// Apply one update to the live snapshot (and log it for a running rescan).
fn update(sh: &Shared, u: Update) {
    let mut cur = {
        let Ok(mut st) = sh.state.lock() else { return };
        st.seq += 1;
        let seq = st.seq;
        if st.rescanning {
            st.log.push((seq, u.clone()));
        }
        st.dirty = true;
        st.snap.clone()
    };
    loop {
        let mut files = (*cur.files).clone();
        let regraph = apply(&mut files, &u);
        let sync = regraph && cur.graph.size < SYNC_GRAPH;
        let graph = if sync { Arc::new(Graph::build(&files)) } else { cur.graph.clone() };
        let next = Arc::new(Snapshot { files: Arc::new(files), graph, max_file_bytes: cur.max_file_bytes, ready: cur.ready });
        let Ok(mut st) = sh.state.lock() else { return };
        if Arc::ptr_eq(&st.snap, &cur) {
            st.snap = next;
            drop(st);
            if regraph && !sync {
                if let Ok(g) = sh.regraph.lock() {
                    if let Some(tx) = g.as_ref() {
                        let _ = tx.send(());
                    }
                }
            }
            return;
        }
        cur = st.snap.clone();
    }
}

/// Background graph rebuilds for big graphs (coalesced). The old graph stays consistent meanwhile.
fn graph_loop(sh: &Shared, rx: &Receiver<()>) {
    while rx.recv().is_ok() {
        while rx.try_recv().is_ok() {}
        loop {
            if sh.stop.load(Ordering::Relaxed) {
                return;
            }
            let cur = sh.snap();
            let graph = Arc::new(Graph::build(&cur.files));
            let Ok(mut st) = sh.state.lock() else { return };
            if Arc::ptr_eq(&st.snap.files, &cur.files) {
                let s = &st.snap;
                st.snap = Arc::new(Snapshot { files: s.files.clone(), graph, max_file_bytes: s.max_file_bytes, ready: s.ready });
                break;
            }
        }
    }
}

/// Publish a rescan result, replaying updates logged since `since`.
/// `last`: the rescan is over (stop logging). `ready` is sticky once a full index finished.
fn commit(sh: &Shared, mut files: Vec<Arc<FileRec>>, max_file_bytes: u64, last: bool, since: u64) -> Arc<Snapshot> {
    let mut applied = since;
    loop {
        let pending: Vec<(u64, Update)> = match sh.state.lock() {
            Ok(st) => st.log.iter().filter(|(s, _)| *s > applied).cloned().collect(),
            Err(_) => Vec::new(),
        };
        for (s, u) in &pending {
            apply(&mut files, u);
            applied = *s;
        }
        let graph = Arc::new(Graph::build(&files));
        let ready = last || sh.snap().ready;
        let snap = Arc::new(Snapshot { files: Arc::new(files.clone()), graph, max_file_bytes, ready });
        let Ok(mut st) = sh.state.lock() else { return snap };
        if st.log.iter().all(|(s, _)| *s <= applied) {
            st.snap = snap.clone();
            if last {
                st.rescanning = false;
                st.log.clear();
            }
            return snap;
        }
    }
}

// ---- indexing ----

fn index_loop(sh: &Arc<Shared>, first: IndexOpts, jobs: &Receiver<Job>) {
    let mut opts = first;
    let mut initial = true;
    loop {
        if sh.stop.load(Ordering::Relaxed) {
            return;
        }
        index_once(sh, &opts, initial);
        initial = false;
        match jobs.recv() {
            Ok(Job::Rescan(o)) => opts = o,
            Err(_) => return,
        }
        // Coalesce.
        while let Ok(Job::Rescan(o)) = jobs.try_recv() {
            opts = o;
        }
    }
}

struct Progress {
    phase: AtomicU32,
    seen: AtomicU32,
    done: AtomicU32,
    bytes: AtomicU64,
    from_cache: AtomicU32,
    skipped: AtomicU32,
}

fn status(p: &Progress, t0: Instant, symbols: u32) -> IndexStatus {
    let phase = match p.phase.load(Ordering::Relaxed) {
        0 => IndexPhase::Scanning,
        1 => IndexPhase::Indexing,
        _ => IndexPhase::Ready,
    };
    IndexStatus {
        phase,
        files_done: p.done.load(Ordering::Relaxed),
        files_total: p.seen.load(Ordering::Relaxed),
        bytes: p.bytes.load(Ordering::Relaxed),
        symbols,
        skipped: p.skipped.load(Ordering::Relaxed),
        from_cache: p.from_cache.load(Ordering::Relaxed),
        elapsed_ms: u32::try_from(t0.elapsed().as_millis()).unwrap_or(u32::MAX),
    }
}

fn index_once(sh: &Arc<Shared>, opts: &IndexOpts, initial: bool) {
    let t0 = Instant::now();
    let since = match sh.state.lock() {
        Ok(mut st) => {
            st.rescanning = true;
            st.seq
        }
        Err(_) => return,
    };
    let p = Progress { phase: AtomicU32::new(0), seen: AtomicU32::new(0), done: AtomicU32::new(0), bytes: AtomicU64::new(0), from_cache: AtomicU32::new(0), skipped: AtomicU32::new(0) };
    let done = AtomicBool::new(false);
    let (tick_tx, tick_rx) = channel::<()>();
    std::thread::scope(|s| {
        let (done, p, shr) = (&done, &p, &**sh);
        s.spawn(move || loop {
            match tick_rx.recv_timeout(TICK) {
                Err(RecvTimeoutError::Timeout) => {
                    if done.load(Ordering::Relaxed) || shr.stop.load(Ordering::Relaxed) {
                        return;
                    }
                    shr.out.event(Event::Index(status(p, t0, 0)));
                }
                _ => return,
            }
        });
        let res = build(sh, opts, initial, since, p);
        done.store(true, Ordering::Relaxed);
        let _ = tick_tx.send(());
        if let Some((snap, changed)) = res {
            p.phase.store(2, Ordering::Relaxed);
            p.done.store(snap.files.len() as u32, Ordering::Relaxed);
            p.seen.store(snap.files.len() as u32, Ordering::Relaxed);
            let wrote = changed && save_cache(sh, &snap.files);
            if wrote || !changed {
                if let Ok(mut st) = sh.state.lock() {
                    if !st.rescanning {
                        st.dirty = false;
                    }
                }
            }
            sh.out.event(Event::Index(status(p, t0, snap.graph.symbols)));
        }
    });
}

/// Walk + reuse + index. `None` if stopped. Returns the snapshot and whether anything changed vs the base.
fn build(sh: &Arc<Shared>, opts: &IndexOpts, initial: bool, since: u64, p: &Progress) -> Option<(Arc<Snapshot>, bool)> {
    let root = sh.jail.root();
    let cur = sh.snap();
    let (base, from_disk): (Vec<Arc<FileRec>>, bool) = if initial && cur.files.is_empty() {
        match sh.cache_dir.as_deref().and_then(|d| cache::load(d, &sh.root_str)) {
            Some(v) => (v, true),
            None => (Vec::new(), false),
        }
    } else {
        ((*cur.files).clone(), false)
    };
    let base_map: HashMap<&str, &Arc<FileRec>> = base.iter().map(|f| (&*f.path, f)).collect();
    let w = walk::walk(&sh.index, root, opts, &sh.stop, &p.seen);
    if sh.stop.load(Ordering::Relaxed) {
        return None;
    }
    let mut reused = 0u32;
    let mut bytes = 0u64;
    let mut files: Vec<Arc<FileRec>> = w
        .files
        .into_iter()
        .map(|e| match base_map.get(e.path.as_str()) {
            Some(old) if old.reusable(e.size, e.mtime_ns, opts.max_file_bytes) => {
                reused += 1;
                bytes = bytes.saturating_add(old.content_bytes());
                (*old).clone()
            }
            _ => Arc::new(FileRec::pending(e.path.into(), e.size, e.mtime_ns)),
        })
        .collect();
    let removed = base.len() > reused as usize;
    drop(base_map);
    drop(base);
    p.seen.store(files.len() as u32, Ordering::Relaxed);
    p.done.store(reused, Ordering::Relaxed);
    p.bytes.store(bytes, Ordering::Relaxed);
    p.skipped.store(w.skipped, Ordering::Relaxed);
    if from_disk {
        p.from_cache.store(reused, Ordering::Relaxed);
    }
    p.phase.store(1, Ordering::Relaxed);
    let todo: Vec<usize> = files.iter().enumerate().filter(|(_, f)| f.flags & F_PENDING != 0).map(|(i, _)| i).collect();
    // Finder works now; search scans pending files.
    if !todo.is_empty() {
        commit(sh, files.clone(), opts.max_file_bytes, false, since);
    }
    let stop = &sh.stop;
    let new: Vec<(usize, Arc<FileRec>)> = sh.index.install(|| {
        todo.par_iter()
            .map_init(Vec::new, |buf, &i| {
                let f = &files[i];
                if stop.load(Ordering::Relaxed) {
                    return (i, f.clone());
                }
                let rec = index_file(&root.join(&*f.path), &f.path, f.size, f.mtime_ns, opts.max_file_bytes, buf);
                if buf.capacity() > (8 << 20) {
                    *buf = Vec::new();
                }
                p.done.fetch_add(1, Ordering::Relaxed);
                p.bytes.fetch_add(rec.content_bytes(), Ordering::Relaxed);
                (i, Arc::new(rec))
            })
            .collect()
    });
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    let changed = !new.is_empty() || removed || (initial && !from_disk);
    for (i, rec) in new {
        if let Some(slot) = files.get_mut(i) {
            *slot = rec;
        }
    }
    let extra = files.iter().filter(|f| !f.searchable()).count() as u32;
    p.skipped.store(w.skipped.saturating_add(extra), Ordering::Relaxed);
    let snap = commit(sh, files, opts.max_file_bytes, true, since);
    Some((snap, changed))
}

fn save_cache(sh: &Shared, files: &[Arc<FileRec>]) -> bool {
    match sh.cache_dir.as_deref() {
        Some(d) => cache::save(d, &sh.root_str, files).is_ok(),
        None => false,
    }
}

// ---- queries ----

fn clamp_limit(limit: u32) -> usize {
    if limit == 0 {
        MAX_ITEMS
    } else {
        (limit as usize).min(MAX_ITEMS)
    }
}

fn query_op(sh: &Shared, id: ReqId, req: Request, cancel: &AtomicBool) {
    let snap = sh.snap();
    match req {
        Request::FindFiles { query, limit, .. } => {
            let items = crate::fuzzy::find(&sh.query, &snap.files, &query, limit);
            sh.reply(id, Reply::FileMatches { items, total_files: u32::try_from(snap.files.len()).unwrap_or(u32::MAX) });
        }
        Request::Search { query } => {
            let plan = match crate::search::plan(&query) {
                Ok(p) => p,
                Err((c, m)) => return sh.reply(id, err(c, m)),
            };
            let emit = |hits: Vec<Site>| sh.reply(id, Reply::SearchBatch { hits });
            match crate::search::run(&sh.query, sh.jail.root(), &snap.files, &plan, snap.max_file_bytes, cancel, &emit) {
                Ok(stats) => sh.reply(id, Reply::SearchDone(stats)),
                Err(code) => sh.reply(id, err(code, "search cancelled")),
            }
        }
        Request::Lens { fns } => {
            let g = &snap.graph;
            let fns = fns.iter().take(MAX_ITEMS).map(|f| g.lens(&f.name, &f.callees)).collect();
            sh.reply(id, Reply::Lens { fns });
        }
        Request::Callers { name, limit } => {
            let g = &snap.graph;
            let raw: Vec<Raw> = g.callers(&name).iter().take(MAX_ITEMS).map(|c| Raw { at: c.at, len: name.len(), ctx: g.name(c.caller).to_owned() }).collect();
            sites_reply(sh, id, &snap, raw, limit, cancel);
        }
        Request::Defs { name, limit } => {
            let g = &snap.graph;
            let raw: Vec<Raw> = g.defs(&name).iter().take(MAX_ITEMS).map(|d| Raw { at: *d, len: name.len(), ctx: String::new() }).collect();
            sites_reply(sh, id, &snap, raw, limit, cancel);
        }
        Request::Reach { name, limit } => {
            let raw: Vec<Raw> = snap.graph.reach_sites(&name, MAX_ITEMS).into_iter().map(|r| Raw { at: r.at, len: r.sink_len as usize, ctx: r.chain }).collect();
            sites_reply(sh, id, &snap, raw, limit, cancel);
        }
        _ => sh.reply(id, err(ErrCode::Invalid, "not a query")),
    }
}

struct Raw {
    at: Loc,
    len: usize,
    ctx: String,
}

/// Sort by path then line, cut to `limit`, read each file once (parallel, through the jail) for the line text.
fn sites_reply(sh: &Shared, id: ReqId, snap: &Snapshot, mut raw: Vec<Raw>, limit: u32, cancel: &AtomicBool) {
    let g = &snap.graph;
    raw.sort_by(|a, b| g.path(a.at.file).cmp(g.path(b.at.file)).then(a.at.line.cmp(&b.at.line)).then(a.at.col.cmp(&b.at.col)));
    let limit = clamp_limit(limit);
    let truncated = raw.len() > limit;
    raw.truncate(limit);
    let mut groups: Vec<(u32, Vec<Raw>)> = Vec::new();
    for r in raw {
        match groups.last_mut() {
            Some((f, v)) if *f == r.at.file => v.push(r),
            _ => groups.push((r.at.file, vec![r])),
        }
    }
    let cap = content_cap(snap.max_file_bytes);
    let jail = &sh.jail;
    let sites: Vec<Site> = sh.query.install(|| {
        groups
            .into_par_iter()
            .flat_map_iter(|(file, rs)| {
                let path = RelPath::new(g.path(file)).unwrap_or_default();
                let mut data = Vec::new();
                let ok = !cancel.load(Ordering::Relaxed) && jail.read_resolved(&path, cap, &mut data).is_ok();
                if !ok {
                    data.clear();
                }
                rs.into_iter()
                    .map(|r| {
                        let line = line_range(&data, r.at.line).and_then(|(a, b)| data.get(a..b)).unwrap_or_default();
                        make_site(&path, r.at.line, line, r.at.col as usize, r.len, r.ctx)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    });
    if cancel.load(Ordering::Relaxed) {
        return sh.reply(id, err(ErrCode::Cancelled, "cancelled"));
    }
    sh.reply(id, Reply::Sites { sites, truncated });
}
