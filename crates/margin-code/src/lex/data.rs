//! JSON, TOML, YAML, SQL, Diff.
//!
//! | Lang | Multi-line state | Notes |
//! |---|---|---|
//! | JSON | 1 `/* */` (JSONC) | `"k":` → `Key`; `//` comments accepted |
//! | TOML | 1 `"""`, 2 `'''` | `[table]` → `Heading`, `k =` → `Key` |
//! | YAML | 1 block scalar (param = indent of the `key: \|` line + 1) | `k:` → `Key`, `- ` → `Marker`, `&a *a !tag` → `Attribute` |
//! | SQL | 1 `/* */`, 2 `'…'` | keywords case-insensitive |
//! | Diff | none | whole-line `Inserted` / `Deleted` / `Heading` (`+++ ---`) / `Attribute` (`@@`) |

use super::{
    in_table, is_ident_char, is_ident_start, lower, mode, next_char, number_end, param, skip_ws, st, string_body,
    word_end, Em, LineState, TokenKind as K,
};

const SQL_KW: &[&str] = &[
    "add", "all", "alter", "and", "any", "as", "asc", "begin", "between", "by", "cascade", "case", "check",
    "column", "commit", "constraint", "create", "cross", "database", "declare", "default", "delete", "desc",
    "distinct", "drop", "else", "end", "escape", "except", "exec", "execute", "exists", "explain", "fetch",
    "foreign", "from", "full", "function", "grant", "group", "having", "if", "in", "index", "inner", "insert",
    "intersect", "into", "is", "join", "key", "left", "like", "limit", "not", "of", "offset", "on", "or", "order",
    "outer", "over", "partition", "primary", "procedure", "references", "replace", "return", "returning", "returns",
    "revoke", "right", "rollback", "schema", "select", "set", "table", "then", "to", "transaction", "trigger",
    "truncate", "union", "unique", "update", "using", "values", "view", "when", "where", "while", "with",
];
const SQL_TYPES: &[&str] = &[
    "bigint", "binary", "bit", "blob", "boolean", "char", "date", "datetime", "decimal", "double", "float", "int",
    "integer", "interval", "json", "jsonb", "numeric", "real", "serial", "smallint", "text", "time", "timestamp",
    "tinyint", "uuid", "varchar",
];
const SQL_CONSTS: &[&str] = &["false", "null", "true"];
const YAML_CONSTS: &[&str] = &["false", "no", "null", "off", "on", "true", "yes", "~"];

