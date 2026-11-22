//! Python, Ruby, Lua.
//!
//! | Lang | Multi-line state | Notes |
//! |---|---|---|
//! | Python | 1 triple string (param quote, extra bit 0 raw) | prefixes `r b u f t` any mix; `@decorator` at line start |
//! | Ruby | 1 `=begin`, 2 string (param close char), 3 heredoc (param len, extra delim) | `:sym`, `%w[]`, `/re/`, `<<~EOS` |
//! | Lua | 1 long comment, 2 long string (param `=` level) | `--[==[ ]==]`, `[[ ]]` |

use super::{
    delim_matches, escape_end, in_table, is_all_caps, is_ident_char, is_ident_start, mode, next_char, number_end,
    pack_delim, param, extra, skip_ws, st, string_body, word_end, Em, LineState, TokenKind as K,
};

const PY_KW: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except",
    "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass",
    "raise", "return", "try", "while", "with", "yield",
];
const PY_TYPES: &[&str] = &[
    "bool", "bytearray", "bytes", "complex", "dict", "float", "frozenset", "int", "list", "object", "set", "str",
    "tuple", "type",
];
const PY_CONSTS: &[&str] = &["False", "None", "NotImplemented", "True"];

const RB_KW: &[&str] = &[
    "BEGIN", "END", "__FILE__", "__LINE__", "__method__", "alias", "and", "begin", "break", "case", "class", "def",
    "defined?", "do", "else", "elsif", "end", "ensure", "for", "if", "in", "module", "next", "not", "or", "redo",
    "rescue", "retry", "return", "self", "super", "then", "undef", "unless", "until", "when", "while", "yield",
];
const NIL_CONSTS: &[&str] = &["false", "nil", "true"];

const LUA_KW: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "for", "function", "goto", "if", "in", "local", "not", "or",
    "repeat", "return", "then", "until", "while",
];

#[cfg(test)]
pub(crate) fn tables() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        ("PY_KW", PY_KW),
        ("PY_TYPES", PY_TYPES),
        ("PY_CONSTS", PY_CONSTS),
        ("RB_KW", RB_KW),
        ("NIL_CONSTS", NIL_CONSTS),
        ("LUA_KW", LUA_KW),
    ]
}

fn is_op(c: u8) -> bool {
    matches!(c, b'+' | b'-' | b'*' | b'/' | b'%' | b'=' | b'<' | b'>' | b'!' | b'&' | b'|' | b'^' | b'~' | b'?' | b':' | b'@')
}

fn op_run(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while b.get(j).is_some_and(|&c| is_op(c) && c != b'#') {
        j += 1;
    }
    j
}

fn is_punct(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'[' | b']' | b'{' | b'}' | b',' | b'.' | b';')
}

// ---------------- Python ----------------

/// Python string body; raw strings still skip `\x` pairs but emit no `Escape`.
fn py_str(em: &mut Em, b: &[u8], seg: usize, i: usize, q: u8, raw: bool, triple: bool) -> (usize, bool) {
    let mut seg = seg;
    let mut j = i;
    while let Some(&c) = b.get(j) {
        if c == q && (!triple || (b.get(j + 1) == Some(&q) && b.get(j + 2) == Some(&q))) {
            let e = if triple { j + 3 } else { j + 1 };
            em.push(seg, e, K::String);
            return (e, true);
        }
        if c == b'\\' {
            let e = escape_end(b, j);
            if !raw {
                em.push(seg, j, K::String);
                em.push(j, e, K::Escape);
                seg = e;
            }
            j = e;
            continue;
        }
        j += 1;
    }
    em.push(seg, b.len(), K::String);
    (b.len(), false)
}

