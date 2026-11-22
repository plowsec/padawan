//! Markdown (CommonMark subset).
//!
//! | Construct | Token |
//! |---|---|
//! | ```` ``` ````/`~~~` fences (state 1: param fence char, extra length) | `CodeSpan` (whole lines) |
//! | `# h` … `###### h`, `===` underline | `Heading` (whole line) |
//! | `>` quote, `-`/`*`/`+`/`1.` bullet, `[ ]` task | `Marker` |
//! | `` `code` `` | `CodeSpan` |
//! | `**strong**`, `__strong__` | `Strong` |
//! | `*em*`, `_em_` | `Emphasis` |
//! | `[text](url)`, `![alt](src)`, `<https://…>`, bare `https://…` | `Link` |
//! | `<tag>` | `Tag`; `<!-- -->` (state 2) `Comment` |
//! | `\*` | `Escape` |
//!
//! Inline delimiters must close on the same line. Each failed search disables that delimiter for the rest
//! of the line, so a line costs O(length).

use super::{extra, mode, next_char, param, st, Em, LineState, TokenKind as K};

const M_FENCE: u8 = 1;
const M_COMMENT: u8 = 2;

fn fence_run(b: &[u8], i: usize) -> Option<(u8, usize)> {
    let c = *b.get(i)?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let len = b.get(i..)?.iter().take_while(|&&d| d == c).count();
    (len >= 3).then_some((c, len))
}

pub(crate) fn lex(em: &mut Em, line: &str, state: LineState) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let lead = b.iter().take_while(|&&c| c == b' ').count();
    match mode(state) {
        M_FENCE => {
            em.push(0, n, K::CodeSpan);
            let c = param(state);
            let want = usize::try_from(extra(state)).unwrap_or(usize::MAX);
            if lead <= 3 {
                if let Some((fc, len)) = fence_run(b, lead) {
                    let rest = b.get(lead + len..).unwrap_or(b"");
                    if fc == c && len >= want && rest.iter().all(u8::is_ascii_whitespace) {
                        return LineState::default();
                    }
                }
            }
            return state;
        }
        M_COMMENT => match find(b, 0, b"-->") {
            Some(p) => {
                em.push(0, p + 3, K::Comment);
                return inline(em, b, p + 3);
            }
            None => {
                em.push(0, n, K::Comment);
                return state;
            }
        },
        _ => {}
    }
    if lead > 3 {
        return inline(em, b, lead);
    }
    if let Some((c, len)) = fence_run(b, lead) {
        em.push(lead, n, K::CodeSpan);
        return st(M_FENCE, c, len as u64);
    }
    let hashes = b.get(lead..).unwrap_or(b"").iter().take_while(|&&c| c == b'#').count();
    if (1..=6).contains(&hashes) && matches!(b.get(lead + hashes), Some(b' ' | b'\t') | None) {
        em.push(lead, n, K::Heading);
        return LineState::default();
    }
    let rest = b.get(lead..).unwrap_or(b"");
    if rest.len() >= 2 && rest.iter().all(|&c| c == b'=') {
        em.push(lead, n, K::Heading);
        return LineState::default();
    }
    if rest.len() >= 3 {
        if let Some(&c) = rest.first().filter(|c| matches!(c, b'-' | b'*' | b'_')) {
            if rest.iter().all(|&d| d == c || d == b' ') && rest.iter().filter(|&&d| d == c).count() >= 3 {
                em.push(lead, n, K::Marker);
                return LineState::default();
            }
        }
    }
    let mut i = lead;
    // Quote markers, then one list bullet.
    while b.get(i) == Some(&b'>') {
        em.push(i, i + 1, K::Marker);
        i += 1;
        while b.get(i) == Some(&b' ') {
            i += 1;
        }
    }
    match b.get(i) {
        Some(b'-' | b'*' | b'+') if matches!(b.get(i + 1), Some(b' ' | b'\t')) => {
            em.push(i, i + 1, K::Marker);
            i += 2;
        }
        Some(c) if c.is_ascii_digit() => {
            let d = b.get(i..).unwrap_or(b"").iter().take_while(|c| c.is_ascii_digit()).count();
            if d <= 9 && matches!(b.get(i + d), Some(b'.' | b')')) && matches!(b.get(i + d + 1), Some(b' ' | b'\t')) {
                em.push(i, i + d + 1, K::Marker);
                i += d + 2;
            }
        }
        _ => {}
    }
    if matches!(b.get(i..i + 3), Some(b"[ ]" | b"[x]" | b"[X]")) {
        em.push(i, i + 3, K::Marker);
        i += 3;
    }
    inline(em, b, i)
}

fn find(b: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    b.get(from..)?.windows(pat.len()).position(|w| w == pat).map(|p| p + from)
}

fn find_byte(b: &[u8], from: usize, c: u8) -> Option<usize> {
    b.get(from..)?.iter().position(|&d| d == c).map(|p| p + from)
}

