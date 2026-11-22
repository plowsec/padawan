//! Line lexer for syntax colouring. Resumable: the state at the start of a line is a `LineState`.
//!
//! | File | Langs |
//! |---|---|
//! | `lex/clike.rs` | C, C++, Rust, Go, Java, JavaScript, TypeScript, C#, PHP, Kotlin, Swift (one lexer, one `Spec` each) |
//! | `lex/script.rs` | Python, Ruby, Lua |
//! | `lex/shell.rs` | Shell (also Dockerfile), Makefile |
//! | `lex/markdown.rs` | Markdown |
//! | `lex/data.rs` | JSON, TOML, YAML, SQL, Diff |
//! | `lex/web.rs` | HTML (with `<script>`/`<style>` bodies), CSS |
//!
//! `LineState` layout: bits 0–7 mode (0 = normal), 8–15 small parameter, 16–63 extra. Meaning is per lexer;
//! an unknown mode is read as normal. Tokens stop at `LEX_LINE_CAP` bytes; the state is still tracked to
//! the end of the line, so the next line starts right.

use crate::Lang;

mod clike;
mod data;
mod markdown;
mod script;
mod shell;
mod web;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// Identifiers and anything unclassified. Lexers may leave such ranges out entirely.
    Text,
    Keyword,
    /// Built-in and user type names where cheap to tell (`int`, `String`, capitalised names in Rust/Java…).
    Type,
    /// A name directly followed by `(` (call or definition).
    Function,
    String,
    /// Escape sequence inside a string.
    Escape,
    Number,
    Comment,
    Operator,
    Punct,
    /// `true`, `false`, `null`, `nil`, `None`, ALL_CAPS constants.
    Constant,
    /// `#[attr]`, `@decorator`, preprocessor directives, Rust macros `name!`.
    Attribute,
    /// Markdown heading line; YAML/TOML section header.
    Heading,
    /// Markdown `*emphasis*`.
    Emphasis,
    /// Markdown `**strong**`.
    Strong,
    /// Markdown link text / URL.
    Link,
    /// Markdown inline code and fenced code blocks.
    CodeSpan,
    /// HTML/XML tag name.
    Tag,
    /// Key of a key/value pair (JSON, TOML, YAML, HTML attribute, CSS property).
    Key,
    /// Markdown list bullet / quote marker; diff `+` line uses `Inserted`, `-` line `Deleted`.
    Marker,
    Inserted,
    Deleted,
}

/// Byte range `start..end` in the line, on char boundaries, sorted, non-overlapping. Gaps are plain text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub start: u32,
    pub end: u32,
    pub kind: TokenKind,
}

/// Lexer state at the start of a line (inside a block comment, a multi-line string, a code fence…).
/// Opaque. `LineState::default()` is the state at the start of a file.
/// Equal states + equal line text ⇒ equal tokens and equal next state (the editor relies on it to stop re-lexing).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LineState(pub u64);

/// `lex_line` emits tokens for the first `LEX_LINE_CAP` bytes of a line only (rounded down to a char boundary).
pub const LEX_LINE_CAP: usize = 16 * 1024;

/// Tokens of `line` (no line break inside) appended to `out`; returns the state for the next line.
/// Lines longer than a cap (the implementer picks one, ≥ 4096 bytes) may be left partly untokenised.
pub fn lex_line(lang: Lang, state: LineState, line: &str, out: &mut Vec<Token>) -> LineState {
    let limit = floor_boundary(line, LEX_LINE_CAP);
    lex_limited(lang, state, line, out, limit)
}

/// `lex_line` with an explicit token limit (`outline` passes `line.len()`).
pub(crate) fn lex_limited(lang: Lang, state: LineState, line: &str, out: &mut Vec<Token>, limit: usize) -> LineState {
    let mut em = Em { out, full: line, base: 0, limit, last: 0 };
    lex_into(lang, &mut em, line, state)
}

