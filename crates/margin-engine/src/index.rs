//! File table entries and per-file indexing (bloom + outline summary).
//!
//! | Flag | Meaning |
//! |---|---|
//! | `BIG` | over `max_file_bytes`: in the finder, content not indexed / searched |
//! | `BINARY` | NUL in the first 8 KiB |
//! | `PENDING` | listed by the walk, content not indexed yet (search scans it) |
//! | `UNREADABLE` | open / read failed (not a regular file any more, permissions) |

use std::path::Path;

use margin_code::{Lang, Outline, SinkKind};
use margin_proto::limits::MAX_OPEN_FILE;

use crate::bloom;
use crate::jail::{is_binary, meta_of, open_regular, read_capped};

pub const F_BIG: u8 = 1;
pub const F_BINARY: u8 = 2;
pub const F_PENDING: u8 = 4;
pub const F_UNREADABLE: u8 = 8;
pub const F_ALL: u8 = F_BIG | F_BINARY | F_PENDING | F_UNREADABLE;

/// Caps on one file's outline summary.
pub const MAX_FNS: usize = 20_000;
pub const MAX_CALLS: usize = 20_000;
pub const MAX_SYMBOL: usize = 256;
/// Calls kept per file, all functions together.
pub const MAX_FILE_CALLS: usize = 200_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSum {
    pub name: Box<str>,
    pub line: u32,
    pub col: u32,
    /// 0: not a sink, else `sink_code`.
    pub sink: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnSum {
    pub name: Box<str>,
    pub line: u32,
    pub col: u32,
    pub calls: Box<[CallSum]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRec {
    /// Relative path (valid `RelPath` text).
    pub path: Box<str>,
    pub size: u64,
    pub mtime_ns: u64,
    pub flags: u8,
    pub bloom: Box<[u8]>,
    pub fns: Box<[FnSum]>,
}

impl FileRec {
    pub fn pending(path: Box<str>, size: u64, mtime_ns: u64) -> Self {
        Self { path, size, mtime_ns, flags: F_PENDING, bloom: Box::default(), fns: Box::default() }
    }

    /// Content may be searched (indexed, or not yet indexed).
    pub fn searchable(&self) -> bool {
        self.flags & (F_BIG | F_BINARY | F_UNREADABLE) == 0
    }

    pub fn indexed(&self) -> bool {
        self.flags & F_ALL == 0
    }

    /// Bytes of indexed content.
    pub fn content_bytes(&self) -> u64 {
        if self.indexed() {
            self.size
        } else {
            0
        }
    }

    /// Size class still matches the option (a cached entry is reusable).
    pub fn reusable(&self, size: u64, mtime_ns: u64, max_file_bytes: u64) -> bool {
        self.size == size && self.mtime_ns == mtime_ns && self.flags & (F_PENDING | F_UNREADABLE) == 0 && ((self.flags & F_BIG != 0) == (size > max_file_bytes))
    }
}

pub fn sink_code(k: Option<SinkKind>) -> u8 {
    match k {
        None => 0,
        Some(SinkKind::Exec) => 1,
        Some(SinkKind::Memory) => 2,
        Some(SinkKind::Eval) => 3,
        Some(SinkKind::Deserialize) => 4,
        Some(SinkKind::Sql) => 5,
        Some(SinkKind::File) => 6,
        Some(SinkKind::Format) => 7,
        Some(SinkKind::Network) => 8,
        Some(SinkKind::Html) => 9,
    }
}

/// Read cap for indexing and search.
pub fn content_cap(max_file_bytes: u64) -> usize {
    usize::try_from(max_file_bytes).unwrap_or(usize::MAX).min(MAX_OPEN_FILE)
}

/// Outline → compact summary, capped.
pub fn summarize(o: Outline) -> Box<[FnSum]> {
    let mut budget = MAX_FILE_CALLS;
    o.fns
        .into_iter()
        .filter(|f| !f.name.is_empty() && f.name.len() <= MAX_SYMBOL)
        .take(MAX_FNS)
        .map(|f| {
            let calls: Box<[CallSum]> = f
                .calls
                .into_iter()
                .filter(|c| !c.name.is_empty() && c.name.len() <= MAX_SYMBOL)
                .take(MAX_CALLS.min(budget))
                .map(|c| CallSum { name: c.name.into(), line: c.line, col: c.col, sink: sink_code(c.sink) })
                .collect();
            budget -= calls.len();
            FnSum { name: f.name.into(), line: f.line, col: f.col, calls }
        })
        .collect()
}

/// Index one walked file. `abs` is the lexical path under the root (walker-checked: no symlink inside).
pub fn index_file(abs: &Path, rel: &str, size: u64, mtime_ns: u64, max_file_bytes: u64, buf: &mut Vec<u8>) -> FileRec {
    let mut rec = FileRec { path: rel.into(), size, mtime_ns, flags: 0, bloom: Box::default(), fns: Box::default() };
    if size > max_file_bytes {
        rec.flags = F_BIG;
        return rec;
    }
    let slot = crate::jail::io_slot();
    let Ok((mut f, md)) = open_regular(abs) else {
        rec.flags = F_UNREADABLE;
        return rec;
    };
    let m = meta_of(&md);
    rec.size = m.size;
    rec.mtime_ns = m.mtime_ns;
    if m.size > max_file_bytes {
        rec.flags = F_BIG;
        return rec;
    }
    let res = read_capped(&mut f, m.size, content_cap(max_file_bytes), buf);
    drop(f);
    drop(slot);
    match res {
        Ok(()) => {}
        Err(e) if e.code == margin_proto::ErrCode::TooLarge => {
            rec.flags = F_BIG;
            return rec;
        }
        Err(_) => {
            rec.flags = F_UNREADABLE;
            return rec;
        }
    }
    if is_binary(buf) {
        rec.flags = F_BINARY;
        return rec;
    }
    rec.bloom = bloom::build(buf);
    let lang = Lang::from_path(rel);
    if lang.has_outline() {
        let text = String::from_utf8_lossy(buf);
        rec.fns = summarize(margin_code::outline(lang, &text));
    }
    rec
}
