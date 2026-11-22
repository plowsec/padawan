//! PNG encoder: RGBA8, filter `Sub` per row, zlib via flate2. No image crate.

use std::io::Write as _;

use flate2::{write::ZlibEncoder, Compression, Crc};

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}

/// Encode `w × h` RGBA8 (`rgba.len() == w*h*4`).
pub fn encode(w: u32, h: u32, rgba: &[u8]) -> std::io::Result<Vec<u8>> {
    let stride = w as usize * 4;
    if rgba.len() != stride * h as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "rgba length != w*h*4",
        ));
    }
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for row in rgba.chunks_exact(stride.max(1)) {
        raw.push(1); // Sub
        for (i, &b) in row.iter().enumerate() {
            raw.push(if i >= 4 {
                b.wrapping_sub(row[i - 4])
            } else {
                b
            });
        }
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::new(6));
    z.write_all(&raw)?;
    let idat = z.finish()?;

    let mut out = Vec::with_capacity(idat.len() + 64);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, adaptive, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;
    use std::io::Read as _;

    #[test]
    fn roundtrip_2x2() {
        let px = [
            255u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 10, 20, 30, 128,
        ];
        let png = encode(2, 2, &px).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        // CRC of IEND is fixed.
        assert_eq!(&png[png.len() - 4..], &[0xae, 0x42, 0x60, 0x82]);
        let idat_len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let mut raw = Vec::new();
        ZlibDecoder::new(&png[41..41 + idat_len])
            .read_to_end(&mut raw)
            .unwrap();
        // Undo Sub filter.
        let mut back = Vec::new();
        for row in raw.chunks(9) {
            assert_eq!(row[0], 1);
            let mut prev = [0u8; 4];
            for (i, &b) in row[1..].iter().enumerate() {
                let v = b.wrapping_add(if i >= 4 { prev[i % 4] } else { 0 });
                prev[i % 4] = v;
                back.push(v);
            }
        }
        assert_eq!(back, px);
    }

    #[test]
    fn rejects_bad_length() {
        assert!(encode(2, 2, &[0; 3]).is_err());
    }
}
