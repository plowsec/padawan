//! Content search: literal or regex, parallel over files, streamed in batches, cancellable.
//!
//! | Query | Candidates | Verify |
//! |---|---|---|
//! | literal ≥ 3 bytes (ASCII or case-sensitive) | bloom has every folded trigram | `memmem` (on a folded copy if case-insensitive) |
//! | literal < 3 bytes | every searchable file | `memmem` |
//! | literal, case-insensitive, non-ASCII | every searchable file | regex (Unicode case folding) |
//! | regex | every searchable file | `regex::bytes`, size-limited |
//!
//! Caps: 2_000 hits per file, `max_hits` (≤ `MAX_ITEMS`) overall → `truncated`.
//! Batches: ~200 hits or ~30 ms. Cancel: checked per file and every 1024 matches.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use margin_proto::limits::MAX_ITEMS;
use margin_proto::{ErrCode, RelPath, SearchQuery, SearchStats, Site};
use memchr::memmem;
use rayon::prelude::*;

use crate::bloom;
use crate::glob::GlobList;
use crate::index::{content_cap, FileRec};
use crate::jail::{is_binary, open_regular, read_capped};
use crate::snippet::make_site;

pub const PER_FILE: usize = 2_000;
const BATCH_HITS: usize = 200;
const BATCH_TIME: Duration = Duration::from_millis(30);
const MAX_ERR: usize = 400;

enum Matcher {
    Lit { finder: memmem::Finder<'static>, fold: bool, word: bool, len: usize, first_word: bool, last_word: bool },
    Re(regex::bytes::Regex),
}

pub struct Plan {
    m: Matcher,
    /// Bloom prefilter.
    tris: Option<Vec<u32>>,
    include: GlobList,
    exclude: GlobList,
    max_hits: usize,
    empty: bool,
}

fn word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn trim_msg(s: String) -> String {
    if s.len() <= MAX_ERR {
        return s;
    }
    let mut cut = MAX_ERR;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

fn build_re(pat: &str, ci: bool) -> Result<regex::bytes::Regex, (ErrCode, String)> {
    regex::bytes::RegexBuilder::new(pat)
        .case_insensitive(ci)
        .multi_line(true)
        .size_limit(16 << 20)
        .dfa_size_limit(32 << 20)
        .nest_limit(64)
        .build()
        .map_err(|e| (ErrCode::Invalid, trim_msg(format!("bad regex: {e}"))))
}

pub fn plan(q: &SearchQuery) -> Result<Plan, (ErrCode, String)> {
    let max_hits = if q.max_hits == 0 { MAX_ITEMS } else { (q.max_hits as usize).min(MAX_ITEMS) };
    let include = GlobList::parse(&q.include);
    let exclude = GlobList::parse(&q.exclude);
    let empty = q.pattern.is_empty();
    let ci = !q.case_sensitive;
    let (m, tris) = if q.regex {
        let p = if q.whole_word { format!(r"\b(?:{})\b", q.pattern) } else { q.pattern.clone() };
        (Matcher::Re(build_re(&p, ci)?), None)
    } else if ci && !q.pattern.is_ascii() {
        let e = regex::escape(&q.pattern);
        let p = if q.whole_word { format!(r"\b(?:{e})\b") } else { e };
        (Matcher::Re(build_re(&p, true)?), None)
    } else {
        let pb = q.pattern.as_bytes();
        let needle: Vec<u8> = if ci { pb.iter().map(|b| b.to_ascii_lowercase()).collect() } else { pb.to_vec() };
        let tris = (pb.len() >= 3).then(|| bloom::trigrams(pb));
        let m = Matcher::Lit {
            finder: memmem::Finder::new(&needle).into_owned(),
            fold: ci,
            word: q.whole_word,
            len: pb.len(),
            first_word: pb.first().is_some_and(|&b| word_byte(b)),
            last_word: pb.last().is_some_and(|&b| word_byte(b)),
        };
        (m, tris)
    };
    Ok(Plan { m, tris, include, exclude, max_hits, empty })
}

impl Plan {
    fn wants(&self, f: &FileRec) -> bool {
        if !f.searchable() {
            return false;
        }
        if !self.include.is_empty() && !self.include.matches_path(&f.path) {
            return false;
        }
        if !self.exclude.is_empty() && self.exclude.matches_path(&f.path) {
            return false;
        }
        match &self.tris {
            Some(t) if f.indexed() => bloom::may_contain(&f.bloom, t),
            _ => true,
        }
    }

    /// Match ranges in `data` (up to `cap`). `false` if cancelled.
    fn matches(&self, data: &[u8], folded: &mut Vec<u8>, cap: usize, cancel: &AtomicBool, out: &mut Vec<(usize, usize)>) -> bool {
        match &self.m {
            Matcher::Lit { finder, fold, word, len, first_word, last_word } => {
                let hay: &[u8] = if *fold {
                    folded.clear();
                    folded.extend(data.iter().map(|b| b.to_ascii_lowercase()));
                    folded
                } else {
                    data
                };
                let mut pos = 0usize;
                let mut n = 0usize;
                while let Some(i) = hay.get(pos..).and_then(|h| finder.find(h)) {
                    let s = pos + i;
                    let e = s + len;
                    pos = s + (*len).max(1);
                    n += 1;
                    if n.is_multiple_of(1024) && cancel.load(Ordering::Relaxed) {
                        return false;
                    }
                    if *word {
                        let before = s.checked_sub(1).and_then(|j| data.get(j)).is_some_and(|&b| word_byte(b));
                        let after = data.get(e).is_some_and(|&b| word_byte(b));
                        if (*first_word && before) || (*last_word && after) {
                            continue;
                        }
                    }
                    out.push((s, e));
                    if out.len() >= cap {
                        break;
                    }
                }
            }
            Matcher::Re(re) => {
                for (n, m) in re.find_iter(data).enumerate() {
                    if n % 1024 == 1023 && cancel.load(Ordering::Relaxed) {
                        return false;
                    }
                    if m.start() == m.end() {
                        continue;
                    }
                    out.push((m.start(), m.end()));
                    if out.len() >= cap {
                        break;
                    }
                }
            }
        }
        true
    }
}

/// Ranges → sites (line numbers computed incrementally).
pub fn sites_for(path: &RelPath, data: &[u8], ranges: &[(usize, usize)]) -> Vec<Site> {
    let mut out = Vec::with_capacity(ranges.len());
    let (mut line_no, mut scanned) = (0u32, 0usize);
    let mut line_start = 0usize;
    for &(s, e) in ranges {
        let s = s.min(data.len());
        if s < scanned {
            continue;
        }
        for nl in memchr::memchr_iter(b'\n', data.get(scanned..s).unwrap_or_default()) {
            line_no = line_no.saturating_add(1);
            line_start = scanned + nl + 1;
        }
        scanned = s;
        let line_end = memchr::memchr(b'\n', data.get(s..).unwrap_or_default()).map_or(data.len(), |i| s + i);
        let line = data.get(line_start..line_end).unwrap_or_default();
        out.push(make_site(path, line_no, line, s - line_start, e.saturating_sub(s), String::new()));
    }
    out
}

struct Batcher<'a> {
    buf: Vec<Site>,
    last: Instant,
    hits: usize,
    max: usize,
    truncated: bool,
    emit: &'a (dyn Fn(Vec<Site>) + Sync),
}

impl Batcher<'_> {
    /// Returns false once `max` is reached.
    fn add(&mut self, mut sites: Vec<Site>) -> bool {
        let room = self.max.saturating_sub(self.hits);
        if sites.len() >= room {
            if sites.len() > room {
                self.truncated = true;
            }
            sites.truncate(room);
        }
        self.hits += sites.len();
        self.buf.append(&mut sites);
        if self.buf.len() >= BATCH_HITS || self.last.elapsed() >= BATCH_TIME {
            self.flush();
        }
        self.hits < self.max
    }

