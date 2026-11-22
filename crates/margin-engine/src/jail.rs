//! All path resolution and file I/O, confined to the canonical root.
//!
//! | Rule | How |
//! |---|---|
//! | existing path | lexical join, `canonicalize`, must stay under root (component-wise) |
//! | new path | canonical parent under root, final name passes `valid_name` |
//! | open | `O_NOFOLLOW \| O_NONBLOCK`, then `is_file()` on the open handle (FIFO/device → `Denied`) |
//! | dir symlink | `ListDir` on it → `Denied`; walker never enters one |
//! | write | temp file in the same dir, `sync_all`, keep mode, `rename` over the target |
//! | delete | file / link / empty dir only; never recursive |
//! | I/O gate | ≤ 4 concurrent open+read of folder files (`io_slot`); kernel contention above that |
//!
//! Residual TOCTOU: a component can be swapped for a symlink between check and open (no `openat` without unsafe).
//! The OS sandbox (folder + cache dir only) is the backstop.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::UNIX_EPOCH;

use margin_proto::limits::{MAX_DIR_ENTRIES, MAX_OPEN_FILE};
use margin_proto::path::valid_name;
use margin_proto::{DirEntry, EntryKind, ErrCode, Meta, RelPath};

#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x0004;
#[cfg(target_os = "macos")]
const O_NOFOLLOW: i32 = 0x0100;
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(target_os = "linux")]
const O_NOFOLLOW: i32 = 0o400000;

/// Bytes looked at for the binary check.
pub const BINARY_PROBE: usize = 8 << 10;

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Concurrent open+read of folder files. Many threads opening small files contend in the kernel
/// (measured on macOS: 16 threads ~3x slower than 4 on 100k 1 KB files); CPU work runs outside the gate.
static IO_SLOTS: AtomicUsize = AtomicUsize::new(DEFAULT_IO_SLOTS);
static IO_USED: Mutex<usize> = Mutex::new(0);
static IO_CV: Condvar = Condvar::new();
pub const DEFAULT_IO_SLOTS: usize = 4;

/// Change the gate size (bench / tuning). 0 = no gate.
pub fn set_io_slots(n: usize) {
    IO_SLOTS.store(n, Ordering::Relaxed);
    IO_CV.notify_all();
}

/// Held while a folder file is opened and read.
pub struct IoSlot(bool);

pub fn io_slot() -> IoSlot {
    let max = IO_SLOTS.load(Ordering::Relaxed);
    if max == 0 {
        return IoSlot(false);
    }
    let Ok(mut used) = IO_USED.lock() else { return IoSlot(false) };
    while *used >= IO_SLOTS.load(Ordering::Relaxed).max(1) {
        used = match IO_CV.wait(used) {
            Ok(g) => g,
            Err(_) => return IoSlot(false),
        };
    }
    *used += 1;
    IoSlot(true)
}