#[cfg(test)]
pub(crate) fn tables() -> Vec<(&'static str, &'static [&'static str])> {
    vec![("SQL_KW", SQL_KW), ("SQL_TYPES", SQL_TYPES), ("SQL_CONSTS", SQL_CONSTS), ("YAML_CONSTS", YAML_CONSTS)]
}

/// End of a `"…"` starting at `i` (escapes skipped), and whether it closed.
fn dq_end(b: &[u8], i: usize) -> (usize, bool) {
    let mut j = i + 1;
    while let Some(&c) = b.get(j) {
        match c {
            b'\\' => j = next_char(b, j + 1),
            b'"' => return (j + 1, true),
            _ => j += 1,
        }
    }
    (b.len(), false)
}

fn block_comment(em: &mut Em, b: &[u8], seg: usize, i: usize) -> Option<usize> {
    match b.get(i..).and_then(|r| r.windows(2).position(|w| w == b"*/")) {
        Some(p) => {
            em.push(seg, i + p + 2, K::Comment);
            Some(i + p + 2)
        }
        None => {
            em.push(seg, b.len(), K::Comment);
            None
        }
    }
}

// ---------------- JSON ----------------

pub(crate) fn json(em: &mut Em, line: &str, state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0;
    if mode(state) == 1 {
        match block_comment(em, b, 0, 0) {
            Some(e) => i = e,
            None => return state,
        }
    }
    while let Some(&c) = b.get(i) {
        match c {
            b'"' => {
                let (e, _) = dq_end(b, i);
                let key = b.get(skip_ws(b, e)) == Some(&b':');
                if key {
                    em.push(i, e, K::Key);
                } else {
                    string_body(em, b, i, i + 1, b'"', true);
                }
                i = e;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                em.push(i, n, K::Comment);
                i = n;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => match block_comment(em, b, i, i + 2) {
                Some(e) => i = e,
                None => return st(1, 0, 0),
            },
            b'-' | b'0'..=b'9' => {
                let e = number_end(b, i + 1, false).max(i + 1);
                em.push(i, e, K::Number);
                i = e;
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                em.push(i, i + 1, K::Punct);
                i += 1;
            }
            _ if is_ident_start(c) => {
                let e = word_end(b, i, false);
                if matches!(b.get(i..e), Some(b"true" | b"false" | b"null")) {
                    em.push(i, e, K::Constant);
                }
                i = e;
            }
            _ => i = next_char(b, i),
        }
    }
    LineState::default()
}

// ---------------- TOML ----------------

fn toml_value_end(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while b.get(j).is_some_and(|&c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'.' | b'+' | b'-')) {
        j += 1;
    }
    j.max(i + 1).min(b.len())
}

pub(crate) fn toml(em: &mut Em, line: &str, state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0;
    match mode(state) {
        1 | 2 => {
            let (q, esc) = if mode(state) == 1 { (b'"', true) } else { (b'\'', false) };
            let (e, closed) = triple(em, b, 0, 0, q, esc);
            if !closed {
                return state;
            }
            i = e;
        }
        _ => {
            let f = skip_ws(b, 0);
            if b.get(f) == Some(&b'[') {
                let close = b.iter().rposition(|&c| c == b']').filter(|&p| p > f);
                let comment = b.iter().position(|&c| c == b'#');
                if let Some(p) = close.filter(|&p| comment.is_none_or(|h| h > p)) {
                    em.push(f, p + 1, K::Heading);
                    i = p + 1;
                }
            }
        }
    }
    while let Some(&c) = b.get(i) {
        match c {
            b' ' | b'\t' | b'\r' => i += 1,
            b'#' => {
                em.push(i, n, K::Comment);
                i = n;
            }
            b'"' | b'\'' => {
                if b.get(i + 1) == Some(&c) && b.get(i + 2) == Some(&c) {
                    let (e, closed) = triple(em, b, i, i + 3, c, c == b'"');
                    if !closed {
                        return st(if c == b'"' { 1 } else { 2 }, 0, 0);
                    }
                    i = e;
                } else {
                    let (e, _) = if c == b'"' { dq_end(b, i) } else { sq_end(b, i) };
                    if b.get(skip_ws(b, e)) == Some(&b'=') {
                        em.push(i, e, K::Key);
                    } else {
                        string_body(em, b, i, i + 1, c, c == b'"');
                    }
                    i = e;
                }
            }
            b'[' | b']' | b'{' | b'}' | b',' | b'=' | b'.' => {
                em.push(i, i + 1, if c == b'=' { K::Operator } else { K::Punct });
                i += 1;
            }
            b'0'..=b'9' | b'+' | b'-' => {
                let e = toml_value_end(b, i);
                em.push(i, e, K::Number);
                i = e;
            }
            _ if is_ident_char(c) => {
                let mut e = i;
                while b.get(e).is_some_and(|&d| is_ident_char(d) || d == b'-') {
                    e += 1;
                }
                let after = skip_ws(b, e);
                if matches!(b.get(after), Some(b'=' | b'.')) {
                    em.push(i, e, K::Key);
                } else if matches!(b.get(i..e), Some(b"true" | b"false" | b"inf" | b"nan")) {
                    em.push(i, e, K::Constant);
                }
                i = e;
            }
            _ => i = next_char(b, i),
        }
    }
    LineState::default()
}

fn sq_end(b: &[u8], i: usize) -> (usize, bool) {
    match b.get(i + 1..).and_then(|r| r.iter().position(|&c| c == b'\'')) {
        Some(p) => (i + p + 2, true),
        None => (b.len(), false),
    }
}

fn triple(em: &mut Em, b: &[u8], seg: usize, i: usize, q: u8, escapes: bool) -> (usize, bool) {
    let mut seg = seg;
    let mut j = i;
    while let Some(&c) = b.get(j) {
        if c == q && b.get(j + 1) == Some(&q) && b.get(j + 2) == Some(&q) {
            em.push(seg, j + 3, K::String);
            return (j + 3, true);
        }
        if escapes && c == b'\\' {
            em.push(seg, j, K::String);
            let e = super::escape_end(b, j);
            em.push(j, e, K::Escape);
            j = e;
            seg = e;
            continue;
        }
        j += 1;
    }
    em.push(seg, b.len(), K::String);
    (b.len(), false)
}

// ---------------- YAML ----------------

pub(crate) fn yaml(em: &mut Em, line: &str, state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let indent = b.iter().take_while(|&&c| c == b' ').count();
    if mode(state) == 1 {
        if skip_ws(b, 0) == n {
            return state;
        }
        if indent >= usize::from(param(state)) {
            em.push(indent, n, K::String);
            return state;
        }
    }
    if b.starts_with(b"---") || b.starts_with(b"...") {
        em.push(0, 3, K::Heading);
        if b.get(3).is_some_and(|c| !c.is_ascii_whitespace()) {
            return LineState::default();
        }
    }
    let mut i = indent;
    let mut flow = 0u32;
    let mut next_state = LineState::default();
    while let Some(&c) = b.get(i) {
        match c {
            b' ' | b'\t' | b'\r' => i += 1,
            b'#' if i == 0 || b.get(i - 1).is_some_and(|p| *p == b' ' || *p == b'\t') => {
                em.push(i, n, K::Comment);
                i = n;
            }
            b'-' | b'?' if matches!(b.get(i + 1), Some(b' ' | b'\t') | None) => {
                em.push(i, i + 1, K::Marker);
                i += 1;
            }
            b'-' if b.starts_with(b"---") && i == 0 => i += 3,
            b'{' | b'[' => {
                flow = flow.saturating_add(1);
                em.push(i, i + 1, K::Punct);
                i += 1;
            }
            b'}' | b']' => {
                flow = flow.saturating_sub(1);
                em.push(i, i + 1, K::Punct);
                i += 1;
            }
            b',' | b':' => {
                em.push(i, i + 1, K::Punct);
                i += 1;
            }
            b'&' | b'*' | b'!' => {
                let mut e = i + 1;
                while b.get(e).is_some_and(|&d| !d.is_ascii_whitespace() && !matches!(d, b',' | b']' | b'}')) {
                    e += 1;
                }
                em.push(i, e, K::Attribute);
                i = e;
            }
            b'|' | b'>' => {
                let mut e = i + 1;
                while b.get(e).is_some_and(|&d| matches!(d, b'+' | b'-') || d.is_ascii_digit()) {
                    e += 1;
                }
                let rest = skip_ws(b, e);
                if rest == n || b.get(rest) == Some(&b'#') {
                    em.push(i, e, K::Operator);
                    next_state = st(1, (indent + 1).min(255) as u8, 0);
                }
                i = e;
            }
            b'"' | b'\'' => {
                let (e, _) = if c == b'"' { dq_end(b, i) } else { sq_end(b, i) };
                if b.get(e) == Some(&b':') {
                    em.push(i, e, K::Key);
                } else {
                    string_body(em, b, i, i + 1, c, c == b'"');
                }
                i = e;
            }
            _ => {
                // Plain scalar up to `: `, ` #`, a flow char inside flow, or end of line.
                let mut e = i;
                let mut key = false;
                while let Some(&d) = b.get(e) {
                    if d == b':' && matches!(b.get(e + 1), Some(b' ' | b'\t') | None) {
                        key = true;
                        break;
                    }
                    if d == b'#' && e > i && b.get(e - 1) == Some(&b' ') {
                        break;
                    }
                    if flow > 0 && matches!(d, b',' | b']' | b'}' | b':') {
                        break;
                    }
                    e += 1;
                }
                let e = super::floor_boundary(line, e).max(next_char(b, i).min(n));
                // Trim trailing blanks.
                let mut te = e;
                while te > i && matches!(b.get(te - 1), Some(b' ' | b'\t')) {
                    te -= 1;
                }
                if key {
                    em.push(i, te, K::Key);
                } else if let Some(w) = b.get(i..te) {
                    let mut buf = [0u8; 32];
                    if lower(w, &mut buf).is_some_and(|l| in_table(YAML_CONSTS, l)) {
                        em.push(i, te, K::Constant);
                    } else if w.first().is_some_and(|f| f.is_ascii_digit() || (*f == b'-' && w.len() > 1))
                        && w.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'+' | b'_' | b':'))
                    {
                        em.push(i, te, K::Number);
                    }
                }
                i = e;
            }
        }
    }
    next_state
}