/// Dispatch. `line` may be a sub-slice of `em.full` starting at `em.base` (HTML embeds).
pub(crate) fn lex_into(lang: Lang, em: &mut Em, line: &str, state: LineState) -> LineState {
    match lang {
        Lang::Plain => LineState::default(),
        Lang::C => clike::lex(em, line, state, &clike::C),
        Lang::Cpp => clike::lex(em, line, state, &clike::CPP),
        Lang::Rust => clike::lex(em, line, state, &clike::RUST),
        Lang::Go => clike::lex(em, line, state, &clike::GO),
        Lang::Java => clike::lex(em, line, state, &clike::JAVA),
        Lang::JavaScript => clike::lex(em, line, state, &clike::JS),
        Lang::TypeScript => clike::lex(em, line, state, &clike::TS),
        Lang::CSharp => clike::lex(em, line, state, &clike::CSHARP),
        Lang::Php => clike::lex(em, line, state, &clike::PHP),
        Lang::Kotlin => clike::lex(em, line, state, &clike::KOTLIN),
        Lang::Swift => clike::lex(em, line, state, &clike::SWIFT),
        Lang::Python => script::python(em, line, state),
        Lang::Ruby => script::ruby(em, line, state),
        Lang::Lua => script::lua(em, line, state),
        Lang::Shell => shell::shell(em, line, state),
        Lang::Makefile => shell::makefile(em, line, state),
        Lang::Markdown => markdown::lex(em, line, state),
        Lang::Json => data::json(em, line, state),
        Lang::Toml => data::toml(em, line, state),
        Lang::Yaml => data::yaml(em, line, state),
        Lang::Sql => data::sql(em, line, state),
        Lang::Diff => data::diff(em, line),
        Lang::Html => web::html(em, line, state),
        Lang::Css => web::css(em, line, state),
    }
}

/// Token sink: clamps to the limit, keeps tokens sorted and non-overlapping, drops off-boundary ranges.
pub(crate) struct Em<'a> {
    out: &'a mut Vec<Token>,
    /// Whole line (for boundary checks).
    full: &'a str,
    /// Offset of the slice being lexed inside `full`.
    pub(crate) base: usize,
    limit: usize,
    last: usize,
}

impl Em<'_> {
    /// Range relative to the slice being lexed.
    #[inline]
    pub(crate) fn push(&mut self, s: usize, e: usize, kind: TokenKind) {
        let s = self.base.saturating_add(s).max(self.last);
        let e = self.base.saturating_add(e).min(self.limit);
        if s >= e || e > u32::MAX as usize {
            return;
        }
        if !self.full.is_char_boundary(s) || !self.full.is_char_boundary(e) {
            return;
        }
        self.out.push(Token { start: s as u32, end: e as u32, kind });
        self.last = e;
    }
}

// ---- state packing ----

pub(crate) fn st(mode: u8, param: u8, extra: u64) -> LineState {
    LineState(u64::from(mode) | (u64::from(param) << 8) | ((extra & 0xFFFF_FFFF_FFFF) << 16))
}
pub(crate) fn mode(s: LineState) -> u8 {
    (s.0 & 0xFF) as u8
}
pub(crate) fn param(s: LineState) -> u8 {
    ((s.0 >> 8) & 0xFF) as u8
}
pub(crate) fn extra(s: LineState) -> u64 {
    s.0 >> 16
}

/// Up to 6 bytes of a delimiter packed in `extra` (heredocs, C++ raw strings).
pub(crate) fn pack_delim(d: &[u8]) -> u64 {
    let mut v = 0u64;
    for (k, &c) in d.iter().take(6).enumerate() {
        v |= u64::from(c) << (8 * k);
    }
    v
}
/// Does `d` match a delimiter packed by `pack_delim` with full length `len`?
pub(crate) fn delim_matches(d: &[u8], len: usize, packed: u64) -> bool {
    d.len() == len && pack_delim(d) == packed
}

// ---- byte helpers (all positions they return are char boundaries when the input position is) ----

pub(crate) fn floor_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut i = i;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[inline]
pub(crate) fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}
#[inline]
pub(crate) fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

/// End of an identifier starting at `i` (non-ASCII bytes count as identifier bytes, so runs stay whole).
#[inline]
pub(crate) fn word_end(b: &[u8], i: usize, dollar: bool) -> usize {
    let mut j = i;
    while let Some(&c) = b.get(j) {
        if is_ident_char(c) || (dollar && c == b'$') {
            j += 1;
        } else {
            break;
        }
    }
    j
}

/// Next char boundary after `i`.
#[inline]
pub(crate) fn next_char(b: &[u8], i: usize) -> usize {
    let mut j = i.saturating_add(1);
    while let Some(&c) = b.get(j) {
        if (0x80..0xC0).contains(&c) {
            j += 1;
        } else {
            break;
        }
    }
    j.min(b.len())
}

#[inline]
pub(crate) fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while let Some(&c) = b.get(i) {
        if c == b' ' || c == b'\t' || c == b'\r' || c == 0x0c {
            i += 1;
        } else {
            break;
        }
    }
    i
}

