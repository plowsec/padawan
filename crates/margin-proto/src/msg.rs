//! Messages. GUI → engine: `(ReqId, Request)`. Engine → GUI: `Out` (a reply to an id, or an event).
//!
//! Conventions: lines are 0-based; columns and lengths are byte offsets in the line's UTF-8;
//! times are nanoseconds since the Unix epoch; every list is capped (`limits`).

use crate::limits::{MAX_ARGS, MAX_ARG_TEXT, MAX_DIR_ENTRIES, MAX_HOPS, MAX_ITEMS, MAX_NAME, MAX_OPEN_FILE, MAX_SNIPPET, MAX_TEXT};
use crate::path::RelPath;
use crate::wire::{Dec, Enc, WireError, WireResult};

pub type ReqId = u64;

/// What the index covers. Sent at start-up and with `Reindex`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexOpts {
    /// Honour `.gitignore` files (ignored files stay visible in the tree, greyed; not indexed, not searched).
    pub respect_gitignore: bool,
    /// Index dot-files and dot-directories (`.git` is always skipped).
    pub index_hidden: bool,
    /// Files above this size are listed but their content is not indexed or searched.
    pub max_file_bytes: u64,
}

impl Default for IndexOpts {
    fn default() -> Self {
        Self { respect_gitignore: true, index_hidden: true, max_file_bytes: 2 << 20 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub pattern: String,
    /// `pattern` is a regex (Rust `regex` syntax); else a literal.
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Comma-separated globs on the relative path (`*.rs, src/**`); empty = all.
    pub include: String,
    /// Comma-separated globs to leave out; empty = none.
    pub exclude: String,
    /// Stop after this many hits (engine clamps to `MAX_ITEMS`).
    pub max_hits: u32,
}

/// One function of the file on screen: its name and the distinct names it calls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LensFn {
    pub name: String,
    pub callees: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// → `Dir`. Directories first, then case-insensitive by name.
    ListDir { path: RelPath },
    /// → `File`. Over `MAX_OPEN_FILE` → `Err(TooLarge)`.
    ReadFile { path: RelPath },
    /// → `Meta` (new metadata). Atomic (temp file + rename in the same directory).
    /// `expect_mtime_ns`: fail with `Conflict` if the file on disk has another mtime.
    /// `create_new`: fail with `Exists` if the path exists.
    WriteFile { path: RelPath, data: Vec<u8>, expect_mtime_ns: Option<u64>, create_new: bool },
    /// → `Done`.
    CreateDir { path: RelPath },
    /// → `Done`. Fails with `Exists` if `to` exists.
    Rename { from: RelPath, to: RelPath },
    /// → `Done`. A file, a symlink (the link itself) or an empty directory. Never recursive.
    Delete { path: RelPath },
    /// → `Meta`.
    Stat { path: RelPath },
    /// File-name search → `FileMatches`.
    ///
    /// | `query` | Match | Order |
    /// |---|---|---|
    /// | empty | every file | by path |
    /// | has `*`, `?`, `[` or `{` | glob on the relative path (on the name alone when it has no `/`) | by path |
    /// | else | fuzzy on the relative path | best first |
    ///
    /// `include` / `exclude`: comma-separated globs like `SearchQuery`; a file must pass both. `!glob` in `include` excludes.
    FindFiles { query: String, include: String, exclude: String, limit: u32 },
    /// Content search → zero or more `SearchBatch`, then one `SearchDone` (or `Err`).
    Search { query: SearchQuery },
    /// Stop request `target`. Its stream ends with `Err(Cancelled)` or its normal end. No reply to the cancel itself.
    Cancel { target: ReqId },
    /// Numbers for the hint line above each function → `Lens`, same order as `fns`.
    Lens { fns: Vec<LensFn> },
    /// Call sites of `name` across the folder → `Sites` (`context` = calling function).
    Callers { name: String, limit: u32 },
    /// Definitions of `name` → `Sites`.
    Defs { name: String, limit: u32 },
    /// Sinks reachable from function `name` through calls → `Sites`
    /// (one per sink call site; `context` = call chain `a → b → memcpy`).
    Reach { name: String, limit: u32 },
    /// Call paths from function `name` to sink call sites → `Paths`. Shortest path per sink site.
    /// `sink`: only this sink name; empty = every sink. Direct sink calls in `name` are paths of one hop.
    Paths { name: String, sink: String, limit: u32 },
    /// Replace every match of `query` → `Replaced`. Same matching and file filters as `Search`.
    ///
    /// | Field | Meaning |
    /// |---|---|
    /// | `replacement` | literal text; with `query.regex`: `$1`, `${name}`, `$$` expand (Rust `regex` syntax) |
    /// | `only` | non-empty: just these files |
    /// | `skip` | never touch these (unsaved in the editor) |
    ///
    /// Each file is rewritten atomically; one that changed while being processed is skipped and reported.
    /// Cancellable: files already rewritten stay rewritten. `query.max_hits` does not apply.
    Replace { query: SearchQuery, replacement: String, only: Vec<RelPath>, skip: Vec<RelPath> },
    /// Re-scan the folder (changed files only) with these options. Progress comes as `Event::Index`. → `Done` at once.
    Reindex { opts: IndexOpts },
    /// Engine exits. No reply.
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrCode {
    NotFound,
    /// Outside the folder, through a symlink that leaves it, or not a regular file / directory.
    Denied,
    TooLarge,
    /// The file changed on disk since it was read.
    Conflict,
    Exists,
    /// Bad argument (bad regex, bad name, directory not empty…).
    Invalid,
    Io,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    /// Shown, never followed out of the folder.
    Symlink,
    /// FIFO, socket, device: listed, never opened.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    pub kind: EntryKind,
    pub size: u64,
    pub mtime_ns: u64,
    pub readonly: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    /// Matched by `.gitignore` (or always-skipped like `.git`).
    pub ignored: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatch {
    pub path: RelPath,
    pub score: i32,
    /// Byte offsets in `path` of the matched characters (for highlighting).
    pub positions: Vec<u32>,
}

/// A place in a file, with its line as context. Used for search hits, callers, definitions, sinks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Site {
    pub path: RelPath,
    pub line: u32,
    /// Byte column of the match in the file's line.
    pub col: u32,
    /// Byte length of the match.
    pub len: u32,
    /// The line (or a window of it around the match), at most `MAX_SNIPPET` bytes, no line break.
    pub text: String,
    /// Byte offset of the match inside `text`.
    pub text_col: u32,
    /// Enclosing function, or a call chain; may be empty.
    pub context: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchStats {
    pub files_scanned: u32,
    pub files_matched: u32,
    pub hits: u32,
    /// Stopped at `max_hits`.
    pub truncated: bool,
    pub elapsed_ms: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LensInfo {
    /// Call sites of this name in the folder.
    pub callers: u32,
    /// Definitions sharing this name (> 1: the numbers are ambiguous).
    pub defs: u32,
    /// How many of the given callees have a definition in the folder.
    pub callees: u32,
    /// Sink names reachable through calls (not counting direct calls in the function itself), a few.
    pub reach: Vec<String>,
    pub reach_more: bool,
}

/// One argument of a call on a path, with the inputs that flow into it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlowArg {
    /// Source text of the argument, at most `MAX_ARG_TEXT` bytes.
    pub text: String,
    /// Names of the tracked inputs of the enclosing function found in this argument, directly or through
    /// local assignments. First hop: every parameter is tracked. Later hops: only the parameters that
    /// received a tracked argument from the hop before. Empty: nothing tracked reaches it.
    pub from: Vec<String>,
}

/// One call on a path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hop {
    /// The call site. `context` = function containing the call.
    pub site: Site,
    /// Arguments of the call, in order (at most `MAX_ARGS`). Empty when they could not be read.
    pub args: Vec<FlowArg>,
}

/// A call chain from a function to a sink call, with heuristic argument flow.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallPath {
    /// Sink name (`memcpy`).
    pub sink: String,
    /// Sink class label (`memory`, `exec`, …).
    pub kind: String,
    /// Calls in order; the last one is the sink call. At most `MAX_HOPS`.
    pub hops: Vec<Hop>,
    /// At least one argument of the sink call is fed by a parameter of the first function, through every hop.
    pub flows: bool,
}

/// A file `Replace` left alone, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Skipped {
    pub path: RelPath,
    pub why: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Err { code: ErrCode, msg: String },
    Done,
    Dir { entries: Vec<DirEntry>, truncated: bool },
    /// `binary`: NUL bytes near the start; `data` is then empty.
    File { data: Vec<u8>, meta: Meta, binary: bool },
    Meta(Meta),
    FileMatches { items: Vec<FileMatch>, total_files: u32 },
    SearchBatch { hits: Vec<Site> },
    SearchDone(SearchStats),
    Lens { fns: Vec<LensInfo> },
    Sites { sites: Vec<Site>, truncated: bool },
    Paths { paths: Vec<CallPath>, truncated: bool },
    /// `changed`: files rewritten (at most `MAX_ITEMS`; `files` is the true count).
    Replaced { files: u32, replacements: u32, changed: Vec<RelPath>, skipped: Vec<Skipped>, cancelled: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexPhase {
    /// Walking the tree.
    Scanning,
    /// Reading changed files.
    Indexing,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexStatus {
    pub phase: IndexPhase,
    pub files_done: u32,
    pub files_total: u32,
    /// Bytes of indexed content.
    pub bytes: u64,
    /// Functions in the call index.
    pub symbols: u32,
    /// Files listed but not indexed (too large, binary, ignored).
    pub skipped: u32,
    /// Files whose index entry came from the on-disk cache.
    pub from_cache: u32,
    pub elapsed_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// First message of an engine.
    Hello { protocol: u32, root: String, name: String },
    Index(IndexStatus),
    /// The engine cannot go on (folder gone, worker died). Last message.
    Fatal { msg: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Reply(ReqId, Reply),
    Event(Event),
}

// ---- codec ----

pub trait Wire: Sized {
    fn enc(&self, e: &mut Enc);
    fn dec(d: &mut Dec<'_>) -> WireResult<Self>;
}

fn text(d: &mut Dec<'_>) -> WireResult<String> {
    d.string(MAX_TEXT)
}

fn names(d: &mut Dec<'_>) -> WireResult<Vec<String>> {
    d.seq(MAX_ITEMS, 4, text)
}

fn put_names(e: &mut Enc, v: &[String]) {
    e.seq(v, |e, s| e.str(s));
}

impl Wire for IndexOpts {
    fn enc(&self, e: &mut Enc) {
        e.bool(self.respect_gitignore);
        e.bool(self.index_hidden);
        e.u64(self.max_file_bytes);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self { respect_gitignore: d.bool()?, index_hidden: d.bool()?, max_file_bytes: d.u64()? })
    }
}

impl Wire for SearchQuery {
    fn enc(&self, e: &mut Enc) {
        e.str(&self.pattern);
        e.bool(self.regex);
        e.bool(self.case_sensitive);
        e.bool(self.whole_word);
        e.str(&self.include);
        e.str(&self.exclude);
        e.u32(self.max_hits);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self {
            pattern: text(d)?,
            regex: d.bool()?,
            case_sensitive: d.bool()?,
            whole_word: d.bool()?,
            include: text(d)?,
            exclude: text(d)?,
            max_hits: d.u32()?,
        })
    }
}

impl Wire for LensFn {
    fn enc(&self, e: &mut Enc) {
        e.str(&self.name);
        put_names(e, &self.callees);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self { name: text(d)?, callees: names(d)? })
    }
}

impl Wire for Request {
    fn enc(&self, e: &mut Enc) {
        match self {
            Request::ListDir { path } => {
                e.u8(1);
                e.path(path);
            }
            Request::ReadFile { path } => {
                e.u8(2);
                e.path(path);
            }
            Request::WriteFile { path, data, expect_mtime_ns, create_new } => {
                e.u8(3);
                e.path(path);
                e.bytes(data);
                e.opt(expect_mtime_ns.as_ref(), |e, v| e.u64(*v));
                e.bool(*create_new);
            }
            Request::CreateDir { path } => {
                e.u8(4);
                e.path(path);
            }
            Request::Rename { from, to } => {
                e.u8(5);
                e.path(from);
                e.path(to);
            }
            Request::Delete { path } => {
                e.u8(6);
                e.path(path);
            }
            Request::Stat { path } => {
                e.u8(7);
                e.path(path);
            }
            Request::FindFiles { query, include, exclude, limit } => {
                e.u8(8);
                e.str(query);
                e.str(include);
                e.str(exclude);
                e.u32(*limit);
            }
            Request::Search { query } => {
                e.u8(9);
                query.enc(e);
            }
            Request::Cancel { target } => {
                e.u8(10);
                e.u64(*target);
            }
            Request::Lens { fns } => {
                e.u8(11);
                e.seq(fns, |e, f| f.enc(e));
            }
            Request::Callers { name, limit } => {
                e.u8(12);
                e.str(name);
                e.u32(*limit);
            }
            Request::Defs { name, limit } => {
                e.u8(13);
                e.str(name);
                e.u32(*limit);
            }
            Request::Reach { name, limit } => {
                e.u8(14);
                e.str(name);
                e.u32(*limit);
            }
            Request::Reindex { opts } => {
                e.u8(15);
                opts.enc(e);
            }
            Request::Shutdown => e.u8(16),
            Request::Paths { name, sink, limit } => {
                e.u8(17);
                e.str(name);
                e.str(sink);
                e.u32(*limit);
            }
            Request::Replace { query, replacement, only, skip } => {
                e.u8(18);
                query.enc(e);
                e.str(replacement);
                e.seq(only, |e, p| e.path(p));
                e.seq(skip, |e, p| e.path(p));
            }
        }
    }

    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(match d.u8()? {
            1 => Request::ListDir { path: d.path()? },
            2 => Request::ReadFile { path: d.path()? },
            3 => Request::WriteFile {
                path: d.path()?,
                data: d.bytes(MAX_OPEN_FILE)?.to_vec(),
                expect_mtime_ns: d.opt(|d| d.u64())?,
                create_new: d.bool()?,
            },
            4 => Request::CreateDir { path: d.path()? },
            5 => Request::Rename { from: d.path()?, to: d.path()? },
            6 => Request::Delete { path: d.path()? },
            7 => Request::Stat { path: d.path()? },
            8 => Request::FindFiles { query: text(d)?, include: text(d)?, exclude: text(d)?, limit: d.u32()? },
            9 => Request::Search { query: SearchQuery::dec(d)? },
            10 => Request::Cancel { target: d.u64()? },
            11 => Request::Lens { fns: d.seq(MAX_ITEMS, 8, LensFn::dec)? },
            12 => Request::Callers { name: text(d)?, limit: d.u32()? },
            13 => Request::Defs { name: text(d)?, limit: d.u32()? },
            14 => Request::Reach { name: text(d)?, limit: d.u32()? },
            15 => Request::Reindex { opts: IndexOpts::dec(d)? },
            16 => Request::Shutdown,
            17 => Request::Paths { name: text(d)?, sink: text(d)?, limit: d.u32()? },
            18 => Request::Replace {
                query: SearchQuery::dec(d)?,
                replacement: text(d)?,
                only: d.seq(MAX_ITEMS, 4, |d| d.path())?,
                skip: d.seq(MAX_ITEMS, 4, |d| d.path())?,
            },
            t => return Err(WireError::BadTag(t)),
        })
    }
}

impl Wire for ErrCode {
    fn enc(&self, e: &mut Enc) {
        e.u8(match self {
            ErrCode::NotFound => 1,
            ErrCode::Denied => 2,
            ErrCode::TooLarge => 3,
            ErrCode::Conflict => 4,
            ErrCode::Exists => 5,
            ErrCode::Invalid => 6,
            ErrCode::Io => 7,
            ErrCode::Cancelled => 8,
        });
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(match d.u8()? {
            1 => ErrCode::NotFound,
            2 => ErrCode::Denied,
            3 => ErrCode::TooLarge,
            4 => ErrCode::Conflict,
            5 => ErrCode::Exists,
            6 => ErrCode::Invalid,
            7 => ErrCode::Io,
            8 => ErrCode::Cancelled,
            t => return Err(WireError::BadTag(t)),
        })
    }
}

impl Wire for EntryKind {
    fn enc(&self, e: &mut Enc) {
        e.u8(match self {
            EntryKind::File => 1,
            EntryKind::Dir => 2,
            EntryKind::Symlink => 3,
            EntryKind::Other => 4,
        });
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(match d.u8()? {
            1 => EntryKind::File,
            2 => EntryKind::Dir,
            3 => EntryKind::Symlink,
            4 => EntryKind::Other,
            t => return Err(WireError::BadTag(t)),
        })
    }
}

impl Wire for Meta {
    fn enc(&self, e: &mut Enc) {
        self.kind.enc(e);
        e.u64(self.size);
        e.u64(self.mtime_ns);
        e.bool(self.readonly);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self { kind: EntryKind::dec(d)?, size: d.u64()?, mtime_ns: d.u64()?, readonly: d.bool()? })
    }
}