impl Drop for IoSlot {
    fn drop(&mut self) {
        if self.0 {
            if let Ok(mut used) = IO_USED.lock() {
                *used = used.saturating_sub(1);
            }
            IO_CV.notify_one();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JailErr {
    pub code: ErrCode,
    pub msg: String,
}

impl JailErr {
    pub fn new(code: ErrCode, msg: impl Into<String>) -> Self {
        Self { code, msg: msg.into() }
    }
}

pub type JResult<T> = Result<T, JailErr>;

fn io_err(what: &str, e: &io::Error) -> JailErr {
    let code = match e.kind() {
        io::ErrorKind::NotFound => ErrCode::NotFound,
        io::ErrorKind::PermissionDenied => ErrCode::Denied,
        io::ErrorKind::AlreadyExists => ErrCode::Exists,
        io::ErrorKind::DirectoryNotEmpty => ErrCode::Invalid,
        _ => ErrCode::Io,
    };
    JailErr::new(code, format!("{what}: {e}"))
}

fn denied(what: &str) -> JailErr {
    JailErr::new(ErrCode::Denied, what.to_owned())
}

/// Nanoseconds since the epoch (0 if unknown or before 1970).
pub fn mtime_ns(md: &fs::Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

pub fn kind_of(ft: fs::FileType) -> EntryKind {
    if ft.is_symlink() {
        EntryKind::Symlink
    } else if ft.is_dir() {
        EntryKind::Dir
    } else if ft.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

pub fn meta_of(md: &fs::Metadata) -> Meta {
    Meta { kind: kind_of(md.file_type()), size: md.len(), mtime_ns: mtime_ns(md), readonly: md.permissions().readonly() }
}

/// Open read-only without following a final symlink and without blocking on a FIFO; regular files only.
pub fn open_regular(path: &Path) -> JResult<(File, fs::Metadata)> {
    let mut o = OpenOptions::new();
    o.read(true);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(O_NOFOLLOW | O_NONBLOCK);
    }
    let f = o.open(path).map_err(|e| {
        // ELOOP from O_NOFOLLOW on a symlink.
        if e.raw_os_error() == Some(if cfg!(target_os = "linux") { 40 } else { 62 }) {
            denied("symbolic link")
        } else {
            io_err("open", &e)
        }
    })?;
    let md = f.metadata().map_err(|e| io_err("stat", &e))?;
    if !md.is_file() {
        return Err(denied("not a regular file"));
    }
    Ok((f, md))
}

/// Read at most `cap` bytes; more → `TooLarge`. `take(cap + 1)` so a growing file cannot exceed the cap.
pub fn read_capped(f: &mut File, size_hint: u64, cap: usize, buf: &mut Vec<u8>) -> JResult<()> {
    buf.clear();
    let hint = usize::try_from(size_hint).unwrap_or(usize::MAX).min(cap);
    buf.reserve(hint.saturating_add(1));
    let lim = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    f.take(lim).read_to_end(buf).map_err(|e| io_err("read", &e))?;
    if buf.len() > cap {
        buf.clear();
        return Err(JailErr::new(ErrCode::TooLarge, format!("file over {cap} bytes")));
    }
    Ok(())
}

pub fn is_binary(data: &[u8]) -> bool {
    memchr::memchr(0, data.get(..BINARY_PROBE).unwrap_or(data)).is_some()
}

#[derive(Clone, Debug)]
pub struct Jail {
    root: PathBuf,
    /// `ReadFile` cap (`MAX_OPEN_FILE`; tests lower it).
    pub max_open: usize,
}

impl Jail {
    /// `root` is canonicalised here and must be a directory.
    pub fn new(root: &Path) -> io::Result<Self> {
        let root = fs::canonicalize(root)?;
        if !fs::metadata(&root)?.is_dir() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a directory"));
        }
        Ok(Self { root, max_open: MAX_OPEN_FILE })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Lexical join only (no disk access). For paths produced by the walker.
    pub fn lexical(&self, rel: &RelPath) -> PathBuf {
        rel.under(&self.root)
    }

    pub fn inside(&self, p: &Path) -> bool {
        p.starts_with(&self.root)
    }

    /// Existing path → canonical path under root.
    pub fn resolve_existing(&self, rel: &RelPath) -> JResult<PathBuf> {
        let lex = self.lexical(rel);
        let canon = fs::canonicalize(&lex).map_err(|e| io_err("resolve", &e))?;
        if !self.inside(&canon) {
            return Err(denied("path leaves the folder"));
        }
        Ok(canon)
    }

    /// Canonical parent directory (under root) + validated final name. The name itself is not resolved.
    pub fn resolve_parent(&self, rel: &RelPath) -> JResult<(PathBuf, String)> {
        let name = rel.name();
        if rel.is_root() || !valid_name(name) {
            return Err(JailErr::new(ErrCode::Invalid, "bad name"));
        }
        let parent = rel.parent().unwrap_or_default();
        let pdir = self.resolve_existing(&parent)?;
        let md = fs::metadata(&pdir).map_err(|e| io_err("stat", &e))?;
        if !md.is_dir() {
            return Err(JailErr::new(ErrCode::NotFound, "parent is not a directory"));
        }
        Ok((pdir, name.to_owned()))
    }

    /// The entry itself (a link is not followed). A link is accepted only if it resolves inside root.
    fn resolve_entry(&self, rel: &RelPath) -> JResult<(PathBuf, fs::Metadata)> {
        let (pdir, name) = self.resolve_parent(rel)?;
        let p = pdir.join(&name);
        let md = fs::symlink_metadata(&p).map_err(|e| io_err("stat", &e))?;
        if md.file_type().is_symlink() {
            let target = fs::canonicalize(&p).map_err(|_| denied("dangling or looping symbolic link"))?;
            if !self.inside(&target) {
                return Err(denied("symbolic link leaves the folder"));
            }
        }
        Ok((p, md))
    }

    pub fn stat(&self, rel: &RelPath) -> JResult<Meta> {
        let p = self.resolve_existing(rel)?;
        let md = fs::symlink_metadata(&p).map_err(|e| io_err("stat", &e))?;
        Ok(meta_of(&md))
    }

    /// Entries of a directory (not sorted, not flagged). Dir symlinks → `Denied`.
    /// Returns (entries, truncated, skipped names).
    pub fn list_dir(&self, rel: &RelPath) -> JResult<(Vec<DirEntry>, bool, u32)> {
        if !rel.is_root() {
            let lex = self.lexical(rel);
            let md = fs::symlink_metadata(&lex).map_err(|e| io_err("stat", &e))?;
            if md.file_type().is_symlink() {
                return Err(denied("directory symbolic links are not followed"));
            }
        }
        let p = self.resolve_existing(rel)?;
        let md = fs::symlink_metadata(&p).map_err(|e| io_err("stat", &e))?;
        if !md.is_dir() {
            return Err(JailErr::new(ErrCode::Invalid, "not a directory"));
        }
        let rd = fs::read_dir(&p).map_err(|e| io_err("list", &e))?;
        let mut out = Vec::new();
        let mut truncated = false;
        let mut skipped = 0u32;
        for ent in rd {
            let Ok(ent) = ent else {
                skipped = skipped.saturating_add(1);
                continue;
            };
            let name = match ent.file_name().into_string() {
                Ok(n) if valid_name(&n) => n,
                _ => {
                    skipped = skipped.saturating_add(1);
                    continue;
                }
            };
            if out.len() >= MAX_DIR_ENTRIES {
                truncated = true;
                break;
            }
            // `DirEntry::metadata` does not follow symlinks.
            let (kind, size) = match ent.metadata() {
                Ok(m) => (kind_of(m.file_type()), if m.is_file() { m.len() } else { 0 }),
                Err(_) => (EntryKind::Other, 0),
            };
            out.push(DirEntry { name, kind, size, ignored: false });
        }
        Ok((out, truncated, skipped))
    }

    /// → (data, meta, binary). Binary: data empty.
    pub fn read_file(&self, rel: &RelPath) -> JResult<(Vec<u8>, Meta, bool)> {
        let p = self.resolve_existing(rel)?;
        let (mut f, md) = open_regular(&p)?;
        if md.len() > self.max_open as u64 {
            return Err(JailErr::new(ErrCode::TooLarge, format!("file over {} bytes", self.max_open)));
        }
        let mut buf = Vec::new();
        read_capped(&mut f, md.len(), self.max_open, &mut buf)?;
        let meta = meta_of(&md);
        if is_binary(&buf) {
            return Ok((Vec::new(), meta, true));
        }
        Ok((buf, meta, false))
    }

    /// Read the content of a file through full resolution, capped (`TooLarge` beyond).
    pub fn read_resolved(&self, rel: &RelPath, cap: usize, buf: &mut Vec<u8>) -> JResult<Meta> {
        let p = self.resolve_existing(rel)?;
        let (mut f, md) = open_regular(&p)?;
        read_capped(&mut f, md.len(), cap, buf)?;
        Ok(meta_of(&md))
    }

    /// Atomic write → new metadata. Returns also the canonical target (for index updates).
    pub fn write_file(&self, rel: &RelPath, data: &[u8], expect_mtime_ns: Option<u64>, create_new: bool) -> JResult<Meta> {
        let (pdir, name) = self.resolve_parent(rel)?;
        let lex = pdir.join(&name);
        let (target, old) = match fs::symlink_metadata(&lex) {
            Ok(lmd) => {
                if create_new {
                    return Err(JailErr::new(ErrCode::Exists, "already exists"));
                }
                let target = if lmd.file_type().is_symlink() {
                    let t = fs::canonicalize(&lex).map_err(|_| denied("dangling or looping symbolic link"))?;
                    if !self.inside(&t) {
                        return Err(denied("symbolic link leaves the folder"));
                    }
                    t
                } else {
                    lex
                };
                let md = fs::symlink_metadata(&target).map_err(|e| io_err("stat", &e))?;
                if !md.is_file() {
                    return Err(denied("not a regular file"));
                }
                if let Some(want) = expect_mtime_ns {
                    if mtime_ns(&md) != want {
                        return Err(JailErr::new(ErrCode::Conflict, "the file changed on disk"));
                    }
                }
                (target, Some(md))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if expect_mtime_ns.is_some() {
                    return Err(JailErr::new(ErrCode::Conflict, "the file was removed on disk"));
                }
                (lex, None)
            }
            Err(e) => return Err(io_err("stat", &e)),
        };
        let dir = target.parent().ok_or_else(|| denied("no parent"))?.to_path_buf();
        if !self.inside(&dir) {
            return Err(denied("path leaves the folder"));
        }
        let tname = target.file_name().and_then(|n| n.to_str()).unwrap_or("f");
        let tmp = dir.join(tmp_name(tname));
        let f = create_tmp(&tmp).map_err(|e| io_err("write", &e))?;
        let res = fill_tmp(f, data, old.as_ref()).and_then(|()| fs::rename(&tmp, &target));
        if let Err(e) = res {
            // Only the exact temp path we created above.
            let _ = fs::remove_file(&tmp);
            return Err(io_err("write", &e));
        }
        let md = fs::symlink_metadata(&target).map_err(|e| io_err("stat", &e))?;
        Ok(meta_of(&md))
    }

    /// One level; the parent must exist.
    pub fn create_dir(&self, rel: &RelPath) -> JResult<()> {
        let (pdir, name) = self.resolve_parent(rel)?;
        fs::create_dir(pdir.join(name)).map_err(|e| io_err("create directory", &e))
    }

    /// Renames the entry itself (a link stays a link). `Exists` if `to` exists.
    /// Residual race: `to` may appear between the check and `rename` (no `RENAME_EXCL` without libc).
    pub fn rename(&self, from: &RelPath, to: &RelPath) -> JResult<()> {
        let (src, _) = self.resolve_entry(from)?;
        let (pdir, name) = self.resolve_parent(to)?;
        let dst = pdir.join(name);
        if fs::symlink_metadata(&dst).is_ok() {
            return Err(JailErr::new(ErrCode::Exists, "target exists"));
        }
        if !self.inside(&src) || !self.inside(&dst) || src == self.root {
            return Err(denied("path leaves the folder"));
        }
        fs::rename(&src, &dst).map_err(|e| io_err("rename", &e))
    }

    /// File or link: unlink. Directory: `remove_dir` (empty only). Never recursive.
    pub fn delete(&self, rel: &RelPath) -> JResult<EntryKind> {
        let (p, md) = self.resolve_entry(rel)?;
        if p == self.root || !self.inside(&p) {
            return Err(denied("cannot delete the folder"));
        }
        let kind = kind_of(md.file_type());
        if md.is_dir() {
            fs::remove_dir(&p).map_err(|e| {
                let nonempty = e.kind() == io::ErrorKind::DirectoryNotEmpty || e.raw_os_error() == Some(if cfg!(target_os = "linux") { 39 } else { 66 });
                if nonempty {
                    JailErr::new(ErrCode::Invalid, "directory not empty (delete is never recursive)")
                } else {
                    io_err("delete", &e)
                }
            })?;
        } else {
            fs::remove_file(&p).map_err(|e| io_err("delete", &e))?;
        }
        Ok(kind)
    }
}

/// `.{name}.margin-tmp-{pid}-{n}`; name cut to stay under NAME_MAX.
fn tmp_name(name: &str) -> String {
    let mut cut = name.len().min(160);
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    let n = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    format!(".{}.margin-tmp-{}-{n}", &name[..cut], std::process::id())
}

fn create_tmp(tmp: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(tmp)
}

fn fill_tmp(mut f: File, data: &[u8], old: Option<&fs::Metadata>) -> io::Result<()> {
    f.write_all(data)?;
    if let Some(md) = old {
        f.set_permissions(md.permissions())?;
    }
    f.sync_all()
}

/// Atomic write of a file this process owns (the cache). `dir` must be absolute.
pub fn write_atomic_owned(dir: &Path, name: &str, data: &[u8]) -> io::Result<()> {
    if !dir.is_absolute() || dir.as_os_str().is_empty() || !valid_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad cache path"));
    }
    let tmp = dir.join(tmp_name(name));
    let f = create_tmp(&tmp)?;
    let res = fill_tmp(f, data, None).and_then(|()| fs::rename(&tmp, dir.join(name)));
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}