pub(crate) fn python(em: &mut Em, line: &str, state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0;
    if mode(state) == 1 {
        let q = if param(state) == b'\'' { b'\'' } else { b'"' };
        let raw = extra(state) & 1 == 1;
        let (e, closed) = py_str(em, b, 0, 0, q, raw, true);
        if !closed {
            return st(1, q, u64::from(raw));
        }
        i = e;
    }
    let first = skip_ws(b, 0);
    let mut after_class = false;
    while let Some(&c) = b.get(i) {
        match c {
            b' ' | b'\t' | b'\r' | 0x0c => i += 1,
            b'#' => {
                em.push(i, n, K::Comment);
                i = n;
            }
            b'@' if i == first => {
                let mut e = i + 1;
                loop {
                    e = word_end(b, e, false);
                    if b.get(e) == Some(&b'.') && b.get(e + 1).is_some_and(|&d| is_ident_start(d)) {
                        e += 1;
                    } else {
                        break;
                    }
                }
                em.push(i, e, K::Attribute);
                i = e;
            }
            b'"' | b'\'' => match py_string(em, b, i, i, c, false) {
                Ok(e) => i = e,
                Err(s) => return s,
            },
            b'0'..=b'9' => {
                let e = number_end(b, i, false);
                em.push(i, e, K::Number);
                i = e;
            }
            b'.' if b.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                let e = number_end(b, i, false);
                em.push(i, e, K::Number);
                i = e;
            }
            _ if is_ident_start(c) => {
                let we = word_end(b, i, false);
                let w = b.get(i..we).unwrap_or(b"");
                if w.len() <= 2 && w.iter().all(|c| b"rRbBuUfFtT".contains(c)) {
                    if let Some(&q) = b.get(we).filter(|&&q| q == b'"' || q == b'\'') {
                        let raw = w.iter().any(|c| *c == b'r' || *c == b'R');
                        match py_string(em, b, i, we, q, raw) {
                            Ok(e) => i = e,
                            Err(s) => return s,
                        }
                        continue;
                    }
                }
                let ws = line.get(i..we).unwrap_or("");
                let kind = if in_table(PY_KW, ws) {
                    Some(K::Keyword)
                } else if in_table(PY_CONSTS, ws) {
                    Some(K::Constant)
                } else if in_table(PY_TYPES, ws) {
                    Some(K::Type)
                } else if b.get(skip_ws(b, we)) == Some(&b'(') {
                    Some(K::Function)
                } else if after_class {
                    Some(K::Type)
                } else if is_all_caps(w) {
                    Some(K::Constant)
                } else {
                    None
                };
                if let Some(k) = kind {
                    em.push(i, we, k);
                }
                after_class = ws == "class";
                i = we;
            }
            _ if is_punct(c) => {
                em.push(i, i + 1, K::Punct);
                i += 1;
            }
            _ if is_op(c) => {
                let e = op_run(b, i);
                em.push(i, e, K::Operator);
                i = e;
            }
            _ => i = next_char(b, i),
        }
    }
    LineState::default()
}

/// String at `seg` (prefix start) with the quote at `q_at`.
fn py_string(em: &mut Em, b: &[u8], seg: usize, q_at: usize, q: u8, raw: bool) -> Result<usize, LineState> {
    if b.get(q_at + 1) == Some(&q) && b.get(q_at + 2) == Some(&q) {
        let (e, closed) = py_str(em, b, seg, q_at + 3, q, raw, true);
        if closed {
            Ok(e)
        } else {
            Err(st(1, q, u64::from(raw)))
        }
    } else {
        Ok(py_str(em, b, seg, q_at + 1, q, raw, false).0)
    }
}

// ---------------- Ruby ----------------

const RB_SQ: u8 = 2;
const RB_HEREDOC: u8 = 3;

fn closing_of(c: u8) -> u8 {
    match c {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        b'<' => b'>',
        _ => c,
    }
}