    fn flush(&mut self) {
        if !self.buf.is_empty() {
            (self.emit)(std::mem::take(&mut self.buf));
        }
        self.last = Instant::now();
    }
}

thread_local! {
    static BUFS: std::cell::RefCell<(Vec<u8>, Vec<u8>)> = const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
}

/// Run a search. `emit` gets batches in order. `Err(Cancelled)` if `cancel` was set.
pub fn run(
    pool: &rayon::ThreadPool,
    root: &Path,
    files: &[Arc<FileRec>],
    plan: &Plan,
    max_file_bytes: u64,
    cancel: &AtomicBool,
    emit: &(dyn Fn(Vec<Site>) + Sync),
) -> Result<SearchStats, ErrCode> {
    let t0 = Instant::now();
    if plan.empty {
        return Ok(SearchStats::default());
    }
    let cands: Vec<&Arc<FileRec>> = pool.install(|| files.par_iter().filter(|f| plan.wants(f)).collect());
    let scanned = AtomicU32::new(0);
    let matched = AtomicU32::new(0);
    let full = AtomicBool::new(false);
    let left_out = AtomicBool::new(false);
    let per_file_cut = AtomicBool::new(false);
    let batch = Mutex::new(Batcher { buf: Vec::new(), last: Instant::now(), hits: 0, max: plan.max_hits, truncated: false, emit });
    let cap = content_cap(max_file_bytes);
    pool.install(|| {
        cands.par_iter().with_max_len(16).for_each(|f| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            if full.load(Ordering::Relaxed) {
                left_out.store(true, Ordering::Relaxed);
                return;
            }
            let Ok(rel) = RelPath::new(&f.path) else { return };
            let sites = BUFS.with(|b| {
                let mut b = b.borrow_mut();
                let (data, folded) = &mut *b;
                {
                    let _slot = crate::jail::io_slot();
                    let Ok((mut fh, md)) = open_regular(&rel.under(root)) else { return None };
                    if read_capped(&mut fh, md.len(), cap, data).is_err() {
                        return None;
                    }
                }
                scanned.fetch_add(1, Ordering::Relaxed);
                if f.flags & crate::index::F_PENDING != 0 && is_binary(data) {
                    return None;
                }
                let mut ranges = Vec::new();
                if !plan.matches(data, folded, PER_FILE, cancel, &mut ranges) || ranges.is_empty() {
                    return None;
                }
                if ranges.len() >= PER_FILE {
                    per_file_cut.store(true, Ordering::Relaxed);
                }
                let s = sites_for(&rel, data, &ranges);
                if data.capacity() > (8 << 20) {
                    *data = Vec::new();
                    *folded = Vec::new();
                }
                Some(s)
            });
            let Some(sites) = sites else { return };
            if sites.is_empty() || cancel.load(Ordering::Relaxed) {
                return;
            }
            matched.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut b) = batch.lock() {
                if !b.add(sites) {
                    full.store(true, Ordering::Relaxed);
                }
            }
        })
    });
    if cancel.load(Ordering::Relaxed) {
        return Err(ErrCode::Cancelled);
    }
    let mut b = batch.into_inner().unwrap_or_else(|e| e.into_inner());
    b.flush();
    Ok(SearchStats {
        files_scanned: scanned.into_inner(),
        files_matched: matched.into_inner(),
        hits: u32::try_from(b.hits).unwrap_or(u32::MAX),
        truncated: b.truncated || per_file_cut.into_inner() || left_out.into_inner(),
        elapsed_ms: u32::try_from(t0.elapsed().as_millis()).unwrap_or(u32::MAX),
    })
}
