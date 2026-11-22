//! `Replace`: rewrite every match of a search query, file by file, atomically, through the jail.
//!
//! | Step | Rule |
//! |---|---|
//! | files | as `Search` (searchable, include/exclude, bloom); `only` non-empty → just those (still indexed); `skip` → never |
//! | read | lexical path, `O_NOFOLLOW`, regular file, capped like search, under the I/O gate; mtime kept |
//! | matches | `search::Plan` (literal / regex, case, whole word); empty matches skipped |
//! | replacement | literal; regex query: `$1` / `${name}` / `$$` expand |
//! | write | `Jail::write_file` with `expect_mtime_ns` = mtime read (→ "changed on disk") |
//! | bytes outside matches | untouched (CRLF, BOM, trailing newline) |
//! | cancel | no new file starts; files already rewritten stay |
//!
//! Skipped (`why`, short): too large, binary, not valid UTF-8, read-only, changed on disk, unreadable, not in the index.
//! Without `only`, binary / too-large files are left out silently (as `Search` does).

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use margin_proto::limits::MAX_ITEMS;
use margin_proto::{ErrCode, Meta, RelPath, SearchQuery, Skipped};
use rayon::prelude::*;

use crate::index::{content_cap, FileRec, F_BIG, F_BINARY};
use crate::jail::{is_binary, mtime_ns, open_regular, read_capped, Jail};
use crate::search::{build_re, plan, Plan};

/// Longest `Skipped::why`.
const MAX_WHY: usize = 80;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub files: u32,
    pub replacements: u32,
    /// Sorted, all of them (the reply cuts to `MAX_ITEMS`; the index refresh needs every one).
    pub changed: Vec<RelPath>,
    /// Sorted by path. At most `MAX_ITEMS`.
    pub skipped: Vec<Skipped>,
    pub cancelled: bool,
}

enum One {
    Nothing,
    Changed(u32),
    Skip(String),
    Cancelled,
}

fn why(s: &str) -> String {
    let one: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut e = one.len().min(MAX_WHY);
    while e > 0 && !one.is_char_boundary(e) {
        e -= 1;
    }
    one.get(..e).unwrap_or_default().to_owned()
}

/// Atomic rewrite expecting `mtime`. `Err(why)`. Outside the I/O gate (the fsync would hold a read slot).
pub fn write_back(jail: &Jail, rel: &RelPath, data: &[u8], mtime: u64) -> Result<Meta, String> {
    jail.write_file(rel, data, Some(mtime), false).map_err(|e| if e.code == ErrCode::Conflict { "changed on disk".to_owned() } else { why(&e.msg) })
}

struct Ctx<'a> {
    jail: &'a Jail,
    root: &'a Path,
    plan: &'a Plan,
    rep: &'a [u8],
    cap: usize,
    cancel: &'a AtomicBool,
}

fn one(cx: &Ctx, rel: &RelPath, only: bool) -> One {
    let Ctx { jail, root, plan, rep, cap, cancel } = *cx;
    let mut data = Vec::new();
    let md = {
        let _slot = crate::jail::io_slot();
        let (mut fh, md) = match open_regular(&rel.under(root)) {
            Ok(x) => x,
            Err(e) if e.code == ErrCode::NotFound => return One::Skip("not found".into()),
            Err(e) if e.code == ErrCode::Denied => return One::Skip("not a regular file".into()),
            Err(e) => return One::Skip(why(&e.msg)),
        };
        match read_capped(&mut fh, md.len(), cap, &mut data) {
            Ok(()) => md,
            Err(e) if e.code == ErrCode::TooLarge => return One::Skip("too large".into()),
            Err(e) => return One::Skip(why(&e.msg)),
        }
    };
    if is_binary(&data) {
        return if only { One::Skip("binary".into()) } else { One::Nothing };
    }
    let Some((new, n)) = plan.replace(&data, rep, cancel) else { return One::Cancelled };
    if n == 0 || new == data {
        return One::Nothing;
    }
    if std::str::from_utf8(&data).is_err() {
        return One::Skip("not valid UTF-8".into());
    }
    if md.permissions().readonly() {
        return One::Skip("read-only".into());
    }
    match write_back(jail, rel, &new, mtime_ns(&md)) {
        Ok(_) => One::Changed(n),
        Err(w) => One::Skip(w),
    }
}

