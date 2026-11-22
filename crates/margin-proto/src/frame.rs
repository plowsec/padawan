//! Frames on a byte stream: `u32 LE length` + payload.

use std::io::{self, Read, Write};

/// One frame. `Ok(None)`: clean end of stream before a header. A length over `max` is an error.
pub fn read_frame(r: &mut impl Read, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_le_bytes(head) as usize;
    if len > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame over the size cap"));
    }
    // Grow as bytes arrive: a lying header cannot make us allocate `len` up front.
    let mut buf = Vec::with_capacity(len.min(64 << 10));
    r.take(len as u64).read_to_end(&mut buf)?;
    if buf.len() != len {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(Some(buf))
}

pub fn write_frame(w: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_eof() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"abc").unwrap();
        write_frame(&mut buf, b"").unwrap();
        let mut r = &buf[..];
        assert_eq!(read_frame(&mut r, 10).unwrap().unwrap(), b"abc");
        assert_eq!(read_frame(&mut r, 10).unwrap().unwrap(), b"");
        assert!(read_frame(&mut r, 10).unwrap().is_none());
    }

    #[test]
    fn rejects_oversize_and_truncation() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &[0u8; 100]).unwrap();
        assert!(read_frame(&mut &buf[..], 99).is_err());
        assert!(read_frame(&mut &buf[..50], 1000).is_err());
        assert!(read_frame(&mut &buf[..2], 1000).is_err());
        // Header claims 4 GiB, nothing follows: error, no allocation of that size.
        assert!(read_frame(&mut &[0xff, 0xff, 0xff, 0x7f][..], usize::MAX).is_err());
    }
}