// ---------------- SQL ----------------

pub(crate) fn sql(em: &mut Em, line: &str, state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0;
    match mode(state) {
        1 => match block_comment(em, b, 0, 0) {
            Some(e) => i = e,
            None => return state,
        },
        2 => match sql_str(em, b, 0, 0) {
            Some(e) => i = e,
            None => return state,
        },
        _ => {}
    }
    while let Some(&c) = b.get(i) {
        match c {
            b' ' | b'\t' | b'\r' => i += 1,
            b'-' if b.get(i + 1) == Some(&b'-') => {
                em.push(i, n, K::Comment);
                i = n;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => match block_comment(em, b, i, i + 2) {
                Some(e) => i = e,
                None => return st(1, 0, 0),
            },
            b'\'' => match sql_str(em, b, i, i + 1) {
                Some(e) => i = e,
                None => return st(2, 0, 0),
            },
            b'"' | b'`' => {
                i = b.get(i + 1..).and_then(|r| r.iter().position(|&d| d == c)).map_or(n, |p| i + p + 2);
            }
            b'0'..=b'9' => {
                let e = number_end(b, i, false);
                em.push(i, e, K::Number);
                i = e;
            }
            b'@' | b':' | b'$' if b.get(i + 1).is_some_and(|&d| is_ident_char(d)) => {
                let e = word_end(b, i + 1, false);
                em.push(i, e, K::Constant);
                i = e;
            }
            _ if is_ident_start(c) => {
                let e = word_end(b, i, false);
                let w = b.get(i..e).unwrap_or(b"");
                let mut buf = [0u8; 32];
                let l = lower(w, &mut buf);
                let kind = match l {
                    Some(l) if in_table(SQL_KW, l) => Some(K::Keyword),
                    Some(l) if in_table(SQL_CONSTS, l) => Some(K::Constant),
                    Some(l) if in_table(SQL_TYPES, l) => Some(K::Type),
                    _ if b.get(skip_ws(b, e)) == Some(&b'(') => Some(K::Function),
                    _ => None,
                };
                if let Some(k) = kind {
                    em.push(i, e, k);
                }
                i = e;
            }
            b'(' | b')' | b',' | b';' | b'.' => {
                em.push(i, i + 1, K::Punct);
                i += 1;
            }
            b'=' | b'<' | b'>' | b'!' | b'+' | b'*' | b'/' | b'%' | b'|' | b'-' => {
                em.push(i, i + 1, K::Operator);
                i += 1;
            }
            _ => i = next_char(b, i),
        }
    }
    LineState::default()
}

/// `'…'` body from `i` with `''` as a quote; `None` if it runs past the line.
fn sql_str(em: &mut Em, b: &[u8], seg: usize, i: usize) -> Option<usize> {
    let mut j = i;
    while let Some(&c) = b.get(j) {
        if c == b'\'' {
            if b.get(j + 1) == Some(&b'\'') {
                j += 2;
                continue;
            }
            em.push(seg, j + 1, K::String);
            return Some(j + 1);
        }
        j += 1;
    }
    em.push(seg, b.len(), K::String);
    None
}

// ---------------- Diff ----------------

pub(crate) fn diff(em: &mut Em, line: &str) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let kind = if b.starts_with(b"+++ ") || b.starts_with(b"--- ") || b == b"+++" || b == b"---" {
        Some(K::Heading)
    } else if b.starts_with(b"@@") {
        Some(K::Attribute)
    } else if [&b"diff "[..], b"index ", b"new file", b"deleted file", b"similarity", b"rename ", b"Binary files", b"old mode", b"new mode"]
        .iter()
        .any(|p| b.starts_with(p))
    {
        Some(K::Keyword)
    } else if b.starts_with(b"+") {
        Some(K::Inserted)
    } else if b.starts_with(b"-") {
        Some(K::Deleted)
    } else if b.starts_with(b"\\") {
        Some(K::Comment)
    } else {
        None
    };
    if let Some(k) = kind {
        em.push(0, n, k);
    }
    LineState::default()
}