pub(crate) fn ruby(em: &mut Em, line: &str, state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0;
    match mode(state) {
        1 => {
            em.push(0, n, K::Comment);
            return if b.starts_with(b"=end") { LineState::default() } else { state };
        }
        RB_SQ => {
            let q = param(state);
            if q == 0 {
                return LineState::default();
            }
            let (e, closed) = string_body(em, b, 0, 0, q, true);
            if !closed {
                return state;
            }
            i = e;
        }
        RB_HEREDOC => {
            let t = line.trim().as_bytes();
            em.push(0, n, K::String);
            return if delim_matches(t, usize::from(param(state)), extra(state)) { LineState::default() } else { state };
        }
        _ => {
            if b.starts_with(b"=begin") {
                em.push(0, n, K::Comment);
                return st(1, 0, 0);
            }
        }
    }
    let mut value_pos = true; // a value may start here: `/re/`, `%w[]`, heredoc
    let mut regex_dead = false;
    let mut heredoc: Option<LineState> = None;
    let mut after_def = false;
    while let Some(&c) = b.get(i) {
        match c {
            b' ' | b'\t' | b'\r' | 0x0c => i += 1,
            b'#' => {
                em.push(i, n, K::Comment);
                i = n;
            }
            b'"' | b'`' | b'\'' => {
                let (e, closed) = string_body(em, b, i, i + 1, c, true);
                if !closed {
                    return st(RB_SQ, c, 0);
                }
                i = e;
                value_pos = false;
            }
            b':' if b.get(i + 1).is_some_and(|&d| is_ident_start(d)) && value_pos_or_space(b, i) => {
                let mut e = word_end(b, i + 1, false);
                if matches!(b.get(e), Some(b'?' | b'!' | b'=')) && b.get(e + 1) != Some(&b'=') {
                    e += 1;
                }
                em.push(i, e, K::Constant);
                i = e;
                value_pos = false;
            }
            b'@' | b'$' => {
                let mut e = i + 1;
                if b.get(e) == Some(&b'@') {
                    e += 1;
                }
                let we = word_end(b, e, false);
                i = if we == e { next_char(b, e.saturating_sub(1)).max(e) } else { we };
                value_pos = false;
            }
            b'%' if value_pos && percent_open(b, i).is_some() => {
                let (open_at, close) = percent_open(b, i).unwrap_or((i, b')'));
                match b.get(open_at + 1..).and_then(|r| r.iter().position(|&d| d == close)) {
                    Some(p) => {
                        em.push(i, open_at + p + 2, K::String);
                        i = open_at + p + 2;
                    }
                    None => {
                        em.push(i, n, K::String);
                        return st(RB_SQ, close, 0);
                    }
                }
                value_pos = false;
            }
            b'/' if value_pos && !regex_dead => match regex_end(b, i) {
                Some(e) => {
                    em.push(i, e, K::String);
                    i = e;
                    value_pos = false;
                }
                None => regex_dead = true,
            },
            b'<' if b.get(i + 1) == Some(&b'<') && heredoc_start(b, i).is_some() => {
                let (e, d) = heredoc_start(b, i).unwrap_or((i + 2, &[]));
                em.push(i, e, K::String);
                heredoc = Some(st(RB_HEREDOC, d.len().min(255) as u8, pack_delim(d)));
                i = e;
                value_pos = false;
            }
            b'0'..=b'9' => {
                let e = number_end(b, i, false);
                em.push(i, e, K::Number);
                i = e;
                value_pos = false;
            }
            _ if is_ident_start(c) => {
                let mut we = word_end(b, i, false);
                if matches!(b.get(we), Some(b'?' | b'!')) && !matches!(b.get(we + 1), Some(b'=')) {
                    we += 1;
                }
                let w = b.get(i..we).unwrap_or(b"");
                let ws = line.get(i..we).unwrap_or("");
                let dotted = i > 0 && b.get(i - 1) == Some(&b'.');
                let kind = if !dotted && in_table(RB_KW, ws) {
                    Some(K::Keyword)
                } else if in_table(NIL_CONSTS, ws) {
                    Some(K::Constant)
                } else if after_def || b.get(we) == Some(&b'(') {
                    Some(K::Function)
                } else if is_all_caps(w) {
                    Some(K::Constant)
                } else if w.first().is_some_and(u8::is_ascii_uppercase) {
                    Some(K::Type)
                } else {
                    None
                };
                if let Some(k) = kind {
                    em.push(i, we, k);
                }
                after_def = ws == "def";
                // After a keyword (`if`, `return`…) or a bare method name with a space, a value may follow.
                value_pos = kind == Some(K::Keyword) || (kind.is_none() && b.get(we) == Some(&b' '));
                i = we;
            }
            _ if is_punct(c) => {
                em.push(i, i + 1, K::Punct);
                i += 1;
                value_pos = !matches!(c, b')' | b']' | b'}');
            }
            _ if is_op(c) => {
                let e = op_run(b, i);
                em.push(i, e, K::Operator);
                i = e;
                value_pos = true;
            }
            _ => i = next_char(b, i),
        }
    }
    heredoc.unwrap_or_default()
}

/// `:sym` is a symbol unless it is the `:` of `a ? b : c` written tight, or `::`.
fn value_pos_or_space(b: &[u8], i: usize) -> bool {
    i == 0 || !matches!(b.get(i - 1), Some(b':')) && !b.get(i - 1).is_some_and(|&p| is_ident_char(p) || p == b')')
}

/// `%w[`, `%q(`, `%(`: (position of the opening delimiter, closing delimiter).
fn percent_open(b: &[u8], i: usize) -> Option<(usize, u8)> {
    let mut j = i + 1;
    if matches!(b.get(j), Some(b'q' | b'Q' | b'w' | b'W' | b'i' | b'I' | b'r' | b's' | b'x')) {
        j += 1;
    }
    match b.get(j) {
        Some(&d @ (b'(' | b'[' | b'{' | b'<' | b'|' | b'!' | b'/')) => Some((j, closing_of(d))),
        _ => None,
    }
}

/// `<<~ID`, `<<-ID`, `<<ID` (uppercase), `<<~'ID'`: (end of the opener, delimiter).
fn heredoc_start(b: &[u8], i: usize) -> Option<(usize, &[u8])> {
    let mut j = i + 2;
    let flag = matches!(b.get(j), Some(b'~' | b'-'));
    if flag {
        j += 1;
    }
    let quote = matches!(b.get(j), Some(b'\'' | b'"' | b'`'));
    if quote {
        j += 1;
    }
    let ds = j;
    let de = word_end(b, ds, false);
    if de == ds {
        return None;
    }
    if !flag && !quote && !b.get(ds).is_some_and(u8::is_ascii_uppercase) {
        return None;
    }
    let d = b.get(ds..de)?;
    let end = if quote { (de + 1).min(b.len()) } else { de };
    Some((end, d))
}

