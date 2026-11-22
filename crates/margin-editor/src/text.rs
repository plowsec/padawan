//! Char classes and boundary helpers. Pure, no egui.
//!
//! | Fn | What |
//! |---|---|
//! | `is_hazard` | char drawn as a boxed `U+XXXX` code, never laid out raw |
//! | `is_combining` | mark that sticks to the previous char (cursor skips it) |
//! | `is_ident` | identifier char (word highlight, cmd+click, whole-word find) |
//! | `next_boundary` / `prev_boundary` | cursor step: one char plus trailing combining marks |
//! | `word_left` / `word_right` | word-wise cursor step |

/// Control, bidi, zero-width and other default-ignorable chars (Trojan Source, smuggling). Tab is not one.
pub fn is_hazard(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x00..=0x08 | 0x0A..=0x1F | 0x7F..=0x9F // C0 (not tab), DEL, C1
        | 0xAD // soft hyphen
        | 0x034F // combining grapheme joiner
        | 0x061C // arabic letter mark
        | 0x115F | 0x1160 | 0x17B4 | 0x17B5 | 0x180B..=0x180F
        | 0x200B..=0x200F // zero-width, LRM, RLM
        | 0x2028 | 0x2029 // line / paragraph separator
        | 0x202A..=0x202E // bidi embeddings / overrides
        | 0x2060..=0x206F // word joiner, invisible operators, bidi isolates (2066-2069), deprecated
        | 0x3164 | 0xFFA0 // hangul fillers
        | 0xFE00..=0xFE0F // variation selectors
        | 0xFEFF // BOM / ZWNBSP inside a line
        | 0xFFF0..=0xFFFB // unassigned specials, interlinear annotation
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A
        | 0xE0000..=0xE0FFF // tags, variation selectors supplement
    )
}

/// Label drawn for a hazard char: `U+202E`.
pub fn hazard_label(c: char) -> String {
    format!("U+{:04X}", c as u32)
}

