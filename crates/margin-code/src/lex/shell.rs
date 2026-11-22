//! Shell (sh/bash/zsh, also Dockerfile) and Makefile.
//!
//! | Lang | Multi-line state | Notes |
//! |---|---|---|
//! | Shell | 1 `"…"`, 2 `'…'`, 3 heredoc (param len, extra delim) | command word → `Function`, `NAME=` → `Key`, `$x` `${x}` → `Constant` |
//! | Makefile | none | targets → `Function`, `VAR =` → `Key`, `$(…)` → `Constant`, tab recipes lexed as Shell |

use super::{
    delim_matches, extra, in_table, is_ident_char, is_ident_start, mode, next_char, pack_delim, param, skip_ws, st,
    string_body, word_end, Em, LineState, TokenKind as K,
};

const SH_KW: &[&str] = &[
    "case", "coproc", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in", "select", "then",
    "time", "until", "while",
];
/// Keywords after which a command word follows.
const SH_CMD_AFTER: &[&str] = &["do", "elif", "else", "if", "then", "time", "until", "while"];

const MK_DIRECTIVES: &[&str] = &[
    "-include", "define", "else", "endef", "endif", "export", "ifdef", "ifeq", "ifndef", "ifneq", "include",
    "override", "sinclude", "unexport", "vpath",
];

#[cfg(test)]
pub(crate) fn tables() -> Vec<(&'static str, &'static [&'static str])> {
    vec![("SH_KW", SH_KW), ("SH_CMD_AFTER", SH_CMD_AFTER), ("MK_DIRECTIVES", MK_DIRECTIVES)]
}

fn is_word_char(c: u8) -> bool {
    is_ident_char(c) || matches!(c, b'-' | b'.' | b'/' | b'+' | b'@' | b'%' | b':' | b'~' | b',' | b'^')
}

/// `${…}` / `$(…)` / `$((…))` balanced on this line: end position, else end of line.
fn balanced_end(b: &[u8], i: usize, open: u8, close: u8) -> usize {
    let mut depth = 0u32;
    let mut j = i;
    while let Some(&c) = b.get(j) {
        if c == open {
            depth += 1;
        } else if c == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return j + 1;
            }
        }
        j += 1;
    }
    b.len()
}

/// `$` expansion at `i` (not `$(`): end position.
fn dollar_end(b: &[u8], i: usize) -> usize {
    match b.get(i + 1) {
        Some(b'{') => balanced_end(b, i + 1, b'{', b'}'),
        Some(&c) if is_ident_start(c) && c < 0x80 => word_end(b, i + 1, false),
        Some(c) if c.is_ascii_digit() || b"@?#$!*-".contains(c) => i + 2,
        _ => i + 1,
    }
}

/// `"…"` body from `i`: `\` escapes and `$x` expansions inside.
fn dq_body(em: &mut Em, b: &[u8], seg: usize, i: usize) -> (usize, bool) {
    let mut seg = seg;
    let mut j = i;
    while let Some(&c) = b.get(j) {
        match c {
            b'"' => {
                em.push(seg, j + 1, K::String);
                return (j + 1, true);
            }
            b'\\' => {
                em.push(seg, j, K::String);
                let e = next_char(b, j + 1);
                em.push(j, e, K::Escape);
                j = e;
                seg = e;
            }
            b'$' if b.get(j + 1) != Some(&b'(') => {
                let e = dollar_end(b, j);
                if e > j + 1 {
                    em.push(seg, j, K::String);
                    em.push(j, e, K::Constant);
                    seg = e;
                }
                j = e;
            }
            _ => j += 1,
        }
    }
    em.push(seg, b.len(), K::String);
    (b.len(), false)
}