impl Wire for DirEntry {
    fn enc(&self, e: &mut Enc) {
        e.str(&self.name);
        self.kind.enc(e);
        e.u64(self.size);
        e.bool(self.ignored);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        let name = d.string(MAX_NAME)?;
        if !crate::path::valid_name(&name) {
            return Err(WireError::BadPath);
        }
        Ok(Self { name, kind: EntryKind::dec(d)?, size: d.u64()?, ignored: d.bool()? })
    }
}

impl Wire for FileMatch {
    fn enc(&self, e: &mut Enc) {
        e.path(&self.path);
        e.i32(self.score);
        e.seq(&self.positions, |e, p| e.u32(*p));
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self { path: d.path()?, score: d.i32()?, positions: d.seq(crate::limits::MAX_PATH, 4, |d| d.u32())? })
    }
}

impl Wire for Site {
    fn enc(&self, e: &mut Enc) {
        e.path(&self.path);
        e.u32(self.line);
        e.u32(self.col);
        e.u32(self.len);
        e.str(&self.text);
        e.u32(self.text_col);
        e.str(&self.context);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self {
            path: d.path()?,
            line: d.u32()?,
            col: d.u32()?,
            len: d.u32()?,
            text: d.string(MAX_SNIPPET)?,
            text_col: d.u32()?,
            context: text(d)?,
        })
    }
}