/// Inline spans from `i`. Returns the state for the next line (an open HTML comment).
fn inline(em: &mut Em, b: &[u8], i: usize) -> LineState {
    let n = b.len();
    let mut i = i;
    let mut no_code = [false; 16];
    let mut no_strong = [false; 2];
    let mut no_em = [false; 2];
    let mut no_bracket = false;
    let mut no_paren = false;
    let mut no_gt = false;
    while let Some(&c) = b.get(i) {
        match c {
            b'\\' if b.get(i + 1).is_some_and(u8::is_ascii_punctuation) => {
                em.push(i, i + 2, K::Escape);
                i += 2;
            }
            b'`' => {
                let k = b.get(i..).unwrap_or(b"").iter().take_while(|&&d| d == b'`').count();
                let slot = k.min(15);
                if !no_code[slot] {
                    if let Some(e) = code_close(b, i + k, k) {
                        em.push(i, e, K::CodeSpan);
                        i = e;
                        continue;
                    }
                    no_code[slot] = true;
                }
                i += k;
            }
            b'*' | b'_' => {
                let s = usize::from(c == b'_');
                let word_inside = c == b'_' && i > 0 && b.get(i - 1).is_some_and(u8::is_ascii_alphanumeric);
                if word_inside {
                    i += 1;
                    continue;
                }
                if b.get(i + 1) == Some(&c) {
                    if !no_strong[s] && b.get(i + 2).is_some_and(|d| !d.is_ascii_whitespace()) {
                        if let Some(p) = find(b, i + 3, &[c, c]) {
                            em.push(i, p + 2, K::Strong);
                            i = p + 2;
                            continue;
                        }
                        no_strong[s] = true;
                    }
                    i += 2;
                    continue;
                }
                if !no_em[s] && b.get(i + 1).is_some_and(|d| !d.is_ascii_whitespace()) {
                    if let Some(p) = em_close(b, i + 2, c) {
                        em.push(i, p + 1, K::Emphasis);
                        i = p + 1;
                        continue;
                    }
                    no_em[s] = true;
                }
                i += 1;
            }
            b'[' | b'!' if !no_bracket && (c == b'[' || b.get(i + 1) == Some(&b'[')) => {
                let open = if c == b'!' { i + 1 } else { i };
                let Some(close) = find_byte(b, open + 1, b']') else {
                    no_bracket = true;
                    i += 1;
                    continue;
                };
                match b.get(close + 1) {
                    Some(b'(') if !no_paren => {
                        em.push(i, close + 1, K::Link);
                        match find_byte(b, close + 2, b')') {
                            Some(p) => {
                                em.push(close + 1, p + 1, K::Link);
                                i = p + 1;
                            }
                            None => {
                                no_paren = true;
                                i = close + 1;
                            }
                        }
                    }
                    Some(b'[') | Some(b'(') => {
                        em.push(i, close + 1, K::Link);
                        i = close + 1;
                    }
                    _ => i = close + 1,
                }
            }
            b'<' if b.get(i + 1..i + 4) == Some(b"!--") => match find(b, i + 4, b"-->") {
                Some(p) => {
                    em.push(i, p + 3, K::Comment);
                    i = p + 3;
                }
                None => {
                    em.push(i, n, K::Comment);
                    return st(M_COMMENT, 0, 0);
                }
            },
            b'<' if !no_gt && b.get(i + 1).is_some_and(|d| d.is_ascii_alphabetic() || *d == b'/') => {
                match find_byte(b, i + 1, b'>') {
                    Some(p) => {
                        let inner = b.get(i + 1..p).unwrap_or(b"");
                        let url = inner.starts_with(b"http://") || inner.starts_with(b"https://") || inner.starts_with(b"mailto:");
                        em.push(i, p + 1, if url { K::Link } else { K::Tag });
                        i = p + 1;
                    }
                    None => {
                        no_gt = true;
                        i += 1;
                    }
                }
            }
            b'h' if (b.get(i..).is_some_and(|r| r.starts_with(b"https://") || r.starts_with(b"http://")))
                && (i == 0 || b.get(i - 1).is_some_and(|p| p.is_ascii_whitespace() || *p == b'(')) =>
            {
                let mut e = i;
                while b.get(e).is_some_and(|d| !d.is_ascii_whitespace() && *d != b')' && *d != b'>') {
                    e += 1;
                }
                em.push(i, e, K::Link);
                i = e;
            }
            _ => i = next_char(b, i),
        }
    }
    LineState::default()
}

/// End (exclusive) of a code span whose opener is a run of `k` backticks ending at `from`.
fn code_close(b: &[u8], from: usize, k: usize) -> Option<usize> {
    let mut j = from;
    while let Some(p) = find_byte(b, j, b'`') {
        let r = b.get(p..).unwrap_or(b"").iter().take_while(|&&d| d == b'`').count();
        if r == k {
            return Some(p + r);
        }
        j = p + r;
    }
    None
}

/// Closing single `*`/`_`: not preceded by a space, not part of a double.
fn em_close(b: &[u8], from: usize, c: u8) -> Option<usize> {
    let mut j = from;
    while let Some(p) = find_byte(b, j, c) {
        let prev_ok = b.get(p - 1).is_some_and(|d| !d.is_ascii_whitespace());
        let double = b.get(p + 1) == Some(&c);
        let after_ok = c != b'_' || !b.get(p + 1).is_some_and(u8::is_ascii_alphanumeric);
        if prev_ok && !double && after_ok {
            return Some(p);
        }
        j = p + if double { 2 } else { 1 };
    }
    None
}
