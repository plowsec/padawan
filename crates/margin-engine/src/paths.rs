//! `Paths`: call chains from a function to sink call sites, with heuristic argument flow (`margin_code::flow`).
//!
//! | Step | How |
//! |---|---|
//! | chains | `Graph::sink_paths`: BFS, shortest chain per sink site, ≤ `MAX_PATHS` |
//! | files | each file on a chain read once (jail, I/O gate), outlined once, in parallel |
//! | hop k | def of F_k containing the call (innermost); args + `from` via `Src::arg_flow` (memoised) |
//! | tracked | hop 0: every parameter of F_0; hop k+1: params of F_{k+1} at positions with a non-empty `from` |
//! | arity | more args than params (not variadic), unknown names → nothing tracked |
//! | order | `flows` first, fewer hops, then sink path / line / col; cut to `limit` |
//! | non-UTF-8 / unreadable file | sites keep their line (lossy), `args` empty |

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use margin_code::flow::{ArgFlow, Sig, Src};
use margin_code::{FnDef, Lang};
use margin_proto::limits::{MAX_ARGS, MAX_ARG_TEXT, MAX_NAME};
use margin_proto::{CallPath, ErrCode, FlowArg, Hop, RelPath};
use rayon::prelude::*;

use crate::graph::Graph;
use crate::jail::Jail;
use crate::snippet::make_site;

/// Paths computed per request (then sorted and cut to `limit`).
pub const MAX_PATHS: usize = 500;

struct FileCtx {
    rel: RelPath,
    data: Vec<u8>,
    /// Line starts.
    starts: Vec<usize>,
    utf8: bool,
    lang: Lang,
    fns: Vec<FnDef>,
}

impl FileCtx {
    fn line(&self, n: u32) -> &[u8] {
        let Some(&s) = self.starts.get(n as usize) else { return &[] };
        let e = self.starts.get(n as usize + 1).map_or(self.data.len(), |x| x.saturating_sub(1));
        self.data.get(s..e.max(s)).unwrap_or_default()
    }
}

fn read(jail: &Jail, path: &str, cap: usize, cancel: &AtomicBool) -> FileCtx {
    let rel = RelPath::new(path).unwrap_or_default();
    let lang = Lang::from_path(path);
    let mut data = Vec::new();
    if !cancel.load(Ordering::Relaxed) {
        let _slot = crate::jail::io_slot();
        if jail.read_resolved(&rel, cap, &mut data).is_err() || crate::jail::is_binary(&data) {
            data.clear();
        }
    }
    let starts = std::iter::once(0).chain(memchr::memchr_iter(b'\n', &data).map(|i| i + 1)).collect();
    let text = std::str::from_utf8(&data).ok();
    let fns = match text {
        Some(t) if lang.has_outline() && !t.is_empty() => margin_code::outline(lang, t).fns,
        _ => Vec::new(),
    };
    FileCtx { rel, utf8: text.is_some(), data, starts, lang, fns }
}

/// Innermost definition named `name` whose extent holds `line`.
fn def_at<'f>(fns: &'f [FnDef], name: &str, line: u32) -> Option<(usize, &'f FnDef)> {
    fns.iter().enumerate().filter(|(_, f)| f.name == name && f.start <= line && line <= f.end).min_by_key(|(_, f)| f.end - f.start)
}

fn cut(s: &str, max: usize) -> String {
    let mut e = s.len().min(max);
    while e > 0 && !s.is_char_boundary(e) {
        e -= 1;
    }
    s.get(..e).unwrap_or_default().to_owned()
}

fn to_wire(a: &ArgFlow) -> FlowArg {
    FlowArg { text: cut(&a.text, MAX_ARG_TEXT), from: a.from.iter().filter(|n| n.len() <= MAX_NAME).take(MAX_ARGS).cloned().collect() }
}