/// Combining mark (common blocks). The cursor moves over base + marks as one unit.
pub fn is_combining(c: char) -> bool {
    let u = c as u32;
    matches!(u, 0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A | 0x064B..=0x065F
        | 0x0900..=0x0903 | 0x093A..=0x094F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
        && u != 0x034F
}

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Identifier char, `$` included where the language uses it.
pub fn is_ident_lang(c: char, dollar: bool) -> bool {
    is_ident(c) || (dollar && c == '$')
}

/// Largest char boundary `<= i` (clamped to `s.len()`).
pub fn floor_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Boundary after the char at `i` and any combining marks after it.
pub fn next_boundary(s: &str, i: usize) -> usize {
    let i = floor_boundary(s, i);
    let mut it = s[i..].char_indices();
    let Some((_, c)) = it.next() else { return s.len() };
    let mut end = i + c.len_utf8();
    for (o, c) in it {
        if !is_combining(c) {
            return i + o;
        }
        end = i + o + c.len_utf8();
    }
    end
}

/// Boundary before the char ending at `i`, skipping back over combining marks to their base.
pub fn prev_boundary(s: &str, i: usize) -> usize {
    let i = floor_boundary(s, i);
    let mut it = s[..i].char_indices().rev();
    for (o, c) in it.by_ref() {
        if !is_combining(c) {
            return o;
        }
        if o == 0 {
            return 0;
        }
    }
    0
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if is_ident(c) || is_combining(c) {
        Class::Word
    } else {
        Class::Punct
    }
}

/// Word step right: skip spaces, then one run of the same class (word or punctuation).
pub fn word_right(s: &str, i: usize) -> usize {
    let i = floor_boundary(s, i);
    let mut it = s[i..].char_indices().peekable();
    while let Some(&(_, c)) = it.peek() {
        if class(c) != Class::Space {
            break;
        }
        it.next();
    }
    let Some(&(_, first)) = it.peek() else { return s.len() };
    let k = class(first);
    for (o, c) in it {
        if class(c) != k {
            return i + o;
        }
    }
    s.len()
}

/// Word step left: mirror of `word_right`.
pub fn word_left(s: &str, i: usize) -> usize {
    let i = floor_boundary(s, i);
    let mut it = s[..i].char_indices().rev().peekable();
    while let Some(&(_, c)) = it.peek() {
        if class(c) != Class::Space {
            break;
        }
        it.next();
    }
    let Some(&(_, first)) = it.peek() else { return 0 };
    let k = class(first);
    let mut at = 0;
    for (o, c) in it {
        if class(c) != k {
            return at;
        }
        at = o;
    }
    at
}

/// Identifier around byte `i` (touching it on either side): byte range, or `None`.
pub fn ident_at(s: &str, i: usize, dollar: bool) -> Option<(usize, usize)> {
    let i = floor_boundary(s, i);
    let id = |c: char| is_ident_lang(c, dollar) || is_combining(c);
    let mut a = i;
    for (o, c) in s[..i].char_indices().rev() {
        if !id(c) {
            break;
        }
        a = o;
    }
    let mut b = i;
    for (o, c) in s[i..].char_indices() {
        if !id(c) {
            break;
        }
        b = i + o + c.len_utf8();
    }
    let w = &s[a..b];
    // Not a pure number, not empty.
    (a < b && !w.chars().next().is_some_and(|c| c.is_ascii_digit())).then_some((a, b))
}

/// Word (any run of the char's class) around `i`, for double-click.
pub fn word_range_at(s: &str, i: usize) -> (usize, usize) {
    let i = floor_boundary(s, i);
    let pick = s[i..].chars().next().or_else(|| s[..i].chars().next_back());
    let Some(c0) = pick else { return (i, i) };
    let k = class(c0);
    let mut a = i;
    for (o, c) in s[..i].char_indices().rev() {
        if class(c) != k {
            break;
        }
        a = o;
    }
    let mut b = i;
    for (o, c) in s[i..].char_indices() {
        if class(c) != k {
            break;
        }
        b = i + o + c.len_utf8();
    }
    (a, b)
}

/// Leading spaces/tabs, in bytes.
pub fn indent_len(s: &str) -> usize {
    s.len() - s.trim_start_matches([' ', '\t']).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries_skip_combining() {
        let s = "ae\u{301}\u{302}b";
        assert_eq!(next_boundary(s, 0), 1);
        assert_eq!(next_boundary(s, 1), 6);
        assert_eq!(prev_boundary(s, 6), 1);
        assert_eq!(prev_boundary(s, 7), 6);
        assert_eq!(next_boundary(s, 3), 6); // from inside a sequence: floor first
        assert_eq!(next_boundary("", 0), 0);
        assert_eq!(prev_boundary("é", 1), 0);
    }

    #[test]
    fn words_unicode() {
        let s = "  héllo_wörld, 世界 ++x";
        assert_eq!(word_right(s, 0), "  héllo_wörld".len());
        let w = word_right(s, "  héllo_wörld".len());
        assert_eq!(&s[..w], "  héllo_wörld,");
        assert_eq!(word_left(s, s.len()), s.len() - 1);
        assert_eq!(word_left(s, s.len() - 1), s.len() - 3);
        assert_eq!(word_left(s, "  h".len()), 2);
        assert_eq!(ident_at(s, 4, false), Some((2, "  héllo_wörld".len())));
        assert_eq!(ident_at("12 ab", 1, false), None);
        assert_eq!(ident_at("$x", 1, true), Some((0, 2)));
        assert_eq!(word_range_at("ab  cd", 3), (2, 4));
    }

    #[test]
    fn hazards() {
        for c in ['\u{202E}', '\u{2066}', '\u{200B}', '\u{FEFF}', '\0', '\x1b', '\u{7f}', '\u{85}', '\u{E0041}', '\r'] {
            assert!(is_hazard(c), "{c:?}");
        }
        for c in ['\t', ' ', 'a', 'é', '世', '\u{301}'] {
            assert!(!is_hazard(c), "{c:?}");
        }
        assert_eq!(hazard_label('\u{202E}'), "U+202E");
        assert_eq!(hazard_label('\u{E0041}'), "U+E0041");
    }

    #[test]
    fn indents() {
        assert_eq!(indent_len(" \t x"), 3);
        assert_eq!(indent_len("   "), 3);
    }
}
