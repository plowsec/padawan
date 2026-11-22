//! HTML/XML and CSS.
//!
//! | Lang | State | Notes |
//! |---|---|---|
//! | HTML | 1 comment, 2 inside a tag, 3 `<script>` body (extra = JS state), 4 `<style>` body (extra = CSS state), 5/6 inside a `<script`/`<style` tag | tag → `Tag`, attribute → `Key`, value → `String`, `&amp;` → `Escape`; bodies use the JS / CSS lexers |
//! | CSS | mode 1 comment; param = brace depth | selector names → `Tag`, `.class`/`#id` → `Type`, `:hover` → `Keyword`, property → `Key`, `@media` → `Attribute` |

use super::{
    clike, extra, is_ident_char, is_ident_start, mode, next_char, number_end, param, skip_ws, st,
    string_body, Em, LineState, TokenKind as K,
};

const H_COMMENT: u8 = 1;
const H_TAG: u8 = 2;
const H_SCRIPT: u8 = 3;
const H_STYLE: u8 = 4;
const H_TAG_SCRIPT: u8 = 5;
const H_TAG_STYLE: u8 = 6;

#[cfg(test)]
pub(crate) fn tables() -> Vec<(&'static str, &'static [&'static str])> {
    Vec::new()
}

fn find_ci(b: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    b.get(from..)?.windows(pat.len()).position(|w| w.eq_ignore_ascii_case(pat)).map(|p| p + from)
}

fn find(b: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    b.get(from..)?.windows(pat.len()).position(|w| w == pat).map(|p| p + from)
}

fn is_name_char(c: u8) -> bool {
    is_ident_char(c) || matches!(c, b'-' | b':' | b'.')
}

/// Lex `line[from..to]` with an embedded lexer.
fn embed(em: &mut Em, line: &str, from: usize, to: usize, f: impl FnOnce(&mut Em, &str) -> LineState) -> LineState {
    let Some(sub) = line.get(from..to) else {
        return LineState::default();
    };
    let old = em.base;
    em.base = old.saturating_add(from);
    let s = f(em, sub);
    em.base = old;
    s
}

pub(crate) fn html(em: &mut Em, line: &str, state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0;
    let mut m = mode(state);
    let mut inner = LineState(extra(state) & 0xFFFF);
    loop {
        match m {
            H_SCRIPT | H_STYLE => {
                let close: &[u8] = if m == H_SCRIPT { b"</script" } else { b"</style" };
                let end = find_ci(b, i, close);
                let to = end.unwrap_or(n);
                let s = if m == H_SCRIPT {
                    embed(em, line, i, to, |em, sub| clike::lex(em, sub, inner, &clike::JS))
                } else {
                    embed(em, line, i, to, |em, sub| css(em, sub, inner))
                };
                match end {
                    Some(p) => {
                        i = p;
                        m = 0;
                        inner = LineState::default();
                    }
                    None => return st(m, 0, s.0 & 0xFFFF),
                }
            }
            H_COMMENT => match find(b, i, b"-->") {
                Some(p) => {
                    em.push(i, p + 3, K::Comment);
                    i = p + 3;
                    m = 0;
                }
                None => {
                    em.push(i, n, K::Comment);
                    return st(H_COMMENT, 0, 0);
                }
            },
            H_TAG | H_TAG_SCRIPT | H_TAG_STYLE => {
                // Attributes up to `>`.
                loop {
                    let Some(&c) = b.get(i) else {
                        return st(m, 0, 0);
                    };
                    match c {
                        b'>' => {
                            em.push(i, i + 1, K::Punct);
                            i += 1;
                            m = match m {
                                H_TAG_SCRIPT => H_SCRIPT,
                                H_TAG_STYLE => H_STYLE,
                                _ => 0,
                            };
                            break;
                        }
                        b'/' | b'?' if b.get(i + 1) == Some(&b'>') => {
                            em.push(i, i + 2, K::Punct);
                            i += 2;
                            m = 0;
                            break;
                        }
                        b'"' | b'\'' => i = string_body(em, b, i, i + 1, c, false).0,
                        b'=' => {
                            em.push(i, i + 1, K::Operator);
                            i += 1;
                            let j = skip_ws(b, i);
                            if b.get(j).is_some_and(|&d| !matches!(d, b'"' | b'\'' | b'>' | b' ')) {
                                let mut e = j;
                                while b.get(e).is_some_and(|&d| !d.is_ascii_whitespace() && d != b'>') {
                                    e += 1;
                                }
                                em.push(j, e, K::String);
                                i = e;
                            }
                        }
                        _ if is_name_char(c) => {
                            let mut e = i;
                            while b.get(e).is_some_and(|&d| is_name_char(d)) {
                                e += 1;
                            }
                            em.push(i, e, K::Key);
                            i = e;
                        }
                        _ => i = next_char(b, i),
                    }
                }
            }
            _ => {
                // Text.
                let Some(&c) = b.get(i) else {
                    return LineState::default();
                };
                match c {
                    b'&' => {
                        let mut e = i + 1;
                        while e < i + 33 && b.get(e).is_some_and(|&d| d.is_ascii_alphanumeric() || d == b'#') {
                            e += 1;
                        }
                        if b.get(e) == Some(&b';') && e > i + 1 {
                            em.push(i, e + 1, K::Escape);
                            i = e + 1;
                        } else {
                            i += 1;
                        }
                    }
                    b'<' if b.get(i + 1..i + 4) == Some(b"!--") => match find(b, i + 4, b"-->") {
                        Some(p) => {
                            em.push(i, p + 3, K::Comment);
                            i = p + 3;
                        }
                        None => {
                            em.push(i, n, K::Comment);
                            return st(H_COMMENT, 0, 0);
                        }
                    },
                    b'<' if matches!(b.get(i + 1), Some(b'!' | b'?')) => {
                        let e = b.get(i..).and_then(|r| r.iter().position(|&d| d == b'>')).map_or(n, |p| i + p + 1);
                        em.push(i, e, K::Keyword);
                        i = e;
                    }
                    b'<' if b.get(i + 1).is_some_and(|&d| is_ident_start(d) || d == b'/') => {
                        let closing = b.get(i + 1) == Some(&b'/');
                        let ns = if closing { i + 2 } else { i + 1 };
                        em.push(i, ns, K::Punct);
                        let mut e = ns;
                        while b.get(e).is_some_and(|&d| is_name_char(d)) {
                            e += 1;
                        }
                        em.push(ns, e, K::Tag);
                        let name = b.get(ns..e).unwrap_or(b"");
                        m = if closing {
                            H_TAG
                        } else if name.eq_ignore_ascii_case(b"script") {
                            H_TAG_SCRIPT
                        } else if name.eq_ignore_ascii_case(b"style") {
                            H_TAG_STYLE
                        } else {
                            H_TAG
                        };
                        i = e;
                    }
                    _ => i = next_char(b, i),
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Sel,
    Prop,
    Value,
}

/// Inside braces: a selector when a `{` comes before the next `;` or `}` (nested rules, `@media` bodies).
fn ctx_at(b: &[u8], i: usize, depth: u32) -> Ctx {
    if depth == 0 {
        return Ctx::Sel;
    }
    match b.get(i..).and_then(|r| r.iter().find(|&&c| matches!(c, b'{' | b';' | b'}'))) {
        Some(b'{') => Ctx::Sel,
        _ => Ctx::Prop,
    }
}

pub(crate) fn css(em: &mut Em, line: &str, state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let mut depth = u32::from(param(state));
    let mut i = 0;
    if mode(state) == 1 {
        match find(b, 0, b"*/") {
            Some(p) => {
                em.push(0, p + 2, K::Comment);
                i = p + 2;
            }
            None => {
                em.push(0, n, K::Comment);
                return state;
            }
        }
    }
    let mut ctx = ctx_at(b, i, depth);
    while let Some(&c) = b.get(i) {
        match c {
            b' ' | b'\t' | b'\r' => i += 1,
            b'/' if b.get(i + 1) == Some(&b'*') => match find(b, i + 2, b"*/") {
                Some(p) => {
                    em.push(i, p + 2, K::Comment);
                    i = p + 2;
                }
                None => {
                    em.push(i, n, K::Comment);
                    return st(1, depth.min(255) as u8, 0);
                }
            },
            b'/' if b.get(i + 1) == Some(&b'/') && (i == 0 || b.get(i - 1).is_some_and(u8::is_ascii_whitespace)) => {
                em.push(i, n, K::Comment);
                i = n;
            }
            b'"' | b'\'' => i = string_body(em, b, i, i + 1, c, true).0,
            b'@' => {
                let mut e = i + 1;
                while b.get(e).is_some_and(|&d| is_ident_char(d) || d == b'-') {
                    e += 1;
                }
                em.push(i, e, K::Attribute);
                i = e;
            }
            b'{' | b'}' | b';' => {
                depth = match c {
                    b'{' => depth.saturating_add(1),
                    b'}' => depth.saturating_sub(1),
                    _ => depth,
                };
                em.push(i, i + 1, K::Punct);
                i += 1;
                ctx = ctx_at(b, i, depth);
            }
            b':' if ctx == Ctx::Prop => {
                em.push(i, i + 1, K::Punct);
                i += 1;
                ctx = Ctx::Value;
            }
            b':' if ctx == Ctx::Sel => {
                let mut e = i + 1;
                if b.get(e) == Some(&b':') {
                    e += 1;
                }
                while b.get(e).is_some_and(|&d| is_ident_char(d) || d == b'-') {
                    e += 1;
                }
                em.push(i, e, if e > i + 1 { K::Keyword } else { K::Punct });
                i = e;
            }
            b'#' | b'.' if b.get(i + 1).is_some_and(|&d| is_ident_start(d) || d == b'-') && !(c == b'.' && ctx == Ctx::Value) => {
                let mut e = i + 1;
                while b.get(e).is_some_and(|&d| is_ident_char(d) || d == b'-') {
                    e += 1;
                }
                em.push(i, e, if ctx == Ctx::Value { K::Number } else { K::Type });
                i = e;
            }
            b'.' if b.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                let e = number_end(b, i, false);
                em.push(i, e, K::Number);
                i = e;
            }
            b'!' => {
                let e = (i + 1..n).find(|&k| !b.get(k).is_some_and(u8::is_ascii_alphabetic)).unwrap_or(n);
                em.push(i, e, K::Keyword);
                i = e;
            }
            b'0'..=b'9' => {
                let mut e = number_end(b, i, false);
                if b.get(e) == Some(&b'%') {
                    e += 1;
                }
                em.push(i, e, K::Number);
                i = e;
            }
            b'-' if b.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == b'.') => {
                let e = number_end(b, i + 1, false).max(i + 1);
                em.push(i, e, K::Number);
                i = e;
            }
            _ if is_ident_start(c) || c == b'-' => {
                let mut e = i;
                while b.get(e).is_some_and(|&d| is_ident_char(d) || d == b'-') {
                    e += 1;
                }
                let e = e.max(i + 1);
                let after = skip_ws(b, e);
                let next = b.get(after).copied();
                let w = b.get(i..e).unwrap_or(b"");
                if w.eq_ignore_ascii_case(b"url") && next == Some(b'(') {
                    em.push(i, e, K::Function);
                    let close = b.get(after..).and_then(|r| r.iter().position(|&d| d == b')')).map_or(n, |p| after + p);
                    em.push(after, after + 1, K::Punct);
                    em.push(after + 1, close, K::String);
                    i = close;
                    continue;
                }
                let kind = match ctx {
                    _ if next == Some(b'(') => Some(K::Function),
                    Ctx::Prop => Some(K::Key),
                    Ctx::Sel => Some(K::Tag),
                    Ctx::Value => None,
                };
                if let Some(k) = kind {
                    em.push(i, e, k);
                }
                i = e;
            }
            b',' | b'(' | b')' | b'>' | b'+' | b'~' | b'*' | b'[' | b']' | b'=' | b'&' | b':' => {
                em.push(i, i + 1, K::Punct);
                i += 1;
            }
            _ => i = next_char(b, i),
        }
    }
    st(0, depth.min(255) as u8, 0)
}