/// End of a number literal starting at `i` (digit, or `.` + digit). Covers hex, binary, suffixes, exponents,
/// `_` separators; `quote_sep` also allows C++ `1'000`.
pub(crate) fn number_end(b: &[u8], i: usize, quote_sep: bool) -> usize {
    let mut j = i;
    let hex = b.get(i) == Some(&b'0') && matches!(b.get(i + 1), Some(b'x' | b'X'));
    while let Some(&c) = b.get(j) {
        if c.is_ascii_alphanumeric() || c == b'_' {
            j += 1;
        } else if c == b'.' {
            // `1..2` is a range, `1.foo` a method call.
            match b.get(j + 1) {
                Some(&d) if d.is_ascii_digit() => j += 1,
                Some(b'.') => break,
                Some(&d) if is_ident_start(d) => break,
                _ => j += 1,
            }
        } else if ((c == b'+' || c == b'-') && !hex && j > i && matches!(b.get(j - 1), Some(b'e' | b'E')))
            || (quote_sep && c == b'\'' && b.get(j + 1).is_some_and(|d| d.is_ascii_hexdigit()))
        {
            j += 1;
        } else {
            break;
        }
    }
    j
}

/// End of an escape sequence; `b[i]` is the backslash.
pub(crate) fn escape_end(b: &[u8], i: usize) -> usize {
    let Some(&c) = b.get(i + 1) else {
        return b.len().min(i + 1);
    };
    let hex_run = |from: usize, max: usize| {
        let mut j = from;
        while j < from + max && b.get(j).is_some_and(|d| d.is_ascii_hexdigit()) {
            j += 1;
        }
        j
    };
    match c {
        b'x' => hex_run(i + 2, 2),
        b'u' if b.get(i + 2) == Some(&b'{') => {
            let mut j = i + 3;
            while j < i + 12 && b.get(j).is_some_and(|d| d.is_ascii_hexdigit()) {
                j += 1;
            }
            if b.get(j) == Some(&b'}') {
                j + 1
            } else {
                j
            }
        }
        b'u' => hex_run(i + 2, 4),
        b'U' => hex_run(i + 2, 8),
        b'0'..=b'7' => {
            let mut j = i + 2;
            while j < i + 4 && b.get(j).is_some_and(|d| (b'0'..=b'7').contains(d)) {
                j += 1;
            }
            j
        }
        _ => next_char(b, i + 1),
    }
}

/// Scan string text from `i` up to the closing `quote`, emitting `String` and `Escape` pieces.
/// `seg` is where the current `String` piece starts (may be before `i` to include the opening quote).
/// Returns (position after the close or end of line, closed).
pub(crate) fn string_body(em: &mut Em, b: &[u8], seg: usize, i: usize, quote: u8, escapes: bool) -> (usize, bool) {
    let mut seg = seg;
    let mut j = i;
    while let Some(&c) = b.get(j) {
        if c == quote {
            em.push(seg, j + 1, TokenKind::String);
            return (j + 1, true);
        }
        if escapes && c == b'\\' {
            em.push(seg, j, TokenKind::String);
            let e = escape_end(b, j);
            em.push(j, e, TokenKind::Escape);
            j = e;
            seg = e;
            continue;
        }
        j += 1;
    }
    em.push(seg, b.len(), TokenKind::String);
    (b.len(), false)
}

/// ALL_CAPS constant: ≥ 2 bytes, ≥ 1 uppercase letter, only `A-Z0-9_`, not starting with a digit.
pub(crate) fn is_all_caps(w: &[u8]) -> bool {
    w.len() >= 2
        && !w[0].is_ascii_digit()
        && w.iter().any(u8::is_ascii_uppercase)
        && w.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
}

/// Sorted-table lookup (tables are checked sorted by a test).
#[inline]
pub(crate) fn in_table(t: &[&str], w: &str) -> bool {
    t.binary_search(&w).is_ok()
}

/// Lowercase copy of a short ASCII word in `buf`; `None` if too long or not ASCII.
pub(crate) fn lower<'b>(w: &[u8], buf: &'b mut [u8; 32]) -> Option<&'b str> {
    if w.len() > buf.len() || !w.is_ascii() {
        return None;
    }
    for (d, s) in buf.iter_mut().zip(w) {
        *d = s.to_ascii_lowercase();
    }
    std::str::from_utf8(buf.get(..w.len())?).ok()
}

/// Every keyword/type table of every lexer, for the sortedness test.
#[cfg(test)]
pub(crate) fn all_tables() -> Vec<(&'static str, &'static [&'static str])> {
    let mut v = clike::tables();
    v.extend(script::tables());
    v.extend(shell::tables());
    v.extend(data::tables());
    v.extend(web::tables());
    v
}

#[cfg(test)]
mod tests {
    #[test]
    fn tables_sorted_and_unique() {
        for (name, t) in super::all_tables() {
            for w in t.windows(2) {
                assert!(w[0] < w[1], "table {name} not sorted/unique at {:?} {:?}", w[0], w[1]);
            }
        }
    }
}
