//! Fuzzy path matcher for `FindFiles` (fzf-style scoring).
//!
//! | Term | Score |
//! |---|---|
//! | matched char | +16 |
//! | after `/` | +9 ; after `_ - . space` +8 ; camelCase hump / digit start +7 ; path start +10 |
//! | first query char | its bonus × 2 |
//! | consecutive run | ≥ +4, keeps the run's first bonus |
//! | gap | −3 start, −1 per extra char |
//! | char inside the base name | +4 ; first char at base-name start +12 |
//! | path length | −len/10 |
//!
//! Smart case: case-insensitive (ASCII) unless the query has an uppercase char.
//! Candidates per path: leftmost greedy, rightmost greedy, base name only; each tightened backward; best wins.

use std::cmp::Ordering;
use std::sync::Arc;

use margin_proto::limits::MAX_ITEMS;
use margin_proto::{FileMatch, RelPath};
use rayon::prelude::*;

use crate::glob::{FileFilter, PathGlob};
use crate::index::FileRec;

const MATCH: i32 = 16;
const GAP_START: i32 = -3;
const GAP_EXT: i32 = -1;
const B_START: i32 = 10;
const B_SLASH: i32 = 9;
const B_DELIM: i32 = 8;
const B_CAMEL: i32 = 7;
const B_CONSEC: i32 = 4;
const B_BASE: i32 = 4;
const B_BASE_START: i32 = 12;

pub const MAX_LIMIT: usize = 1000;
const CHUNK: usize = 8192;

pub struct Query {
    pat: Vec<u8>,
    fold: bool,
}

impl Query {
    pub fn new(q: &str) -> Query {
        let fold = !q.chars().any(char::is_uppercase);
        let s: String = q.chars().filter(|c| !c.is_whitespace()).collect();
        let pat = if fold { s.to_lowercase().into_bytes() } else { s.into_bytes() };
        Query { pat, fold }
    }

    pub fn is_empty(&self) -> bool {
        self.pat.is_empty()
    }
}

#[inline]
fn class(b: u8) -> u8 {
    match b {
        b'a'..=b'z' => 1,
        b'A'..=b'Z' => 2,
        b'0'..=b'9' => 3,
        b'/' => 4,
        b'_' | b'-' | b'.' | b' ' => 5,
        0x80..=0xff => 1,
        _ => 6,
    }
}

#[inline]
fn bonus(path: &[u8], i: usize) -> i32 {
    let Some(&cur) = path.get(i) else { return 0 };
    if i == 0 {
        return B_START;
    }
    let prev = path.get(i - 1).copied().unwrap_or(b'/');
    let (p, c) = (class(prev), class(cur));
    match (p, c) {
        (4, _) => B_SLASH,
        (5 | 6, 1..=3) => B_DELIM,
        (1, 2) => B_CAMEL,
        (1 | 2, 3) => B_CAMEL,
        _ => 0,
    }
}

#[inline]
fn eq(q: u8, p: u8, fold: bool) -> bool {
    if fold {
        q == p.to_ascii_lowercase()
    } else {
        q == p
    }
}

/// Forward greedy from `from` → end position (exclusive) of the first complete match.
fn forward(pat: &[u8], path: &[u8], from: usize, fold: bool) -> Option<usize> {
    let mut qi = 0;
    for (i, &b) in path.iter().enumerate().skip(from) {
        if eq(pat[qi], b, fold) {
            qi += 1;
            if qi == pat.len() {
                return Some(i + 1);
            }
        }
    }
    None
}

/// Backward greedy within `path[lo..end]`: rightmost positions, written to `pos`.
fn backward(pat: &[u8], path: &[u8], lo: usize, end: usize, fold: bool, pos: &mut Vec<u32>) -> bool {
    pos.clear();
    pos.resize(pat.len(), 0);
    let mut qi = pat.len();
    let mut i = end;
    while i > lo && qi > 0 {
        i -= 1;
        if eq(pat[qi - 1], path[i], fold) {
            qi -= 1;
            pos[qi] = i as u32;
        }
    }
    qi == 0
}

