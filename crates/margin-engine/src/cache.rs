//! On-disk index cache: `<cache_dir>/index.bin`. Untrusted on load (a compromised worker may have written it).
//!
//! | Part | Encoding (`wire::Enc`) |
//! |---|---|
//! | header | magic `MRGNIDX1`, u32 version, canonical root (str) |
//! | files | seq of: path, u64 size, u64 mtime_ns, u8 flags, bloom bytes, seq of fn (name, line, col, seq of call (name, line, col, u8 sink)) |
//!
//! Load: file ≤ 1 GiB, every length bounded, paths re-validated, any error → whole cache ignored.
//! Save: temp file + rename inside `cache_dir` (created with `create_dir_all` on that one path).

use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use margin_proto::limits::MAX_PATH;
use margin_proto::wire::{Dec, Enc, WireError, WireResult};
use margin_proto::RelPath;

use crate::bloom::MAX_BLOOM;
use crate::index::{CallSum, FileRec, FnSum, F_ALL, F_PENDING, MAX_CALLS, MAX_FILE_CALLS, MAX_FNS, MAX_SYMBOL};
use crate::jail::write_atomic_owned;
use crate::walk::MAX_FILES;

const MAGIC: &[u8; 8] = b"MRGNIDX1";
pub const VERSION: u32 = 1;
pub const FILE_NAME: &str = "index.bin";
pub const MAX_CACHE_BYTES: u64 = 1 << 30;

pub fn encode(root: &str, files: &[Arc<FileRec>]) -> Vec<u8> {
    let mut e = Enc::with_capacity(files.len().saturating_mul(256));
    for &b in MAGIC {
        e.u8(b);
    }
    e.u32(VERSION);
    e.str(root);
    let keep: Vec<&Arc<FileRec>> = files.iter().filter(|f| f.flags & F_PENDING == 0).collect();
    e.seq(&keep, |e, f| {
        e.str(&f.path);
        e.u64(f.size);
        e.u64(f.mtime_ns);
        e.u8(f.flags);
        e.bytes(&f.bloom);
        e.seq(&f.fns, |e, d| {
            e.str(&d.name);
            e.u32(d.line);
            e.u32(d.col);
            e.seq(&d.calls, |e, c| {
                e.str(&c.name);
                e.u32(c.line);
                e.u32(c.col);
                e.u8(c.sink);
            });
        });
    });
    e.finish()
}

fn sym(d: &mut Dec<'_>) -> WireResult<Box<str>> {
    let s = d.string(MAX_SYMBOL)?;
    if s.is_empty() {
        return Err(WireError::BadPath);
    }
    Ok(s.into())
}

pub fn decode(buf: &[u8], root: &str) -> WireResult<Vec<Arc<FileRec>>> {
    let mut d = Dec::new(buf);
    for &b in MAGIC {
        if d.u8()? != b {
            return Err(WireError::BadTag(b));
        }
    }
    if d.u32()? != VERSION {
        return Err(WireError::BadTag(0));
    }
    if d.string(MAX_PATH * 4)? != root {
        return Err(WireError::BadPath);
    }
    // Min encoded file: 4 + 8 + 8 + 1 + 4 + 4.
    let files = d.seq(MAX_FILES, 29, |d| {
        let path = d.string(MAX_PATH)?;
        if path.is_empty() || RelPath::new(&path).is_err() {
            return Err(WireError::BadPath);
        }
        let size = d.u64()?;
        let mtime_ns = d.u64()?;
        let flags = d.u8()?;
        if flags & !F_ALL != 0 || flags & F_PENDING != 0 {
            return Err(WireError::BadTag(flags));
        }
        let bloom = d.bytes(MAX_BLOOM)?;
        if !bloom.is_empty() && bloom.len() % 8 != 0 {
            return Err(WireError::TooLong);
        }
        let mut budget = MAX_FILE_CALLS;
        let fns = d.seq(MAX_FNS, 16, |d| {
            let name = sym(d)?;
            let line = d.u32()?;
            let col = d.u32()?;
            let calls = d.seq(MAX_CALLS.min(budget), 13, |d| Ok(CallSum { name: sym(d)?, line: d.u32()?, col: d.u32()?, sink: d.u8()? }))?;
            budget -= calls.len();
            Ok(FnSum { name, line, col, calls: calls.into() })
        })?;
        Ok(Arc::new(FileRec { path: path.into(), size, mtime_ns, flags, bloom: bloom.into(), fns: fns.into() }))
    })?;
    d.finish()?;
    // Sorted and unique, as the walk produces them.
    if files.windows(2).any(|w| w[0].path >= w[1].path) {
        return Err(WireError::BadPath);
    }
    Ok(files)
}

/// `None` on any problem (missing, too large, corrupt, other root, other version).
pub fn load(dir: &Path, root: &str) -> Option<Vec<Arc<FileRec>>> {
    let p = dir.join(FILE_NAME);
    let md = fs::symlink_metadata(&p).ok()?;
    if !md.is_file() || md.len() > MAX_CACHE_BYTES {
        return None;
    }
    let f = fs::File::open(&p).ok()?;
    let mut buf = Vec::with_capacity(md.len() as usize);
    f.take(MAX_CACHE_BYTES + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > MAX_CACHE_BYTES {
        return None;
    }
    decode(&buf, root).ok()
}

pub fn save(dir: &Path, root: &str, files: &[Arc<FileRec>]) -> std::io::Result<()> {
    if !dir.is_absolute() || dir.as_os_str().is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "cache dir must be absolute"));
    }
    fs::create_dir_all(dir)?;
    let data = encode(root, files);
    write_atomic_owned(dir, FILE_NAME, &data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Arc<FileRec>> {
        vec![
            Arc::new(FileRec {
                path: "a/b.rs".into(),
                size: 10,
                mtime_ns: 99,
                flags: 0,
                bloom: vec![1u8; 16].into(),
                fns: vec![FnSum { name: "f".into(), line: 1, col: 2, calls: vec![CallSum { name: "system".into(), line: 3, col: 4, sink: 1 }].into() }].into(),
            }),
            Arc::new(FileRec { path: "z.bin".into(), size: 5, mtime_ns: 7, flags: crate::index::F_BINARY, bloom: Box::default(), fns: Box::default() }),
        ]
    }

    #[test]
    fn round_trip() {
        let f = sample();
        let b = encode("/r", &f);
        assert_eq!(decode(&b, "/r").unwrap(), f);
        assert!(decode(&b, "/other").is_err());
    }

    #[test]
    fn corrupt_is_rejected_without_panic() {
        let b = encode("/r", &sample());
        for n in 0..b.len() {
            assert!(decode(&b[..n], "/r").is_err());
        }
        for i in 0..b.len() {
            for v in [0u8, 1, 0x7f, 0x80, 0xff] {
                let mut c = b.clone();
                c[i] = v;
                let _ = decode(&c, "/r");
            }
        }
        let mut c = b.clone();
        c[8] = 2;
        assert!(decode(&c, "/r").is_err(), "wrong version");
    }

    #[test]
    fn file_round_trip() {
        let t = margin_testutil::TempDir::new("cache");
        let dir = t.path().join("c");
        save(&dir, "/r", &sample()).unwrap();
        assert_eq!(load(&dir, "/r").unwrap(), sample());
        assert!(load(&dir, "/x").is_none());
        let names: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from(FILE_NAME)]);
        fs::write(dir.join(FILE_NAME), b"garbage").unwrap();
        assert!(load(&dir, "/r").is_none());
    }
}