impl Wire for FlowArg {
    fn enc(&self, e: &mut Enc) {
        e.str(&self.text);
        put_names(e, &self.from);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self { text: d.string(MAX_ARG_TEXT)?, from: d.seq(MAX_ARGS, 4, |d| d.string(MAX_NAME))? })
    }
}

impl Wire for Hop {
    fn enc(&self, e: &mut Enc) {
        self.site.enc(e);
        e.seq(&self.args, |e, a| a.enc(e));
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self { site: Site::dec(d)?, args: d.seq(MAX_ARGS, 8, FlowArg::dec)? })
    }
}

impl Wire for CallPath {
    fn enc(&self, e: &mut Enc) {
        e.str(&self.sink);
        e.str(&self.kind);
        e.seq(&self.hops, |e, h| h.enc(e));
        e.bool(self.flows);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self { sink: d.string(MAX_NAME)?, kind: d.string(MAX_NAME)?, hops: d.seq(MAX_HOPS, 32, Hop::dec)?, flows: d.bool()? })
    }
}

impl Wire for Skipped {
    fn enc(&self, e: &mut Enc) {
        e.path(&self.path);
        e.str(&self.why);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self { path: d.path()?, why: d.string(MAX_SNIPPET)? })
    }
}

impl Wire for SearchStats {
    fn enc(&self, e: &mut Enc) {
        e.u32(self.files_scanned);
        e.u32(self.files_matched);
        e.u32(self.hits);
        e.bool(self.truncated);
        e.u32(self.elapsed_ms);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self { files_scanned: d.u32()?, files_matched: d.u32()?, hits: d.u32()?, truncated: d.bool()?, elapsed_ms: d.u32()? })
    }
}

