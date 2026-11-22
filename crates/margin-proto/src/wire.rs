//! Binary codec. Little-endian, length-prefixed. The decoder never trusts a length: every count is checked
//! against the bytes left before anything is allocated.

use std::fmt;

use crate::path::RelPath;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    Truncated,
    TooLong,
    BadTag(u8),
    BadUtf8,
    BadPath,
    Trailing,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::Truncated => f.write_str("truncated message"),
            WireError::TooLong => f.write_str("field over its size cap"),
            WireError::BadTag(t) => write!(f, "unknown tag {t}"),
            WireError::BadUtf8 => f.write_str("invalid UTF-8"),
            WireError::BadPath => f.write_str("invalid path"),
            WireError::Trailing => f.write_str("trailing bytes"),
        }
    }
}

impl std::error::Error for WireError {}

pub type WireResult<T> = Result<T, WireError>;

#[derive(Default)]
pub struct Enc {
    buf: Vec<u8>,
}

impl Enc {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(n: usize) -> Self {
        Self { buf: Vec::with_capacity(n) }
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn bool(&mut self, v: bool) {
        self.buf.push(u8::from(v));
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    /// Lengths saturate at `u32::MAX`; the decoder then rejects the frame (nothing that long is legal).
    fn len(&mut self, n: usize) {
        self.u32(u32::try_from(n).unwrap_or(u32::MAX));
    }

    pub fn bytes(&mut self, v: &[u8]) {
        self.len(v.len());
        self.buf.extend_from_slice(v);
    }

    pub fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }

    pub fn path(&mut self, v: &RelPath) {
        self.str(v.as_str());
    }

    pub fn seq<T>(&mut self, items: &[T], mut f: impl FnMut(&mut Enc, &T)) {
        self.len(items.len());
        for it in items {
            f(self, it);
        }
    }

    pub fn opt<T>(&mut self, v: Option<&T>, f: impl FnOnce(&mut Enc, &T)) {
        match v {
            Some(x) => {
                self.u8(1);
                f(self, x);
            }
            None => self.u8(0),
        }
    }
}

pub struct Dec<'a> {
    buf: &'a [u8],
}

impl<'a> Dec<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len()
    }

    /// Error unless everything was consumed.
    pub fn finish(self) -> WireResult<()> {
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(WireError::Trailing)
        }
    }

    fn take(&mut self, n: usize) -> WireResult<&'a [u8]> {
        if n > self.buf.len() {
            return Err(WireError::Truncated);
        }
        let (a, b) = self.buf.split_at(n);
        self.buf = b;
        Ok(a)
    }

    pub fn u8(&mut self) -> WireResult<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> WireResult<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            t => Err(WireError::BadTag(t)),
        }
    }

    pub fn u32(&mut self) -> WireResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> WireResult<u64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }

    pub fn i32(&mut self) -> WireResult<i32> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Byte string of at most `max` bytes.
    pub fn bytes(&mut self, max: usize) -> WireResult<&'a [u8]> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(WireError::TooLong);
        }
        self.take(n)
    }

    pub fn string(&mut self, max: usize) -> WireResult<String> {
        let b = self.bytes(max)?;
        std::str::from_utf8(b).map(str::to_owned).map_err(|_| WireError::BadUtf8)
    }

    pub fn path(&mut self) -> WireResult<RelPath> {
        let b = self.bytes(crate::limits::MAX_PATH)?;
        let s = std::str::from_utf8(b).map_err(|_| WireError::BadUtf8)?;
        RelPath::new(s).map_err(|_| WireError::BadPath)
    }

    /// List of at most `max` items, each at least `min_size` encoded bytes (so a lying count cannot allocate).
    pub fn seq<T>(&mut self, max: usize, min_size: usize, mut f: impl FnMut(&mut Dec<'a>) -> WireResult<T>) -> WireResult<Vec<T>> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(WireError::TooLong);
        }
        if n.saturating_mul(min_size.max(1)) > self.buf.len() {
            return Err(WireError::Truncated);
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(f(self)?);
        }
        Ok(v)
    }

    pub fn opt<T>(&mut self, f: impl FnOnce(&mut Dec<'a>) -> WireResult<T>) -> WireResult<Option<T>> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(f(self)?)),
            t => Err(WireError::BadTag(t)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_scalars() {
        let mut e = Enc::new();
        e.u8(7);
        e.bool(true);
        e.u32(0xdead_beef);
        e.u64(u64::MAX - 1);
        e.i32(-5);
        e.str("héllo");
        e.seq(&[1u32, 2, 3], |e, v| e.u32(*v));
        e.opt(Some(&9u32), |e, v| e.u32(*v));
        e.opt::<u32>(None, |e, v| e.u32(*v));
        let b = e.finish();
        let mut d = Dec::new(&b);
        assert_eq!(d.u8().unwrap(), 7);
        assert!(d.bool().unwrap());
        assert_eq!(d.u32().unwrap(), 0xdead_beef);
        assert_eq!(d.u64().unwrap(), u64::MAX - 1);
        assert_eq!(d.i32().unwrap(), -5);
        assert_eq!(d.string(64).unwrap(), "héllo");
        assert_eq!(d.seq(10, 4, |d| d.u32()).unwrap(), vec![1, 2, 3]);
        assert_eq!(d.opt(|d| d.u32()).unwrap(), Some(9));
        assert_eq!(d.opt(|d| d.u32()).unwrap(), None);
        d.finish().unwrap();
    }

    #[test]
    fn lying_lengths_do_not_allocate() {
        let mut e = Enc::new();
        e.u32(u32::MAX);
        let b = e.finish();
        assert_eq!(Dec::new(&b).bytes(usize::MAX).unwrap_err(), WireError::Truncated);
        assert_eq!(Dec::new(&b).bytes(10).unwrap_err(), WireError::TooLong);
        assert_eq!(Dec::new(&b).seq(usize::MAX, 1, |d| d.u8()).unwrap_err(), WireError::Truncated);
        assert_eq!(Dec::new(&b).seq(5, 1, |d| d.u8()).unwrap_err(), WireError::TooLong);
    }

    #[test]
    fn bad_values() {
        assert_eq!(Dec::new(&[2]).bool().unwrap_err(), WireError::BadTag(2));
        let mut e = Enc::new();
        e.bytes(&[0xff, 0xfe]);
        assert_eq!(Dec::new(&e.finish()).string(10).unwrap_err(), WireError::BadUtf8);
        let mut e = Enc::new();
        e.str("../etc/passwd");
        assert_eq!(Dec::new(&e.finish()).path().unwrap_err(), WireError::BadPath);
        assert_eq!(Dec::new(&[1, 2]).finish().unwrap_err(), WireError::Trailing);
    }
}