fn score_positions(path: &[u8], pos: &[u32], base: usize) -> i32 {
    let mut score = 0i32;
    let mut prev: Option<usize> = None;
    let mut run_bonus = 0;
    for (k, &p) in pos.iter().enumerate() {
        let p = p as usize;
        let b = bonus(path, p);
        let add = match prev {
            Some(q) if p == q + 1 => {
                run_bonus = run_bonus.max(b);
                b.max(run_bonus).max(B_CONSEC)
            }
            Some(q) => {
                let gap = (p - q - 1) as i32;
                score += GAP_START + GAP_EXT * (gap - 1).min(64);
                run_bonus = b;
                b
            }
            None => {
                run_bonus = b;
                b * 2
            }
        };
        score += MATCH + add;
        if p >= base {
            score += B_BASE;
            if k == 0 && p == base {
                score += B_BASE_START;
            }
        }
        prev = Some(p);
    }
    score - (path.len() / 10) as i32
}

/// Best (score, positions) for one path, or `None`.
pub fn score(q: &Query, path: &str, pos: &mut Vec<u32>) -> Option<i32> {
    let pat = &q.pat;
    let p = path.as_bytes();
    if pat.is_empty() || pat.len() > p.len() {
        return None;
    }
    let base = p.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
    let end = forward(pat, p, 0, q.fold)?;
    let mut best: Option<(i32, Vec<u32>)> = None;
    let mut tmp = Vec::with_capacity(pat.len());
    let mut try_cand = |lo: usize, end: usize, tmp: &mut Vec<u32>| {
        if backward(pat, p, lo, end, q.fold, tmp) {
            let s = score_positions(p, tmp, base);
            if best.as_ref().is_none_or(|(b, _)| s > *b) {
                best = Some((s, tmp.clone()));
            }
        }
    };
    // Leftmost complete match, tightened.
    try_cand(0, end, &mut tmp);
    // Rightmost match.
    try_cand(0, p.len(), &mut tmp);
    // Base name only, from its first possible start.
    if base > 0 {
        if let Some(e) = forward(pat, p, base, q.fold) {
            try_cand(base, e, &mut tmp);
        }
    }
    let (s, v) = best?;
    pos.clear();
    pos.extend(v.into_iter().filter(|&i| path.is_char_boundary(i as usize)));
    Some(s)
}

fn order(a: &(i32, u32), b: &(i32, u32), files: &[Arc<FileRec>]) -> Ordering {
    b.0.cmp(&a.0).then_with(|| {
        let (pa, pb) = (&files[a.1 as usize].path, &files[b.1 as usize].path);
        pa.len().cmp(&pb.len()).then_with(|| pa.cmp(pb))
    })
}

fn top_k(mut v: Vec<(i32, u32)>, k: usize, files: &[Arc<FileRec>]) -> Vec<(i32, u32)> {
    if v.len() > k {
        v.select_nth_unstable_by(k, |a, b| order(a, b, files));
        v.truncate(k);
    }
    v
}

/// `FindFiles`: `filter` first, then by query (see `Request::FindFiles`). `limit` clamped to `1..=MAX_ITEMS`.
///
/// | Query | Result |
/// |---|---|
/// | empty | every file passing the filter, by path |
/// | glob (`*?[{`) | `PathGlob` on the path (name alone without `/`), by path, no positions |
/// | else | fuzzy, best first |
pub fn find_files(pool: &rayon::ThreadPool, files: &[Arc<FileRec>], query: &str, filter: &FileFilter, limit: usize) -> Vec<FileMatch> {
    let k = limit.clamp(1, MAX_ITEMS);
    let pass = |f: &FileRec| filter.is_empty() || filter.pass(&f.path);
    if query.trim().is_empty() || PathGlob::is_glob(query) {
        let glob = PathGlob::is_glob(query).then(|| PathGlob::parse(query));
        let hits: Vec<Vec<&Arc<FileRec>>> = pool.install(|| {
            files
                .par_chunks(CHUNK)
                .map(|chunk| {
                    let mut comps = Vec::new();
                    let mut v = Vec::new();
                    for f in chunk {
                        if v.len() >= k {
                            break;
                        }
                        if pass(f) && glob.as_ref().is_none_or(|g| g.matches(&f.path, &mut comps)) {
                            v.push(f);
                        }
                    }
                    v
                })
                .collect()
        });
        return hits.into_iter().flatten().take(k).filter_map(|f| mk(f, 0, Vec::new())).collect();
    }
    find_by(pool, files, &Query::new(query), k, &pass)
}