/// Run a replace. `Err` for a bad query (empty, bad regex, matches the empty string).
#[allow(clippy::too_many_arguments)]
pub fn run(
    pool: &rayon::ThreadPool,
    jail: &Jail,
    files: &[Arc<FileRec>],
    q: &SearchQuery,
    replacement: &str,
    only: &[RelPath],
    skip: &[RelPath],
    max_file_bytes: u64,
    cancel: &AtomicBool,
) -> Result<Outcome, (ErrCode, String)> {
    if q.pattern.is_empty() {
        return Err((ErrCode::Invalid, "empty pattern".into()));
    }
    if q.regex && build_re(&q.pattern, !q.case_sensitive)?.is_match(b"") {
        return Err((ErrCode::Invalid, "pattern matches the empty string".into()));
    }
    let plan = plan(q)?;
    let skip: HashSet<&str> = skip.iter().map(RelPath::as_str).collect();
    let mut skipped: Vec<Skipped> = Vec::new();
    let cands: Vec<(&FileRec, bool)> = if only.is_empty() {
        pool.install(|| files.par_iter().filter(|f| !skip.contains(&*f.path) && plan.wants(f)).map(|f| (&**f, false)).collect())
    } else {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut v = Vec::new();
        for p in only {
            if skip.contains(p.as_str()) || !seen.insert(p.as_str()) {
                continue;
            }
            match files.binary_search_by(|f| (*f.path).cmp(p.as_str())) {
                Ok(i) => {
                    let Some(f) = files.get(i) else { continue };
                    if f.flags & F_BIG != 0 {
                        skipped.push(Skipped { path: p.clone(), why: "too large".into() });
                    } else if f.flags & F_BINARY != 0 {
                        skipped.push(Skipped { path: p.clone(), why: "binary".into() });
                    } else if !f.searchable() {
                        skipped.push(Skipped { path: p.clone(), why: "unreadable".into() });
                    } else if plan.wants(f) {
                        v.push((&**f, true));
                    }
                }
                Err(_) => skipped.push(Skipped { path: p.clone(), why: "not in the index".into() }),
            }
        }
        v
    };
    let cx = Ctx { jail, root: jail.root(), plan: &plan, rep: replacement.as_bytes(), cap: content_cap(max_file_bytes), cancel };
    let acc = Mutex::new((0u32, 0u32, Vec::<RelPath>::new(), skipped));
    let stopped = AtomicBool::new(false);
    pool.install(|| {
        cands.par_iter().with_max_len(4).for_each(|&(f, only)| {
            if cancel.load(Ordering::Relaxed) {
                stopped.store(true, Ordering::Relaxed);
                return;
            }
            let Ok(rel) = RelPath::new(&f.path) else { return };
            let r = one(&cx, &rel, only);
            let Ok(mut a) = acc.lock() else { return };
            match r {
                One::Nothing => {}
                One::Cancelled => stopped.store(true, Ordering::Relaxed),
                One::Changed(n) => {
                    a.0 = a.0.saturating_add(1);
                    a.1 = a.1.saturating_add(n);
                    a.2.push(rel);
                }
                One::Skip(why) => a.3.push(Skipped { path: rel, why }),
            }
        })
    });
    let (files, replacements, mut changed, mut skipped) = acc.into_inner().unwrap_or_else(|e| e.into_inner());
    changed.sort();
    skipped.sort_by(|a, b| a.path.cmp(&b.path));
    skipped.truncate(MAX_ITEMS);
    Ok(Outcome { files, replacements, changed, skipped, cancelled: stopped.into_inner() || cancel.load(Ordering::Relaxed) })
}