/// Call paths from `name` (to `sink` only when non-empty). `Err(Cancelled)` if cancelled.
#[allow(clippy::too_many_arguments)]
pub fn run(pool: &rayon::ThreadPool, jail: &Jail, g: &Graph, name: &str, sink: &str, limit: u32, cap_bytes: usize, cancel: &AtomicBool) -> Result<(Vec<CallPath>, bool), ErrCode> {
    let limit = if limit == 0 { MAX_PATHS } else { (limit as usize).min(MAX_PATHS) };
    let (raws, more) = g.sink_paths(name, sink, MAX_PATHS);
    let mut fids: Vec<u32> = raws.iter().flat_map(|r| r.hops.iter().map(|h| h.at.file)).collect();
    fids.sort_unstable();
    fids.dedup();
    let files: Vec<FileCtx> = pool.install(|| fids.par_iter().map(|&f| read(jail, g.path(f), cap_bytes, cancel)).collect());
    if cancel.load(Ordering::Relaxed) {
        return Err(ErrCode::Cancelled);
    }
    let srcs: Vec<Option<Src>> = pool.install(|| files.par_iter().map(|f| if f.utf8 { std::str::from_utf8(&f.data).ok().map(|t| Src::new(f.lang, t)) } else { None }).collect());
    let slot: HashMap<u32, usize> = fids.iter().enumerate().map(|(i, &f)| (f, i)).collect();

    let mut arg_memo: HashMap<(usize, u32, u32, Vec<String>), Vec<ArgFlow>> = HashMap::new();
    let mut sig_memo: HashMap<(usize, usize), Sig> = HashMap::new();
    let mut out: Vec<CallPath> = Vec::with_capacity(raws.len());
    for r in &raws {
        if cancel.load(Ordering::Relaxed) {
            return Err(ErrCode::Cancelled);
        }
        let n = r.hops.len();
        let mut tracked: Vec<String> = Vec::new();
        let mut chain_ok = true;
        let mut hops: Vec<Hop> = Vec::with_capacity(n);
        for (k, h) in r.hops.iter().enumerate() {
            let Some(&fi) = slot.get(&h.at.file) else { continue };
            let Some(file) = files.get(fi) else { continue };
            let src = srcs.get(fi).and_then(Option::as_ref);
            let fname = g.name(h.caller);
            let callee = g.name(h.callee);
            let def = def_at(&file.fns, fname, h.at.line);
            if k == 0 {
                tracked = match (src, def) {
                    (Some(s), Some((di, d))) => sig_memo.entry((fi, di)).or_insert_with(|| s.sig(d)).names.iter().filter(|x| !x.is_empty()).cloned().collect(),
                    _ => Vec::new(),
                };
                chain_ok = !tracked.is_empty();
            }
            let args: Vec<ArgFlow> = match (src, def) {
                (Some(s), Some((_, d))) => arg_memo
                    .entry((fi, h.at.line, h.at.col, tracked.clone()))
                    .or_insert_with(|| s.arg_flow(d, h.at.line, h.at.col, &tracked).unwrap_or_default())
                    .clone(),
                _ => Vec::new(),
            };
            let site = make_site(&file.rel, h.at.line, file.line(h.at.line), h.at.col as usize, callee.len(), fname.to_owned());
            // Parameters of the next function fed by tracked arguments.
            if let Some(nh) = r.hops.get(k + 1) {
                let next = slot.get(&nh.at.file).and_then(|&nfi| {
                    let nf = files.get(nfi)?;
                    let ns = srcs.get(nfi)?.as_ref()?;
                    let (ndi, nd) = def_at(&nf.fns, g.name(nh.caller), nh.at.line)?;
                    Some(sig_memo.entry((nfi, ndi)).or_insert_with(|| ns.sig(nd)).clone())
                });
                tracked = match next {
                    Some(sig) => feed(&sig, &args),
                    None => Vec::new(),
                };
                if tracked.is_empty() {
                    chain_ok = false;
                }
            }
            hops.push(Hop { site, args: args.iter().take(MAX_ARGS).map(to_wire).collect() });
        }
        let sink_fed = hops.last().is_some_and(|h| h.args.iter().any(|a| !a.from.is_empty()));
        out.push(CallPath {
            sink: cut(r.hops.last().map_or("", |h| g.name(h.callee)), MAX_NAME),
            kind: crate::index::sink_of(r.kind).map_or("", |k| k.label()).to_owned(),
            hops,
            flows: chain_ok && sink_fed,
        });
    }
    out.sort_by_cached_key(|p| (!p.flows, p.hops.len(), p.hops.last().map(|h| (h.site.path.as_str().to_owned(), h.site.line, h.site.col))));
    let truncated = more || out.len() > limit;
    out.truncate(limit);
    Ok((out, truncated))
}

/// Tracked names for the callee: its parameters at the positions of arguments with a non-empty `from`.
fn feed(sig: &Sig, args: &[ArgFlow]) -> Vec<String> {
    if args.len() > sig.names.len() && !sig.variadic {
        return Vec::new();
    }
    let mut v: Vec<String> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a.from.is_empty() {
            continue;
        }
        let p = if i < sig.names.len() { sig.names.get(i) } else { sig.names.last() };
        if let Some(p) = p.filter(|p| !p.is_empty()) {
            if !v.contains(p) {
                v.push(p.clone());
            }
        }
    }
    v
}
