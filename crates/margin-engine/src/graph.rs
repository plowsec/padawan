//! Name-based call graph from file outlines.
//!
//! | Table (CSR, keyed by interned name id) | Holds |
//! |---|---|
//! | `defs` | definitions `(file, line, col)` |
//! | `sites` | call sites of a callee `(file, line, col, caller)` |
//! | `edges` | distinct callee names of a function name (all its definitions merged) |
//! | `sinks` | direct sink calls of a function name `(sink name, count)` |
//! | `sink_sites` | the sink call sites, by caller, with the sink code |
//!
//! Reach: BFS over names, depth ≤ 8, ≤ 5_000 visited; top 32 sinks cached per name until the graph is rebuilt.
//! A lens row merges the cached tops of its callees (approximate order only when a callee reaches > 32 sinks).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use crate::index::FileRec;

/// Multiplicative hash for internal `u32` ids (sequential, not attacker-chosen hash values).
#[derive(Clone, Copy, Default)]
pub struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(b)).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }
    fn write_u32(&mut self, i: u32) {
        self.0 = (self.0 ^ u64::from(i)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

type IdBuild = std::hash::BuildHasherDefault<IdHasher>;
type IdMap<V> = HashMap<u32, V, IdBuild>;
type IdSet = HashSet<u32, IdBuild>;

pub const MAX_DEPTH: u32 = 8;
pub const MAX_VISITED: usize = 5_000;
pub const MAX_REACH_NAMES: usize = 5;
const MAX_CACHE: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loc {
    pub file: u32,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallLoc {
    pub at: Loc,
    pub caller: u32,
    pub callee: u32,
}

/// Compressed rows: `off[k]..off[k+1]` in `val`.
#[derive(Debug)]
struct Csr<T> {
    off: Vec<usize>,
    val: Vec<T>,
}

impl<T> Default for Csr<T> {
    fn default() -> Self {
        Csr { off: Vec::new(), val: Vec::new() }
    }
}

impl<T: Copy> Csr<T> {
    /// Counting placement: O(n), keeps the input order inside each row. Keys ≥ `n` are dropped.
    fn build(n: usize, pairs: &[(u32, T)]) -> Self {
        let mut off = vec![0usize; n + 1];
        for (k, _) in pairs {
            if let Some(o) = off.get_mut(*k as usize + 1) {
                *o += 1;
            }
        }
        for i in 0..n {
            off[i + 1] += off[i];
        }
        let Some(first) = pairs.first() else { return Csr { off, val: Vec::new() } };
        let mut val = vec![first.1; off[n]];
        let mut pos = off.clone();
        for &(k, v) in pairs {
            if let Some(p) = pos.get_mut(k as usize) {
                if let Some(slot) = val.get_mut(*p) {
                    *slot = v;
                    *p += 1;
                }
            }
        }
        Csr { off, val }
    }

    /// Rebuild with each row transformed.
    fn map_rows<U: Copy>(&self, mut f: impl FnMut(&[T], &mut Vec<U>)) -> Csr<U> {
        let n = self.off.len().saturating_sub(1);
        let mut off = Vec::with_capacity(n + 1);
        let mut val = Vec::new();
        off.push(0);
        let mut tmp = Vec::new();
        for k in 0..n {
            tmp.clear();
            f(self.row(k as u32), &mut tmp);
            val.extend_from_slice(&tmp);
            off.push(val.len());
        }
        Csr { off, val }
    }

    fn row(&self, k: u32) -> &[T] {
        let k = k as usize;
        match (self.off.get(k), self.off.get(k + 1)) {
            (Some(&a), Some(&b)) => self.val.get(a..b).unwrap_or_default(),
            _ => &[],
        }
    }
}

#[derive(Debug, Default)]
pub struct Graph {
    /// Paths of the files that have functions; `Loc::file` indexes this.
    paths: Vec<Box<str>>,
    names: Vec<Box<str>>,
    ids: HashMap<Box<str>, u32>,
    defs: Csr<Loc>,
    sites: Csr<CallLoc>,
    edges: Csr<u32>,
    sinks: Csr<(u32, u32)>,
    sink_sites: Csr<(CallLoc, u8)>,
    pub symbols: u32,
    /// Definitions + call sites (rebuild cost).
    pub size: usize,
    reach_cache: Mutex<IdMap<Reach>>,
}

/// Sinks reachable from one name: the `REACH_KEEP` most frequent (count desc, name), and whether
/// there were more. Lens rows merge these: exact when every list is complete, else approximate.
#[derive(Debug)]
struct ReachTop {
    top: Box<[(u32, u32)]>,
    more: bool,
}

type Reach = Arc<ReachTop>;
const REACH_KEEP: usize = 32;

/// A reachable sink call site with its chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReachSite {
    pub at: Loc,
    pub chain: String,
    pub sink_len: u32,
}

/// A call chain to one sink call site: `hops[k]` is a call inside function k; the last one is the sink call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawPath {
    pub hops: Vec<CallLoc>,
    /// `index::sink_code` of the sink.
    pub kind: u8,
}

impl Graph {
    pub fn build(files: &[Arc<FileRec>]) -> Graph {
        let mut g = Graph::default();
        let mut defs: Vec<(u32, Loc)> = Vec::new();
        let mut sites: Vec<(u32, CallLoc)> = Vec::new();
        let mut edges: Vec<(u32, u32)> = Vec::new();
        let mut sinks: Vec<(u32, u32)> = Vec::new();
        let mut sink_sites: Vec<(u32, (CallLoc, u8))> = Vec::new();
        for f in files.iter().filter(|f| !f.fns.is_empty()) {
            let file = g.paths.len() as u32;
            g.paths.push(f.path.clone());
            for d in f.fns.iter() {
                let caller = g.intern(&d.name);
                defs.push((caller, Loc { file, line: d.line, col: d.col }));
                for c in d.calls.iter() {
                    let callee = g.intern(&c.name);
                    let cl = CallLoc { at: Loc { file, line: c.line, col: c.col }, caller, callee };
                    sites.push((callee, cl));
                    if c.sink != 0 {
                        sinks.push((caller, callee));
                        sink_sites.push((caller, (cl, c.sink)));
                    } else {
                        edges.push((caller, callee));
                    }
                }
            }
        }
        let n = g.names.len();
        g.symbols = u32::try_from(defs.len()).unwrap_or(u32::MAX);
        g.size = defs.len() + sites.len();
        g.defs = Csr::build(n, &defs);
        g.sites = Csr::build(n, &sites);
        g.edges = Csr::build(n, &edges).map_rows(|r, out| {
            out.extend_from_slice(r);
            out.sort_unstable();
            out.dedup();
        });
        g.sinks = Csr::build(n, &sinks).map_rows(|r, out: &mut Vec<(u32, u32)>| {
            let mut v = r.to_vec();
            v.sort_unstable();
            for s in v {
                match out.last_mut() {
                    Some((x, k)) if *x == s => *k += 1,
                    _ => out.push((s, 1)),
                }
            }
        });
        g.sink_sites = Csr::build(n, &sink_sites);
        g
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.ids.get(s) {
            return i;
        }
        let i = self.names.len() as u32;
        self.names.push(s.into());
        self.ids.insert(s.into(), i);
        i
    }

    pub fn path(&self, file: u32) -> &str {
        self.paths.get(file as usize).map_or("", |s| s)
    }

    pub fn id(&self, s: &str) -> Option<u32> {
        self.ids.get(s).copied()
    }

    pub fn name(&self, id: u32) -> &str {
        self.names.get(id as usize).map_or("", |s| s)
    }

    pub fn defs(&self, name: &str) -> &[Loc] {
        self.id(name).map_or(&[], |i| self.defs.row(i))
    }

    pub fn callers(&self, name: &str) -> &[CallLoc] {
        self.id(name).map_or(&[], |i| self.sites.row(i))
    }

    fn defined(&self, id: u32) -> bool {
        !self.defs.row(id).is_empty()
    }

    /// Sinks reachable from `id`'s definitions (its own direct sinks included), with counts.
    fn reach_of(&self, id: u32) -> Reach {
        if let Ok(c) = self.reach_cache.lock() {
            if let Some(r) = c.get(&id) {
                return r.clone();
            }
        }
        let mut acc: IdMap<u32> = IdMap::default();
        let mut seen: IdSet = IdSet::default();
        let mut q: VecDeque<(u32, u32)> = VecDeque::new();
        seen.insert(id);
        q.push_back((id, 0));
        while let Some((v, d)) = q.pop_front() {
            for &(s, k) in self.sinks.row(v) {
                let e = acc.entry(s).or_insert(0);
                *e = e.saturating_add(k);
            }
            if d >= MAX_DEPTH {
                continue;
            }
            for &w in self.edges.row(v) {
                if seen.len() >= MAX_VISITED {
                    break;
                }
                if self.defined(w) && seen.insert(w) {
                    q.push_back((w, d + 1));
                }
            }
        }
        let mut v: Vec<(u32, u32)> = acc.into_iter().collect();
        let by = |a: &(u32, u32), b: &(u32, u32)| b.1.cmp(&a.1).then_with(|| self.name(a.0).cmp(self.name(b.0)));
        let more = v.len() > REACH_KEEP;
        if more {
            v.select_nth_unstable_by(REACH_KEEP, by);
            v.truncate(REACH_KEEP);
        }
        v.sort_by(by);
        let r: Reach = Arc::new(ReachTop { top: v.into(), more });
        if let Ok(mut c) = self.reach_cache.lock() {
            if c.len() >= MAX_CACHE {
                c.clear();
            }
            c.insert(id, r.clone());
        }
        r
    }

    /// Numbers for one lens row.
    pub fn lens(&self, name: &str, callees: &[String]) -> margin_proto::LensInfo {
        let id = self.id(name);
        let callers = id.map_or(0, |i| self.sites.row(i).len());
        let defs = id.map_or(0, |i| self.defs.row(i).len());
        let mut defined = 0u32;
        let mut acc: IdMap<u32> = IdMap::default();
        let mut done: IdSet = IdSet::default();
        let mut partial = false;
        for c in callees {
            let Some(cid) = self.id(c) else { continue };
            if !self.defined(cid) {
                continue;
            }
            defined += 1;
            if !done.insert(cid) {
                continue;
            }
            let r = self.reach_of(cid);
            partial |= r.more;
            for &(s, k) in r.top.iter() {
                let e = acc.entry(s).or_insert(0);
                *e = e.saturating_add(k);
            }
        }
        let mut v: Vec<(u32, &str)> = acc.into_iter().map(|(s, k)| (k, self.name(s))).collect();
        let by = |a: &(u32, &str), b: &(u32, &str)| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1));
        let reach_more = v.len() > MAX_REACH_NAMES || partial;
        if reach_more {
            v.select_nth_unstable_by(MAX_REACH_NAMES, by);
            v.truncate(MAX_REACH_NAMES);
        }
        v.sort_by(by);
        margin_proto::LensInfo {
            callers: u32::try_from(callers).unwrap_or(u32::MAX),
            defs: u32::try_from(defs).unwrap_or(u32::MAX),
            callees: defined,
            reach: v.into_iter().take(MAX_REACH_NAMES).map(|(_, n)| n.to_owned()).collect(),
            reach_more,
        }
    }

    /// Every sink call site reachable from `name` (its own included), with the call chain. At most `cap`.
    pub fn reach_sites(&self, name: &str, cap: usize) -> Vec<ReachSite> {
        let Some(id) = self.id(name) else { return Vec::new() };
        let mut parent: IdMap<u32> = IdMap::default();
        let mut q: VecDeque<(u32, u32)> = VecDeque::new();
        parent.insert(id, id);
        q.push_back((id, 0));
        let mut out = Vec::new();
        while let Some((v, d)) = q.pop_front() {
            let sites = self.sink_sites.row(v);
            if !sites.is_empty() {
                let chain = self.chain(v, &parent);
                for (s, _) in sites {
                    if out.len() >= cap {
                        return out;
                    }
                    let sink = self.name(s.callee);
                    out.push(ReachSite { at: s.at, chain: format!("{chain} → {sink}"), sink_len: sink.len() as u32 });
                }
            }
            if d >= MAX_DEPTH {
                continue;
            }
            for &w in self.edges.row(v) {
                if parent.len() >= MAX_VISITED {
                    break;
                }
                if self.defined(w) && !parent.contains_key(&w) {
                    parent.insert(w, v);
                    q.push_back((w, d + 1));
                }
            }
        }
        out
    }

    /// Shortest call chain from `name` to each sink call site (BFS as `reach_sites`), in BFS order.
    /// `sink` non-empty: only that sink. Each path: one site per hop, the last is the sink call.
    /// At most `cap`; `true` when more existed.
    pub fn sink_paths(&self, name: &str, sink: &str, cap: usize) -> (Vec<RawPath>, bool) {
        let Some(id) = self.id(name) else { return (Vec::new(), false) };
        let want = if sink.is_empty() {
            None
        } else {
            match self.id(sink) {
                Some(s) => Some(s),
                None => return (Vec::new(), false),
            }
        };
        let mut parent: IdMap<u32> = IdMap::default();
        let mut q: VecDeque<(u32, u32)> = VecDeque::new();
        let mut edge_memo: HashMap<(u32, u32), Option<CallLoc>> = HashMap::new();
        parent.insert(id, id);
        q.push_back((id, 0));
        let mut out = Vec::new();
        while let Some((v, d)) = q.pop_front() {
            let sites = self.sink_sites.row(v);
            if sites.iter().any(|(s, _)| want.is_none_or(|w| s.callee == w)) {
                let nodes = self.chain_ids(v, &parent);
                let mut prefix = Vec::with_capacity(nodes.len());
                let mut ok = true;
                for w in nodes.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let e = *edge_memo.entry((a, b)).or_insert_with(|| self.first_site(a, b));
                    match e {
                        Some(c) => prefix.push(c),
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok && prefix.len() < margin_proto::limits::MAX_HOPS {
                    for &(s, kind) in sites.iter().filter(|(s, _)| want.is_none_or(|w| s.callee == w)) {
                        if out.len() >= cap {
                            return (out, true);
                        }
                        let mut hops = prefix.clone();
                        hops.push(s);
                        out.push(RawPath { hops, kind });
                    }
                }
            }
            if d >= MAX_DEPTH {
                continue;
            }
            for &w in self.edges.row(v) {
                if parent.len() >= MAX_VISITED {
                    break;
                }
                if self.defined(w) && !parent.contains_key(&w) {
                    parent.insert(w, v);
                    q.push_back((w, d + 1));
                }
            }
        }
        (out, false)
    }

    /// First call of `callee` inside `caller`, in source order (path, line, col).
    fn first_site(&self, caller: u32, callee: u32) -> Option<CallLoc> {
        self.sites
            .row(callee)
            .iter()
            .filter(|c| c.caller == caller)
            .min_by(|a, b| self.path(a.at.file).cmp(self.path(b.at.file)).then(a.at.line.cmp(&b.at.line)).then(a.at.col.cmp(&b.at.col)))
            .copied()
    }

    /// Names from the BFS root to `v`.
    fn chain_ids(&self, mut v: u32, parent: &IdMap<u32>) -> Vec<u32> {
        let mut ids = vec![v];
        for _ in 0..=MAX_DEPTH {
            match parent.get(&v) {
                Some(&p) if p != v => {
                    ids.push(p);
                    v = p;
                }
                _ => break,
            }
        }
        ids.reverse();
        ids
    }

    fn chain(&self, mut v: u32, parent: &IdMap<u32>) -> String {
        let mut names = vec![self.name(v)];
        for _ in 0..=MAX_DEPTH {
            match parent.get(&v) {
                Some(&p) if p != v => {
                    names.push(self.name(p));
                    v = p;
                }
                _ => break,
            }
        }
        names.reverse();
        names.join(" → ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{CallSum, FnSum};

    fn call(name: &str, line: u32, sink: bool) -> CallSum {
        CallSum { name: name.into(), line, col: 4, sink: u8::from(sink) }
    }

    fn func(name: &str, line: u32, calls: Vec<CallSum>) -> FnSum {
        FnSum { name: name.into(), line, col: 3, calls: calls.into() }
    }

    fn file(path: &str, fns: Vec<FnSum>) -> Arc<FileRec> {
        Arc::new(FileRec { path: path.into(), size: 1, mtime_ns: 1, flags: 0, bloom: Box::default(), fns: fns.into() })
    }

    fn sample() -> Graph {
        Graph::build(&[
            file(
                "a.c",
                vec![
                    func("handler", 1, vec![call("parse", 2, false), call("run_cmd", 3, false), call("strcpy", 4, true)]),
                    func("run_cmd", 10, vec![call("system", 11, true), call("log", 12, false)]),
                ],
            ),
            file(
                "b.c",
                vec![
                    func("parse", 1, vec![call("memcpy", 2, true), call("memcpy", 3, true), call("parse", 4, false)]),
                    func("log", 9, vec![call("handler", 10, false)]),
                    func("main", 20, vec![call("handler", 21, false), call("undefined_fn", 22, false)]),
                ],
            ),
        ])
    }

    #[test]
    fn lens_numbers() {
        let g = sample();
        let li = g.lens("handler", &["parse".into(), "run_cmd".into(), "strcpy".into(), "nope".into()]);
        assert_eq!(li.callers, 2);
        assert_eq!(li.defs, 1);
        assert_eq!(li.callees, 2);
        // strcpy is direct: not in reach (unless reachable through a callee: log → handler → strcpy).
        assert_eq!(li.reach, vec!["memcpy", "strcpy", "system"]);
        assert!(!li.reach_more);
        assert_eq!(g.symbols, 5);
        let li = g.lens("unknown", &[]);
        assert_eq!((li.callers, li.defs, li.callees), (0, 0, 0));
    }

    #[test]
    fn callers_defs_and_chains() {
        let g = sample();
        let c = g.callers("handler");
        assert_eq!(c.len(), 2);
        assert_eq!(g.name(c[0].caller), "log");
        assert_eq!(g.defs("parse").len(), 1);
        let r = g.reach_sites("main", 100);
        let chains: Vec<&str> = r.iter().map(|s| s.chain.as_str()).collect();
        assert!(chains.contains(&"main → handler → strcpy"), "{chains:?}");
        assert!(chains.contains(&"main → handler → run_cmd → system"), "{chains:?}");
        assert!(chains.contains(&"main → handler → parse → memcpy"), "{chains:?}");
        assert_eq!(g.reach_sites("main", 2).len(), 2);
    }

    #[test]
    fn sink_paths_shortest_per_site() {
        let g = sample();
        let (p, more) = g.sink_paths("main", "", 100);
        assert!(!more);
        let chain = |r: &RawPath| r.hops.iter().map(|h| format!("{}>{}", g.name(h.caller), g.name(h.callee))).collect::<Vec<_>>().join(" ");
        let all: Vec<String> = p.iter().map(chain).collect();
        assert!(all.contains(&"main>handler handler>strcpy".to_owned()), "{all:?}");
        assert!(all.contains(&"main>handler handler>run_cmd run_cmd>system".to_owned()), "{all:?}");
        assert_eq!(all.iter().filter(|c| c.ends_with("parse>memcpy")).count(), 2, "one per sink site");
        let (p, _) = g.sink_paths("main", "system", 100);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].hops.last().map(|h| h.at.line), Some(11));
        assert_eq!(g.sink_paths("main", "nope", 100).0.len(), 0);
        let (p, more) = g.sink_paths("main", "", 1);
        assert!(more && p.len() == 1);
        // Direct sink: one hop.
        let (p, _) = g.sink_paths("handler", "strcpy", 10);
        assert_eq!(p[0].hops.len(), 1);
    }

    #[test]
    fn cycles_terminate_and_more_flag() {
        let mut fns = Vec::new();
        for i in 0..50 {
            fns.push(func(&format!("f{i}"), i, vec![call(&format!("f{}", (i + 1) % 50), i, false), call(&format!("sink{i}"), i, true)]));
        }
        let g = Graph::build(&[file("c.c", fns)]);
        let li = g.lens("x", &["f0".into()]);
        assert_eq!(li.reach.len(), MAX_REACH_NAMES);
        assert!(li.reach_more);
        assert!(g.reach_sites("f0", 10_000).len() <= 50);
    }
}