pub(crate) fn shell(em: &mut Em, line: &str, state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0;
    match mode(state) {
        1 => {
            let (e, closed) = dq_body(em, b, 0, 0);
            if !closed {
                return state;
            }
            i = e;
        }
        2 => match b.iter().position(|&c| c == b'\'') {
            Some(p) => {
                em.push(0, p + 1, K::String);
                i = p + 1;
            }
            None => {
                em.push(0, n, K::String);
                return state;
            }
        },
        3 => {
            em.push(0, n, K::String);
            let t = line.trim().as_bytes();
            return if delim_matches(t, usize::from(param(state)), extra(state)) { LineState::default() } else { state };
        }
        _ => {}
    }
    let mut cmd = i == 0;
    let mut after_function = false;
    let mut heredoc: Option<LineState> = None;
    while let Some(&c) = b.get(i) {
        match c {
            b' ' | b'\t' | b'\r' | 0x0c => i += 1,
            b'#' if i == 0 || b.get(i - 1).is_some_and(|p| p.is_ascii_whitespace() || *p == b';') => {
                em.push(i, n, K::Comment);
                i = n;
            }
            b'\\' => {
                let e = next_char(b, i + 1);
                em.push(i, e, K::Escape);
                i = e;
            }
            b'"' => {
                let (e, closed) = dq_body(em, b, i, i + 1);
                if !closed {
                    return st(1, 0, 0);
                }
                i = e;
                cmd = false;
            }
            b'\'' => match b.get(i + 1..).and_then(|r| r.iter().position(|&c| c == b'\'')) {
                Some(p) => {
                    em.push(i, i + p + 2, K::String);
                    i = i + p + 2;
                    cmd = false;
                }
                None => {
                    em.push(i, n, K::String);
                    return st(2, 0, 0);
                }
            },
            b'$' if b.get(i + 1) == Some(&b'\'') => {
                let (e, _) = string_body(em, b, i, i + 2, b'\'', true);
                i = e;
                cmd = false;
            }
            b'$' if b.get(i + 1) == Some(&b'(') => {
                let arith = b.get(i + 2) == Some(&b'(');
                let e = if arith { i + 3 } else { i + 2 };
                em.push(i, e, K::Punct);
                i = e;
                cmd = !arith;
            }
            b'$' => {
                let e = dollar_end(b, i);
                em.push(i, e, K::Constant);
                i = e;
                cmd = false;
            }
            b'`' => {
                em.push(i, i + 1, K::Punct);
                i += 1;
                cmd = true;
            }
            b'<' if b.get(i + 1) == Some(&b'<') && b.get(i + 2) != Some(&b'<') => {
                let mut j = i + 2;
                if b.get(j) == Some(&b'-') {
                    j += 1;
                }
                j = skip_ws(b, j);
                let quoted = matches!(b.get(j), Some(b'\'' | b'"'));
                let ds = if quoted { j + 1 } else { j };
                let de = word_end(b, ds, false);
                if de > ds {
                    let d = b.get(ds..de).unwrap_or(b"");
                    heredoc = Some(st(3, d.len().min(255) as u8, pack_delim(d)));
                    let e = if quoted { (de + 1).min(n) } else { de };
                    em.push(i, e, K::String);
                    i = e;
                } else {
                    em.push(i, i + 2, K::Operator);
                    i += 2;
                }
            }
            b';' | b'|' | b'&' => {
                // `>&2`, `&>file`: redirections, not command separators.
                let redirect = c == b'&' && (matches!(b.get(i.wrapping_sub(1)), Some(b'>' | b'<')) || b.get(i + 1) == Some(&b'>'));
                let mut e = i + 1;
                while !redirect && b.get(e).is_some_and(|&d| matches!(d, b';' | b'|' | b'&')) {
                    e += 1;
                }
                em.push(i, e, K::Operator);
                i = e;
                cmd = !redirect;
            }
            b'(' | b'{' => {
                em.push(i, i + 1, K::Punct);
                i += 1;
                cmd = true;
            }
            b')' | b'}' | b'[' | b']' => {
                em.push(i, i + 1, K::Punct);
                i += 1;
            }
            b'<' | b'>' | b'!' | b'=' => {
                em.push(i, i + 1, K::Operator);
                i += 1;
            }
            _ if is_word_char(c) => {
                // An assignment `NAME=` first.
                let ie = word_end(b, i, false);
                if cmd && ie > i && b.get(ie) == Some(&b'=') && is_ident_start(c) {
                    em.push(i, ie, K::Key);
                    em.push(ie, ie + 1, K::Operator);
                    i = ie + 1;
                    let mut e = i;
                    while b.get(e).is_some_and(|&d| is_word_char(d) && d != b'=') {
                        e += 1;
                    }
                    i = e;
                    continue;
                }
                let mut e = i;
                while b.get(e).is_some_and(|&d| is_word_char(d) || d == b'=') {
                    e += 1;
                }
                let ws = line.get(i..e).unwrap_or("");
                if (cmd || ws == "in") && in_table(SH_KW, ws) {
                    em.push(i, e, K::Keyword);
                    cmd = in_table(SH_CMD_AFTER, ws);
                    after_function = ws == "function";
                } else if after_function || (cmd && !ws.starts_with('-')) {
                    em.push(i, e, K::Function);
                    cmd = false;
                    after_function = false;
                } else if b.get(i..e).is_some_and(|w| w.iter().all(u8::is_ascii_digit)) {
                    em.push(i, e, K::Number);
                } else if b.get(e) == Some(&b'(') && b.get(e + 1) == Some(&b')') {
                    em.push(i, e, K::Function);
                }
                i = e;
            }
            _ => i = next_char(b, i),
        }
    }
    heredoc.unwrap_or_default()
}

pub(crate) fn makefile(em: &mut Em, line: &str, _state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let recipe = b.first() == Some(&b'\t');
    let mut i = skip_ws(b, 0);
    // Directive or assignment or target at line start.
    let we = {
        let mut e = i;
        while b.get(e).is_some_and(|&d| is_ident_char(d) || matches!(d, b'-' | b'.' | b'/' | b'%' | b'$' | b'(' | b')')) {
            e += 1;
        }
        e
    };
    let ws = line.get(i..we).unwrap_or("");
    if recipe {
        // Recipe: shell text; only make variables and comments are marked.
    } else if in_table(MK_DIRECTIVES, ws) {
        em.push(i, we, K::Keyword);
        i = we;
    } else if we > i {
        let after = skip_ws(b, we);
        let rest = b.get(after..).unwrap_or(b"");
        let assign = [&b"="[..], b":=", b"::=", b"?=", b"+=", b"!="].iter().any(|op| rest.starts_with(op));
        if assign {
            em.push(i, we, K::Key);
            i = we;
        } else if rest.first() == Some(&b':') {
            em.push(i, we, K::Function);
            i = we;
        }
    }
    while let Some(&c) = b.get(i) {
        match c {
            b'#' => {
                em.push(i, n, K::Comment);
                i = n;
            }
            b'\\' => i = next_char(b, i + 1),
            b'$' => {
                let e = match b.get(i + 1) {
                    Some(b'(') => balanced_end(b, i + 1, b'(', b')'),
                    Some(b'{') => balanced_end(b, i + 1, b'{', b'}'),
                    Some(_) => next_char(b, i + 1),
                    None => n,
                };
                em.push(i, e, K::Constant);
                i = e;
            }
            b'=' | b':' | b'?' | b'+' | b'!' | b'|' | b';' => {
                em.push(i, i + 1, K::Operator);
                i += 1;
            }
            b'"' | b'\'' => i = string_body(em, b, i, i + 1, c, false).0,
            _ => i = next_char(b, i),
        }
    }
    LineState::default()
}
