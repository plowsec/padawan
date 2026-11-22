//! Parallel directory walk → file table rows.
//!
//! | Entry | Result |
//! |---|---|
//! | dir symlink, file symlink | not entered / not indexed (counted in `skipped`) |
//! | `.git` `.hg` `.svn` `.DS_Store` | skipped |
//! | `.gitignore` match (`respect_gitignore`) | skipped; ignored dirs not entered |
//! | dot entry, `index_hidden == false` | skipped |
//! | non-UTF-8 / invalid name, FIFO, socket, device | skipped |
//! | caps | depth 64, 2_000_000 files, 1_000_000 entries per dir |
//!
//! One rayon task per directory on the caller's pool; no recursion on the thread stack.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

use margin_proto::path::valid_name;
use margin_proto::IndexOpts;
use rayon::prelude::*;

use crate::ignore::{self, Stack};
use crate::jail::mtime_ns;

pub const MAX_DEPTH: usize = 64;
pub const MAX_FILES: usize = 2_000_000;
pub const MAX_PER_DIR: usize = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkEntry {
    pub path: String,
    pub size: u64,
    pub mtime_ns: u64,
}

#[derive(Debug, Default)]
pub struct WalkOut {
    /// Sorted by path.
    pub files: Vec<WalkEntry>,
    pub skipped: u32,
}

struct Ctx<'a> {
    root: &'a Path,
    opts: &'a IndexOpts,
    stop: &'a AtomicBool,
    seen: &'a AtomicU32,
    count: AtomicUsize,
    skipped: AtomicU32,
    out: Mutex<Vec<WalkEntry>>,
}

impl Ctx<'_> {
    fn skip(&self, n: u32) {
        self.skipped.fetch_add(n, Ordering::Relaxed);
    }
}

/// Walk `root` (canonical). `seen` counts files found (progress). `stop` aborts early.
pub fn walk(pool: &rayon::ThreadPool, root: &Path, opts: &IndexOpts, stop: &AtomicBool, seen: &AtomicU32) -> WalkOut {
    let ctx = Ctx { root, opts, stop, seen, count: AtomicUsize::new(0), skipped: AtomicU32::new(0), out: Mutex::new(Vec::new()) };
    pool.scope(|s| visit(&ctx, s, String::new(), 0, None));
    let mut files = ctx.out.into_inner().unwrap_or_else(|e| e.into_inner());
    pool.install(|| files.par_sort_unstable_by(|a, b| a.path.cmp(&b.path)));
    WalkOut { files, skipped: ctx.skipped.into_inner() }
}

fn visit<'s>(ctx: &'s Ctx<'s>, s: &rayon::Scope<'s>, rel: String, depth: usize, parent: Stack) {
    if ctx.stop.load(Ordering::Relaxed) {
        return;
    }
    let dir = if rel.is_empty() { ctx.root.to_path_buf() } else { ctx.root.join(&rel) };
    let Ok(rd) = fs::read_dir(&dir) else {
        ctx.skip(1);
        return;
    };
    let mut ents: Vec<(String, fs::DirEntry)> = Vec::new();
    let mut has_ignore = false;
    for e in rd {
        let Ok(e) = e else {
            ctx.skip(1);
            continue;
        };
        if ents.len() >= MAX_PER_DIR {
            ctx.skip(1);
            continue;
        }
        match e.file_name().into_string() {
            Ok(n) if valid_name(&n) => {
                has_ignore |= n == ".gitignore";
                ents.push((n, e));
            }
            _ => ctx.skip(1),
        }
    }
    let mut stack = parent;
    if has_ignore && ctx.opts.respect_gitignore {
        if let Some(f) = ignore::load(&dir.join(".gitignore")) {
            stack = ignore::push(&stack, depth, f);
        }
    }
    let mut comps: Vec<&str> = rel.split('/').filter(|c| !c.is_empty()).collect();
    let mut local: Vec<WalkEntry> = Vec::new();
    for (name, e) in &ents {
        let Ok(ft) = e.file_type() else {
            ctx.skip(1);
            continue;
        };
        if ignore::always_skip(name) || (!ctx.opts.index_hidden && name.starts_with('.')) {
            ctx.skip(1);
            continue;
        }
        let is_dir = ft.is_dir();
        if !is_dir && !ft.is_file() {
            // Symlinks, FIFOs, sockets, devices.
            ctx.skip(1);
            continue;
        }
        comps.push(name);
        let ignored = ctx.opts.respect_gitignore && ignore::is_ignored(&stack, &comps, is_dir);
        comps.pop();
        if ignored {
            ctx.skip(1);
            continue;
        }
        let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if child.len() > margin_proto::limits::MAX_PATH {
            ctx.skip(1);
            continue;
        }
        if is_dir {
            if depth + 1 > MAX_DEPTH {
                ctx.skip(1);
                continue;
            }
            let st = stack.clone();
            s.spawn(move |s| visit(ctx, s, child, depth + 1, st));
            continue;
        }
        if ctx.count.fetch_add(1, Ordering::Relaxed) >= MAX_FILES {
            ctx.skip(1);
            continue;
        }
        // `DirEntry::metadata` is an lstat.
        let (size, mt) = match e.metadata() {
            Ok(m) => (m.len(), mtime_ns(&m)),
            Err(_) => (0, 0),
        };
        ctx.seen.fetch_add(1, Ordering::Relaxed);
        local.push(WalkEntry { path: child, size, mtime_ns: mt });
    }
    if !local.is_empty() {
        if let Ok(mut out) = ctx.out.lock() {
            out.append(&mut local);
        }
    }
}