impl Wire for LensInfo {
    fn enc(&self, e: &mut Enc) {
        e.u32(self.callers);
        e.u32(self.defs);
        e.u32(self.callees);
        put_names(e, &self.reach);
        e.bool(self.reach_more);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(Self { callers: d.u32()?, defs: d.u32()?, callees: d.u32()?, reach: names(d)?, reach_more: d.bool()? })
    }
}

impl Wire for Reply {
    fn enc(&self, e: &mut Enc) {
        match self {
            Reply::Err { code, msg } => {
                e.u8(1);
                code.enc(e);
                e.str(msg);
            }
            Reply::Done => e.u8(2),
            Reply::Dir { entries, truncated } => {
                e.u8(3);
                e.seq(entries, |e, x| x.enc(e));
                e.bool(*truncated);
            }
            Reply::File { data, meta, binary } => {
                e.u8(4);
                e.bytes(data);
                meta.enc(e);
                e.bool(*binary);
            }
            Reply::Meta(m) => {
                e.u8(5);
                m.enc(e);
            }
            Reply::FileMatches { items, total_files } => {
                e.u8(6);
                e.seq(items, |e, x| x.enc(e));
                e.u32(*total_files);
            }
            Reply::SearchBatch { hits } => {
                e.u8(7);
                e.seq(hits, |e, x| x.enc(e));
            }
            Reply::SearchDone(s) => {
                e.u8(8);
                s.enc(e);
            }
            Reply::Lens { fns } => {
                e.u8(9);
                e.seq(fns, |e, x| x.enc(e));
            }
            Reply::Sites { sites, truncated } => {
                e.u8(10);
                e.seq(sites, |e, x| x.enc(e));
                e.bool(*truncated);
            }
            Reply::Paths { paths, truncated } => {
                e.u8(11);
                e.seq(paths, |e, x| x.enc(e));
                e.bool(*truncated);
            }
            Reply::Replaced { files, replacements, changed, skipped, cancelled } => {
                e.u8(12);
                e.u32(*files);
                e.u32(*replacements);
                e.seq(changed, |e, p| e.path(p));
                e.seq(skipped, |e, x| x.enc(e));
                e.bool(*cancelled);
            }
        }
    }

    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(match d.u8()? {
            1 => Reply::Err { code: ErrCode::dec(d)?, msg: text(d)? },
            2 => Reply::Done,
            3 => Reply::Dir { entries: d.seq(MAX_DIR_ENTRIES, 14, DirEntry::dec)?, truncated: d.bool()? },
            4 => Reply::File { data: d.bytes(MAX_OPEN_FILE)?.to_vec(), meta: Meta::dec(d)?, binary: d.bool()? },
            5 => Reply::Meta(Meta::dec(d)?),
            6 => Reply::FileMatches { items: d.seq(MAX_ITEMS, 12, FileMatch::dec)?, total_files: d.u32()? },
            7 => Reply::SearchBatch { hits: d.seq(MAX_ITEMS, 28, Site::dec)? },
            8 => Reply::SearchDone(SearchStats::dec(d)?),
            9 => Reply::Lens { fns: d.seq(MAX_ITEMS, 17, LensInfo::dec)? },
            10 => Reply::Sites { sites: d.seq(MAX_ITEMS, 28, Site::dec)?, truncated: d.bool()? },
            11 => Reply::Paths { paths: d.seq(MAX_ITEMS, 13, CallPath::dec)?, truncated: d.bool()? },
            12 => Reply::Replaced {
                files: d.u32()?,
                replacements: d.u32()?,
                changed: d.seq(MAX_ITEMS, 4, |d| d.path())?,
                skipped: d.seq(MAX_ITEMS, 8, Skipped::dec)?,
                cancelled: d.bool()?,
            },
            t => return Err(WireError::BadTag(t)),
        })
    }
}