/// Top `limit` matches (clamped to 1000). Empty query: most recently modified first.
pub fn find(pool: &rayon::ThreadPool, files: &[Arc<FileRec>], query: &str, limit: u32) -> Vec<FileMatch> {
    let k = (limit as usize).clamp(1, MAX_LIMIT);
    let q = Query::new(query);
    if q.is_empty() {
        let mut idx: Vec<u32> = (0..files.len() as u32).collect();
        let cmp = |a: &u32, b: &u32| {
            let (fa, fb) = (&files[*a as usize], &files[*b as usize]);
            fb.mtime_ns.cmp(&fa.mtime_ns).then_with(|| fa.path.cmp(&fb.path))
        };
        if idx.len() > k {
            idx.select_nth_unstable_by(k, cmp);
            idx.truncate(k);
        }
        idx.sort_unstable_by(cmp);
        return idx.into_iter().filter_map(|i| mk(&files[i as usize], 0, Vec::new())).collect();
    }
    find_by(pool, files, &q, k, &|_| true)
}

/// Fuzzy top `k` among the files `keep` accepts.
fn find_by(pool: &rayon::ThreadPool, files: &[Arc<FileRec>], q: &Query, k: usize, keep: &(dyn Fn(&FileRec) -> bool + Sync)) -> Vec<FileMatch> {
    if q.is_empty() {
        return Vec::new();
    }
    let hits: Vec<(i32, u32)> = pool.install(|| {
        files
            .par_chunks(CHUNK)
            .enumerate()
            .map(|(c, chunk)| {
                let mut pos = Vec::new();
                let v: Vec<(i32, u32)> = chunk
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| keep(f))
                    .filter_map(|(i, f)| score(q, &f.path, &mut pos).map(|s| (s, (c * CHUNK + i) as u32)))
                    .collect();
                top_k(v, k, files)
            })
            .reduce(Vec::new, |mut a, b| {
                a.extend(b);
                top_k(a, k, files)
            })
    });
    let mut hits = top_k(hits, k, files);
    hits.sort_unstable_by(|a, b| order(a, b, files));
    let mut pos = Vec::new();
    hits.into_iter()
        .filter_map(|(s, i)| {
            let f = &files[i as usize];
            score(q, &f.path, &mut pos)?;
            mk(f, s, pos.clone())
        })
        .collect()
}

fn mk(f: &FileRec, score: i32, positions: Vec<u32>) -> Option<FileMatch> {
    Some(FileMatch { path: RelPath::new(&f.path).ok()?, score, positions })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank(paths: &[&str], q: &str) -> Vec<String> {
        let files: Vec<Arc<FileRec>> = paths.iter().map(|p| Arc::new(FileRec::pending((*p).into(), 1, 0))).collect();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        find(&pool, &files, q, 10).into_iter().map(|m| m.path.to_string()).collect()
    }

    #[test]
    fn base_name_beats_directory() {
        let r = rank(&["main/other.rs", "src/x/main.rs"], "main");
        assert_eq!(r[0], "src/x/main.rs");
    }

    #[test]
    fn boundary_bonus() {
        let r = rank(&["src/abcfoo.rs", "src/abc_foo.rs"], "foo");
        assert_eq!(r[0], "src/abc_foo.rs");
        let r = rank(&["a/getxuser.rs", "a/getUser.rs"], "gu");
        assert_eq!(r[0], "a/getUser.rs");
    }

    #[test]
    fn smart_case_and_subsequence() {
        assert_eq!(rank(&["Readme.md", "readme.md"], "R"), vec!["Readme.md"]);
        assert_eq!(rank(&["Readme.md", "readme.md"], "r").len(), 2);
        assert!(rank(&["abc"], "acb").is_empty());
    }

    #[test]
    fn positions_valid() {
        let files: Vec<Arc<FileRec>> = ["src/édit/Main.rs", "a/b/c/main_test.rs"].iter().map(|p| Arc::new(FileRec::pending((*p).into(), 1, 0))).collect();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        for q in ["main", "srm", "é", "édm"] {
            for m in find(&pool, &files, q, 10) {
                let s = m.path.as_str();
                let mut last = None;
                for &p in &m.positions {
                    assert!((p as usize) < s.len() && s.is_char_boundary(p as usize));
                    assert!(last.is_none_or(|l| p > l));
                    last = Some(p);
                }
            }
        }
    }

    #[test]
    fn empty_query_recent_first() {
        let files: Vec<Arc<FileRec>> = vec![Arc::new(FileRec::pending("old".into(), 1, 5)), Arc::new(FileRec::pending("new".into(), 1, 9))];
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let r = find(&pool, &files, "", 10);
        assert_eq!(r[0].path.as_str(), "new");
    }
}
