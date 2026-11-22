//! `Site` construction: line text window around a match.
//!
//! | Field | Rule |
//! |---|---|
//! | `text` | the line (trailing `\r` cut), or a window ≤ `MAX_SNIPPET` bytes cut on char boundaries |
//! | invalid UTF-8 | each bad byte → `?` (1:1, so byte offsets stay exact) |
//! | `col`, `len` | moved to char boundaries; `len` clipped to the first line and to the window |
//! | invariant | `text[text_col..text_col + len]` is always a valid slice |

use margin_proto::limits::MAX_SNIPPET;
use margin_proto::{RelPath, Site};

/// Context kept before the match in a cut window.
const BEFORE: usize = 96;

/// Bytes → valid UTF-8 of the same length.
pub fn sanitize(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    let mut rest = b;
    loop {
        match std::str::from_utf8(rest) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let ok = e.valid_up_to();
                out.push_str(std::str::from_utf8(&rest[..ok]).unwrap_or_default());
                let bad = e.error_len().unwrap_or(rest.len() - ok).max(1);
                for _ in 0..bad {
                    out.push('?');
                }
                rest = &rest[(ok + bad).min(rest.len())..];
            }
        }
    }
}

fn is_cont(b: Option<&u8>) -> bool {
    matches!(b, Some(x) if (0x80..0xc0).contains(x))
}

/// `line`: the bytes of one line without `\n`. `col`/`len`: match in that line (any values; clamped).
pub fn make_site(path: &RelPath, line_no: u32, line: &[u8], col: usize, len: usize, context: String) -> Site {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let col = col.min(line.len());
    let len = len.min(line.len() - col);
    let (mut ws, mut we) = (0usize, line.len());
    if line.len() > MAX_SNIPPET {
        let mlen = len.min(MAX_SNIPPET);
        let before = col.min(BEFORE.min(MAX_SNIPPET - mlen));
        ws = col - before;
        we = (ws + MAX_SNIPPET).min(line.len());
        // Window edges on char starts.
        for _ in 0..3 {
            if is_cont(line.get(ws)) && ws < col {
                ws += 1;
            }
        }
        for _ in 0..3 {
            if is_cont(line.get(we)) && we > ws {
                we -= 1;
            }
        }
    }
    let text = sanitize(line.get(ws..we).unwrap_or_default());
    let mut tc = (col - ws).min(text.len());
    while tc > 0 && !text.is_char_boundary(tc) {
        tc -= 1;
    }
    let mut te = (col + len).saturating_sub(ws).min(text.len()).max(tc);
    while te < text.len() && !text.is_char_boundary(te) {
        te += 1;
    }
    Site { path: path.clone(), line: line_no, col: (ws + tc) as u32, len: (te - tc) as u32, text, text_col: tc as u32, context }
}

/// Byte range of line `n` (0-based) in `data`, without the `\n`.
pub fn line_range(data: &[u8], n: u32) -> Option<(usize, usize)> {
    let mut start = 0usize;
    for (seen, nl) in memchr::memchr_iter(b'\n', data).enumerate() {
        if seen as u32 == n {
            return Some((start, nl));
        }
        start = nl + 1;
    }
    let lines = memchr::memchr_iter(b'\n', data).count() as u32;
    (lines == n && start <= data.len()).then_some((start, data.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(s: &Site) {
        assert!(s.text.len() <= MAX_SNIPPET);
        let (a, b) = (s.text_col as usize, (s.text_col + s.len) as usize);
        assert!(s.text.get(a..b).is_some(), "{s:?}");
        assert!(!s.text.contains('\n'));
    }

    #[test]
    fn short_line() {
        let p = RelPath::new("a").unwrap();
        let s = make_site(&p, 3, b"let x = memcpy(a);\r", 8, 6, String::new());
        assert_eq!(s.text, "let x = memcpy(a);");
        assert_eq!(&s.text[s.text_col as usize..][..s.len as usize], "memcpy");
        check(&s);
    }

    #[test]
    fn long_lines_and_multibyte() {
        let p = RelPath::new("a").unwrap();
        let mut line = "é".repeat(2000).into_bytes();
        line.extend_from_slice(b"NEEDLE");
        line.extend("ü".repeat(2000).as_bytes());
        let col = 4000;
        let s = make_site(&p, 0, &line, col, 6, String::new());
        check(&s);
        assert_eq!(&s.text[s.text_col as usize..][..s.len as usize], "NEEDLE");
        assert_eq!(s.col as usize, col);
        // Match longer than the window.
        let s = make_site(&p, 0, &line, 10, 5000, String::new());
        check(&s);
        // Mid-char column, out-of-range values, invalid bytes.
        for (c, l) in [(1, 3), (99999, 4), (3999, 2), (0, 0)] {
            check(&make_site(&p, 0, &line, c, l, String::new()));
        }
        let bad = [b'a', 0xff, 0xc3, b'b', 0xe2, 0x82];
        let s = make_site(&p, 0, &bad, 1, 3, String::new());
        assert_eq!(s.text, "a??b??");
        check(&s);
    }

    #[test]
    fn lines() {
        let d = b"a\nbb\n\nccc";
        assert_eq!(line_range(d, 0), Some((0, 1)));
        assert_eq!(line_range(d, 1), Some((2, 4)));
        assert_eq!(line_range(d, 2), Some((5, 5)));
        assert_eq!(line_range(d, 3), Some((6, 9)));
        assert_eq!(line_range(d, 4), None);
    }
}