impl Wire for IndexStatus {
    fn enc(&self, e: &mut Enc) {
        e.u8(match self.phase {
            IndexPhase::Scanning => 1,
            IndexPhase::Indexing => 2,
            IndexPhase::Ready => 3,
        });
        e.u32(self.files_done);
        e.u32(self.files_total);
        e.u64(self.bytes);
        e.u32(self.symbols);
        e.u32(self.skipped);
        e.u32(self.from_cache);
        e.u32(self.elapsed_ms);
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        let phase = match d.u8()? {
            1 => IndexPhase::Scanning,
            2 => IndexPhase::Indexing,
            3 => IndexPhase::Ready,
            t => return Err(WireError::BadTag(t)),
        };
        Ok(Self {
            phase,
            files_done: d.u32()?,
            files_total: d.u32()?,
            bytes: d.u64()?,
            symbols: d.u32()?,
            skipped: d.u32()?,
            from_cache: d.u32()?,
            elapsed_ms: d.u32()?,
        })
    }
}

impl Wire for Event {
    fn enc(&self, e: &mut Enc) {
        match self {
            Event::Hello { protocol, root, name } => {
                e.u8(1);
                e.u32(*protocol);
                e.str(root);
                e.str(name);
            }
            Event::Index(s) => {
                e.u8(2);
                s.enc(e);
            }
            Event::Fatal { msg } => {
                e.u8(3);
                e.str(msg);
            }
        }
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(match d.u8()? {
            1 => Event::Hello { protocol: d.u32()?, root: text(d)?, name: text(d)? },
            2 => Event::Index(IndexStatus::dec(d)?),
            3 => Event::Fatal { msg: text(d)? },
            t => return Err(WireError::BadTag(t)),
        })
    }
}

