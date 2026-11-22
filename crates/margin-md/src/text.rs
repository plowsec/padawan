//! Text helpers: hostile-character masking, entities, slugs, char-boundary-safe slicing.

/// Shown in place of a hidden or control character.
pub const MASK: char = '\u{b7}';

/// Bidi controls, invisible / zero-width characters, tags, variation selectors, C0/C1 controls (tab and
/// newline excepted: callers decide).
pub fn is_hazard(c: char) -> bool {
    matches!(c as u32,
        0x00..=0x08 | 0x0b..=0x1f | 0x7f..=0x9f
        | 0xad | 0x34f | 0x61c | 0x115f | 0x1160 | 0x17b4 | 0x17b5 | 0x180b..=0x180f
        | 0x200b..=0x200f | 0x2028..=0x202e | 0x2060..=0x206f | 0x3164
        | 0xfe00..=0xfe0f | 0xfeff | 0xffa0 | 0xfff9..=0xfffb
        | 0x1d173..=0x1d17a | 0xe0000..=0xe0fff)
}

/// Append `s` to `out` with hazards masked. Tab → space, newline kept only if `keep_nl`.
pub fn clean_into(out: &mut String, s: &str, keep_nl: bool) {
    for c in s.chars() {
        match c {
            '\n' if keep_nl => out.push('\n'),
            '\t' => out.push(' '),
            '\n' | '\r' => out.push(' '),
            c if is_hazard(c) => out.push(MASK),
            c => out.push(c),
        }
    }
}

pub fn clean(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    clean_into(&mut o, s, false);
    o
}

/// Largest char boundary `<= i`.
pub fn floor(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut i = i;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `s` cut to at most `max` bytes (on a char boundary).
pub fn cap(s: &str, max: usize) -> &str {
    s.get(..floor(s, max)).unwrap_or("")
}

/// Leading spaces (tabs are expanded before block parsing).
pub fn indent_of(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ').count()
}

pub fn is_blank(s: &str) -> bool {
    s.bytes().all(|b| b == b' ' || b == b'\t')
}

/// Tail of `s` from byte `i` (empty if out of range / not a boundary).
pub fn from(s: &str, i: usize) -> &str {
    s.get(i..).unwrap_or("")
}

/// Expand tabs to spaces (tab stop 4).
pub fn expand_tabs(line: &str) -> std::borrow::Cow<'_, str> {
    if !line.contains('\t') {
        return std::borrow::Cow::Borrowed(line);
    }
    let mut o = String::with_capacity(line.len() + 16);
    let mut col = 0usize;
    for c in line.chars() {
        if c == '\t' {
            let n = 4 - col % 4;
            for _ in 0..n {
                o.push(' ');
            }
            col += n;
        } else {
            o.push(c);
            col += 1;
        }
    }
    std::borrow::Cow::Owned(o)
}

pub fn is_punct(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_punctuation()
    } else {
        !c.is_alphanumeric() && !c.is_whitespace()
    }
}

/// Named entities supported (the common ones). Others stay literal.
const ENTITIES: &[(&str, char)] = &[
    ("amp", '&'), ("lt", '<'), ("gt", '>'), ("quot", '"'), ("apos", '\''), ("nbsp", '\u{a0}'),
    ("copy", '\u{a9}'), ("reg", '\u{ae}'), ("trade", '\u{2122}'), ("hellip", '\u{2026}'),
    ("mdash", '\u{2014}'), ("ndash", '\u{2013}'), ("lsquo", '\u{2018}'), ("rsquo", '\u{2019}'),
    ("ldquo", '\u{201c}'), ("rdquo", '\u{201d}'), ("laquo", '\u{ab}'), ("raquo", '\u{bb}'),
    ("middot", '\u{b7}'), ("bull", '\u{2022}'), ("deg", '\u{b0}'), ("plusmn", '\u{b1}'),
    ("times", '\u{d7}'), ("divide", '\u{f7}'), ("euro", '\u{20ac}'), ("pound", '\u{a3}'),
    ("yen", '\u{a5}'), ("cent", '\u{a2}'), ("sect", '\u{a7}'), ("para", '\u{b6}'),
    ("larr", '\u{2190}'), ("rarr", '\u{2192}'), ("uarr", '\u{2191}'), ("darr", '\u{2193}'),
    ("harr", '\u{2194}'), ("le", '\u{2264}'), ("ge", '\u{2265}'), ("ne", '\u{2260}'),
    ("asymp", '\u{2248}'), ("infin", '\u{221e}'), ("micro", '\u{b5}'), ("frac12", '\u{bd}'),
    ("frac14", '\u{bc}'), ("frac34", '\u{be}'), ("iexcl", '\u{a1}'), ("iquest", '\u{bf}'),
    ("shy", '\u{ad}'), ("zwj", '\u{200d}'), ("zwnj", '\u{200c}'), ("ensp", '\u{2002}'),
    ("emsp", '\u{2003}'), ("thinsp", '\u{2009}'), ("check", '\u{2713}'),
];

