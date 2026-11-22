//! Engine strings are hostile. Every one shown outside the editor goes through `sanitize`.
//!
//! | Input | Shown as |
//! |---|---|
//! | control chars (C0, C1, DEL), tab, line breaks, U+2028/2029 | `·` |
//! | bidi overrides/isolates/marks (U+202A–202E, U+2066–2069, U+200E/F, U+061C) | `·` |
//! | zero-width / invisible (U+200B–200D, U+2060–2064, U+FEFF, U+00AD, U+034F, U+180E, fillers, tags, variation selectors) | `·` |
//! | longer than `max` chars | first `max - 1` chars + `…` |
//!
//! One char in, one char out: char indices survive, so a highlight can be split before sanitising.

/// Placeholder for every hidden or dangerous character.
pub const MARK: char = '\u{b7}';

/// Default cap for names and paths in lists.
pub const MAX_SHOWN: usize = 300;

/// Character that must not reach the screen as is.
pub fn is_hidden(c: char) -> bool {
    let u = c as u32;
    c.is_control()
        || matches!(u,
            0x00AD | 0x034F | 0x061C | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F
            | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F
            | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFFB | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
}

/// Safe display form of `s`, at most `max` chars (≥ 1).
pub fn sanitize(s: &str, max: usize) -> String {
    let max = max.max(1);
    let mut out = String::with_capacity(s.len().min(max * 4));
    let mut n = 0;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if n + 1 == max && it.peek().is_some() {
            out.push('\u{2026}');
            return out;
        }
        out.push(if is_hidden(c) { MARK } else { c });
        n += 1;
    }
    out
}

/// `sanitize` with the default cap.
pub fn clean(s: &str) -> String {
    sanitize(s, MAX_SHOWN)
}

/// Largest char boundary ≤ `i` (clamped to the length). Use on every byte offset from the engine.
pub fn floor_char(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `s` split at engine-given byte offsets `[start, start+len)`, both clamped to char boundaries.
pub fn split3(s: &str, start: u32, len: u32) -> (&str, &str, &str) {
    let a = floor_char(s, start as usize);
    let b = floor_char(s, (start as usize).saturating_add(len as usize)).max(a);
    (&s[..a], &s[a..b], &s[b..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_inputs_become_visible() {
        let cases = [
            ("evil\u{202e}gpj.exe", "evil·gpj.exe"),
            ("a\nb\r\nc", "a·b··c"),
            ("tab\there", "tab·here"),
            ("zero\u{200b}width\u{feff}", "zero·width·"),
            ("iso\u{2066}x\u{2069}", "iso·x·"),
            ("nul\0bel\u{7}del\u{7f}c1\u{85}", "nul·bel·del·c1·"),
            ("tag\u{e0041}", "tag·"),
            ("ls\u{2028}ps\u{2029}", "ls·ps·"),
            ("soft\u{ad}hyphen", "soft·hyphen"),
            ("plain é 中文 ok", "plain é 中文 ok"),
        ];
        for (input, want) in cases {
            assert_eq!(sanitize(input, 100), want, "{input:?}");
        }
    }

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(sanitize("abcdef", 4), "abc…");
        assert_eq!(sanitize("abcd", 4), "abcd");
        assert_eq!(sanitize("", 4), "");
        assert_eq!(sanitize("ab", 0), "…");
        let long = "é".repeat(10_000);
        assert_eq!(sanitize(&long, 10).chars().count(), 10);
    }

    #[test]
    fn offsets_are_clamped() {
        let s = "aé中b";
        for i in 0..20 {
            assert!(s.is_char_boundary(floor_char(s, i)));
        }
        for start in 0..12 {
            for len in [0, 1, 2, 3, 100, u32::MAX] {
                let (a, b, c) = split3(s, start, len);
                assert_eq!(format!("{a}{b}{c}"), s);
            }
        }
        assert_eq!(split3(s, 1, 2), ("a", "é", "中b"));
        assert_eq!(split3(s, u32::MAX, u32::MAX), (s, "", ""));
    }
}