impl Wire for Out {
    fn enc(&self, e: &mut Enc) {
        match self {
            Out::Reply(id, r) => {
                e.u8(1);
                e.u64(*id);
                r.enc(e);
            }
            Out::Event(ev) => {
                e.u8(2);
                ev.enc(e);
            }
        }
    }
    fn dec(d: &mut Dec<'_>) -> WireResult<Self> {
        Ok(match d.u8()? {
            1 => Out::Reply(d.u64()?, Reply::dec(d)?),
            2 => Out::Event(Event::dec(d)?),
            t => return Err(WireError::BadTag(t)),
        })
    }
}

/// Frame payload of a request.
pub fn encode_request(id: ReqId, req: &Request) -> Vec<u8> {
    let mut e = Enc::new();
    e.u64(id);
    req.enc(&mut e);
    e.finish()
}

pub fn decode_request(buf: &[u8]) -> WireResult<(ReqId, Request)> {
    let mut d = Dec::new(buf);
    let id = d.u64()?;
    let req = Request::dec(&mut d)?;
    d.finish()?;
    Ok((id, req))
}

/// Frame payload of an engine message.
pub fn encode_out(out: &Out) -> Vec<u8> {
    let mut e = Enc::new();
    out.enc(&mut e);
    e.finish()
}