fn regex_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if matches!(b.get(j), Some(b' ' | b'=') | None) {
        return None;
    }
    while let Some(&c) = b.get(j) {
        match c {
            b'\\' => j = next_char(b, j + 1),
            b'/' => {
                let mut e = j + 1;
                while b.get(e).is_some_and(u8::is_ascii_alphabetic) {
                    e += 1;
                }
                return Some(e);
            }
            _ => j += 1,
        }
    }
    None
}

// ---------------- Lua ----------------

/// `[`, `=`*, `[` at `i`: the level (number of `=`), if it is a long bracket.
fn long_open(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'[') {
        return None;
    }
    let mut j = i + 1;
    while b.get(j) == Some(&b'=') {
        j += 1;
    }
    (b.get(j) == Some(&b'[')).then_some(j - i - 1)
}

/// End of a long bracket body (after `]`, `=`*level, `]`).
fn long_close(b: &[u8], i: usize, level: usize) -> Option<usize> {
    let mut j = i;
    while let Some(p) = b.get(j..).and_then(|r| r.iter().position(|&c| c == b']')) {
        let q = j + p;
        let eqs = b.get(q + 1..q + 1 + level);
        if eqs.is_some_and(|t| t.iter().all(|&c| c == b'=')) && b.get(q + 1 + level) == Some(&b']') {
            return Some(q + 2 + level);
        }
        j = q + 1;
    }
    None
}

pub(crate) fn lua(em: &mut Em, line: &str, state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0;
    if matches!(mode(state), 1 | 2) {
        let kind = if mode(state) == 1 { K::Comment } else { K::String };
        match long_close(b, 0, usize::from(param(state))) {
            Some(e) => {
                em.push(0, e, kind);
                i = e;
            }
            None => {
                em.push(0, n, kind);
                return state;
            }
        }
    }
    let mut after_function = false;
    while let Some(&c) = b.get(i) {
        match c {
            b' ' | b'\t' | b'\r' | 0x0c => i += 1,
            b'-' if b.get(i + 1) == Some(&b'-') => {
                if let Some(level) = long_open(b, i + 2) {
                    let body = i + 2 + level + 2;
                    match long_close(b, body, level) {
                        Some(e) => {
                            em.push(i, e, K::Comment);
                            i = e;
                        }
                        None => {
                            em.push(i, n, K::Comment);
                            return st(1, level.min(255) as u8, 0);
                        }
                    }
                } else {
                    em.push(i, n, K::Comment);
                    i = n;
                }
            }
            b'[' if long_open(b, i).is_some() => {
                let level = long_open(b, i).unwrap_or(0);
                match long_close(b, i + level + 2, level) {
                    Some(e) => {
                        em.push(i, e, K::String);
                        i = e;
                    }
                    None => {
                        em.push(i, n, K::String);
                        return st(2, level.min(255) as u8, 0);
                    }
                }
            }
            b'"' | b'\'' => i = string_body(em, b, i, i + 1, c, true).0,
            b'0'..=b'9' => {
                let e = number_end(b, i, false);
                em.push(i, e, K::Number);
                i = e;
            }
            _ if is_ident_start(c) => {
                let we = word_end(b, i, false);
                let w = b.get(i..we).unwrap_or(b"");
                let ws = line.get(i..we).unwrap_or("");
                let kind = if in_table(LUA_KW, ws) {
                    Some(K::Keyword)
                } else if in_table(NIL_CONSTS, ws) {
                    Some(K::Constant)
                } else if after_function || b.get(skip_ws(b, we)) == Some(&b'(') {
                    Some(K::Function)
                } else if is_all_caps(w) {
                    Some(K::Constant)
                } else {
                    None
                };
                if let Some(k) = kind {
                    em.push(i, we, k);
                }
                if ws == "function" {
                    after_function = true;
                } else if b.get(we) != Some(&b'.') && b.get(we) != Some(&b':') {
                    after_function = false;
                }
                i = we;
            }
            _ if is_punct(c) => {
                em.push(i, i + 1, K::Punct);
                i += 1;
            }
            _ if is_op(c) || c == b'#' => {
                let mut e = i + 1;
                while b.get(e).is_some_and(|&d| (is_op(d) || d == b'#') && d != b'-') {
                    e += 1;
                }
                em.push(i, e, K::Operator);
                i = e;
            }
            _ => i = next_char(b, i),
        }
    }
    LineState::default()
}