/// Entity at the start of `s` (which starts with `&`): `(char, bytes consumed)`.
pub fn entity(s: &str) -> Option<(char, usize)> {
    let b = s.as_bytes();
    if b.first() != Some(&b'&') {
        return None;
    }
    if b.get(1) == Some(&b'#') {
        let (hex, start) = match b.get(2) {
            Some(b'x' | b'X') => (true, 3),
            _ => (false, 2),
        };
        let max = if hex { 6 } else { 7 };
        let mut i = start;
        let mut v: u32 = 0;
        while i < b.len() && i - start < max {
            let d = match (hex, b[i]) {
                (_, c @ b'0'..=b'9') => u32::from(c - b'0'),
                (true, c @ b'a'..=b'f') => u32::from(c - b'a' + 10),
                (true, c @ b'A'..=b'F') => u32::from(c - b'A' + 10),
                _ => break,
            };
            v = v * if hex { 16 } else { 10 } + d;
            i += 1;
        }
        if i == start || b.get(i) != Some(&b';') {
            return None;
        }
        let c = if v == 0 { '\u{fffd}' } else { char::from_u32(v).unwrap_or('\u{fffd}') };
        return Some((c, i + 1));
    }
    let mut i = 1;
    while i < b.len() && i <= 32 && b[i].is_ascii_alphanumeric() {
        i += 1;
    }
    if i == 1 || b.get(i) != Some(&b';') {
        return None;
    }
    let name = s.get(1..i)?;
    ENTITIES.iter().find(|(n, _)| *n == name).map(|&(_, c)| (c, i + 1))
}

/// GitHub heading slug (before de-duplication): lowercase, spaces → `-`, punctuation other than `-`/`_` dropped.
pub fn slug(text: &str) -> String {
    let mut o = String::with_capacity(text.len());
    for c in text.trim().chars() {
        if c == ' ' {
            o.push('-');
        } else if c == '-' || c == '_' || c.is_alphanumeric() {
            o.extend(c.to_lowercase());
        }
    }
    o
}

/// Percent-decode a URL fragment (invalid sequences kept literally).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = |c: u8| (c as char).to_digit(16);
            if let (Some(x), Some(y)) = (h(b[i + 1]), h(b[i + 2])) {
                out.push((x * 16 + y) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// Normalised link label: case-folded, inner whitespace collapsed.
pub fn norm_label(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for w in s.split_whitespace() {
        if !o.is_empty() {
            o.push(' ');
        }
        o.extend(w.chars().flat_map(char::to_lowercase));
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities() {
        assert_eq!(entity("&amp;x"), Some(('&', 5)));
        assert_eq!(entity("&#123;"), Some(('{', 6)));
        assert_eq!(entity("&#x1F;"), Some(('\u{1f}', 6)));
        assert_eq!(entity("&#0;"), Some(('\u{fffd}', 4)));
        assert_eq!(entity("&#xD800;"), Some(('\u{fffd}', 8)));
        assert_eq!(entity("&bogus;"), None);
        assert_eq!(entity("&amp"), None);
        assert_eq!(entity("&#12345678;"), None);
    }

    #[test]
    fn hazards_are_masked() {
        let s = clean("a\u{202e}b\u{200b}c\u{feff}d\u{e0041}e\u{fe0f}f\u{1}g\u{85}h\u{ad}i\u{2066}j");
        assert_eq!(s, "a·b·c·d·e·f·g·h·i·j");
        assert_eq!(clean("x\ty"), "x y");
    }

    #[test]
    fn slugs_and_labels() {
        assert_eq!(slug("Hello, World!"), "hello-world");
        assert_eq!(slug("  A  b "), "a--b");
        assert_eq!(slug("Ünïcode_ok-1"), "ünïcode_ok-1");
        assert_eq!(norm_label("  Foo \n BAR "), "foo bar");
        assert_eq!(percent_decode("h%C3%A9llo%"), "héllo%");
        assert_eq!(floor("é", 1), 0);
        assert_eq!(cap("héllo", 2), "h");
    }
}