pub fn decode_out(buf: &[u8]) -> WireResult<Out> {
    let mut d = Dec::new(buf);
    let out = Out::dec(&mut d)?;
    d.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RelPath {
        RelPath::new(s).unwrap()
    }

    fn site() -> Site {
        Site { path: p("a/b.c"), line: 3, col: 4, len: 5, text: "x = memcpy(a)".into(), text_col: 4, context: "main → f".into() }
    }

    fn requests() -> Vec<Request> {
        vec![
            Request::ListDir { path: RelPath::root() },
            Request::ReadFile { path: p("a.md") },
            Request::WriteFile { path: p("d/a.md"), data: vec![0, 1, 2, 255], expect_mtime_ns: Some(42), create_new: false },
            Request::WriteFile { path: p("n"), data: vec![], expect_mtime_ns: None, create_new: true },
            Request::CreateDir { path: p("d") },
            Request::Rename { from: p("a"), to: p("b") },
            Request::Delete { path: p("a") },
            Request::Stat { path: p("a") },
            Request::FindFiles { query: "mn".into(), include: "*.rs".into(), exclude: "target/**".into(), limit: 50 },
            Request::Search {
                query: SearchQuery { pattern: "fo+".into(), regex: true, case_sensitive: true, whole_word: false, include: "*.rs".into(), exclude: String::new(), max_hits: 9 },
            },
            Request::Cancel { target: 77 },
            Request::Lens { fns: vec![LensFn { name: "f".into(), callees: vec!["g".into(), "h".into()] }, LensFn::default()] },
            Request::Callers { name: "f".into(), limit: 1 },
            Request::Defs { name: "f".into(), limit: 2 },
            Request::Reach { name: "f".into(), limit: 3 },
            Request::Paths { name: "f".into(), sink: "memcpy".into(), limit: 3 },
            Request::Replace {
                query: SearchQuery { pattern: "a(b)".into(), regex: true, ..Default::default() },
                replacement: "$1".into(),
                only: vec![p("a/b")],
                skip: vec![p("c"), p("d")],
            },
            Request::Reindex { opts: IndexOpts::default() },
            Request::Shutdown,
        ]
    }

    fn outs() -> Vec<Out> {
        let meta = Meta { kind: EntryKind::File, size: 9, mtime_ns: 1, readonly: true };
        vec![
            Out::Reply(1, Reply::Err { code: ErrCode::Denied, msg: "no".into() }),
            Out::Reply(2, Reply::Done),
            Out::Reply(3, Reply::Dir { entries: vec![DirEntry { name: "x".into(), kind: EntryKind::Symlink, size: 0, ignored: true }], truncated: false }),
            Out::Reply(4, Reply::File { data: b"hi".to_vec(), meta, binary: false }),
            Out::Reply(5, Reply::Meta(meta)),
            Out::Reply(6, Reply::FileMatches { items: vec![FileMatch { path: p("a/b"), score: -3, positions: vec![0, 2] }], total_files: 10 }),
            Out::Reply(7, Reply::SearchBatch { hits: vec![site(), site()] }),
            Out::Reply(7, Reply::SearchDone(SearchStats { files_scanned: 1, files_matched: 1, hits: 2, truncated: true, elapsed_ms: 3 })),
            Out::Reply(8, Reply::Lens { fns: vec![LensInfo { callers: 1, defs: 2, callees: 3, reach: vec!["system".into()], reach_more: true }] }),
            Out::Reply(9, Reply::Sites { sites: vec![site()], truncated: true }),
            Out::Reply(
                10,
                Reply::Paths {
                    paths: vec![CallPath {
                        sink: "memcpy".into(),
                        kind: "memory".into(),
                        hops: vec![Hop { site: site(), args: vec![FlowArg { text: "buf".into(), from: vec!["line".into()] }, FlowArg::default()] }, Hop::default()],
                        flows: true,
                    }],
                    truncated: false,
                },
            ),
            Out::Reply(11, Reply::Replaced { files: 2, replacements: 9, changed: vec![p("a"), p("b/c")], skipped: vec![Skipped { path: p("d"), why: "changed".into() }], cancelled: true }),
            Out::Event(Event::Hello { protocol: 1, root: "/r".into(), name: "r".into() }),
            Out::Event(Event::Index(IndexStatus { phase: IndexPhase::Ready, files_done: 1, files_total: 2, bytes: 3, symbols: 4, skipped: 5, from_cache: 6, elapsed_ms: 7 })),
            Out::Event(Event::Fatal { msg: "gone".into() }),
        ]
    }

    #[test]
    fn requests_round_trip() {
        for (i, r) in requests().into_iter().enumerate() {
            let b = encode_request(i as u64, &r);
            assert_eq!(decode_request(&b).unwrap(), (i as u64, r));
        }
    }

    #[test]
    fn outs_round_trip() {
        for o in outs() {
            assert_eq!(decode_out(&encode_out(&o)).unwrap(), o);
        }
    }

    /// Every prefix and every single-byte corruption decodes to a value or an error. Never a panic.
    #[test]
    fn hostile_bytes_never_panic() {
        let mut frames: Vec<Vec<u8>> = requests().iter().map(|r| encode_request(1, r)).collect();
        frames.extend(outs().iter().map(encode_out));
        for f in &frames {
            for n in 0..f.len() {
                assert!(decode_request(&f[..n]).is_err() || n >= 9);
                let _ = decode_out(&f[..n]);
            }
            let mut g = f.clone();
            for i in 0..g.len() {
                for v in [0u8, 1, 0x7f, 0xff] {
                    let old = g[i];
                    g[i] = v;
                    let _ = decode_request(&g);
                    let _ = decode_out(&g);
                    g[i] = old;
                }
            }
        }
    }

    #[test]
    fn hostile_paths_and_names_are_rejected() {
        let mut e = Enc::new();
        e.u64(1);
        e.u8(2);
        e.str("../../etc/passwd");
        assert_eq!(decode_request(&e.finish()).unwrap_err(), WireError::BadPath);

        let mut e = Enc::new();
        e.u8(1);
        e.u64(1);
        e.u8(3);
        e.u32(1);
        e.str("a/b");
        e.u8(1);
        e.u64(0);
        e.bool(false);
        e.bool(false);
        assert_eq!(decode_out(&e.finish()).unwrap_err(), WireError::BadPath);
    }
}
