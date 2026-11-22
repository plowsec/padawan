//! Test helpers shared by the integration tests.
#![allow(dead_code)]

pub mod fixtures;

use margin_code::{lex_line, outline, FnDef, Lang, LineState, Token, TokenKind};

/// Token contract: in order, non-overlapping, non-empty, on char boundaries, inside the line.
pub fn check_tokens(line: &str, toks: &[Token]) {
    let mut last = 0u32;
    for t in toks {
        assert!(t.start < t.end, "empty token {t:?} in {line:?}");
        assert!(t.start >= last, "overlap/unsorted {t:?} in {:?}", short(line));
        assert!((t.end as usize) <= line.len(), "past end {t:?}");
        assert!(line.is_char_boundary(t.start as usize) && line.is_char_boundary(t.end as usize), "off boundary {t:?}");
        last = t.end;
    }
}

fn short(s: &str) -> &str {
    let mut e = s.len().min(80);
    while !s.is_char_boundary(e) {
        e -= 1;
    }
    &s[..e]
}

/// Lex a whole text; checks the contract and determinism of every line. Returns tokens per line.
pub fn lex_text(lang: Lang, text: &str) -> Vec<(Vec<Token>, LineState)> {
    let mut st = LineState::default();
    let mut out = Vec::new();
    for line in text.split('\n') {
        let mut v = Vec::new();
        let next = lex_line(lang, st, line, &mut v);
        check_tokens(line, &v);
        let mut again = Vec::new();
        let next2 = lex_line(lang, st, line, &mut again);
        assert_eq!(v, again, "relex differs: {line:?}");
        assert_eq!(next, next2);
        out.push((v, st));
        st = next;
    }
    out
}

/// `(text, kind)` of every token of the text, line by line flattened.
pub fn spans(lang: Lang, text: &str) -> Vec<(String, TokenKind)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut v = Vec::new();
    for (k, (toks, _)) in lex_text(lang, text).iter().enumerate() {
        for t in toks {
            v.push((lines[k][t.start as usize..t.end as usize].to_string(), t.kind));
        }
    }
    v
}

/// Assert that `text` contains a token `s` of kind `kind`.
#[track_caller]
pub fn has(sp: &[(String, TokenKind)], s: &str, kind: TokenKind) {
    assert!(
        sp.iter().any(|(t, k)| t == s && *k == kind),
        "no {kind:?} token {s:?}; got {:?}",
        sp.iter().filter(|(t, _)| t.contains(s) || s.contains(t.as_str())).collect::<Vec<_>>()
    );
}

#[track_caller]
pub fn not_kind(sp: &[(String, TokenKind)], s: &str, kind: TokenKind) {
    assert!(!sp.iter().any(|(t, k)| t == s && *k == kind), "unexpected {kind:?} token {s:?}");
}

pub fn names(o: &[FnDef]) -> Vec<&str> {
    o.iter().map(|f| f.name.as_str()).collect()
}

pub fn fns(lang: Lang, text: &str) -> Vec<FnDef> {
    outline(lang, text).fns
}

#[track_caller]
pub fn get<'a>(o: &'a [FnDef], name: &str) -> &'a FnDef {
    o.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no fn {name}; have {:?}", names(o)))
}

pub fn calls(f: &FnDef) -> Vec<&str> {
    f.calls.iter().map(|c| c.name.as_str()).collect()
}

pub fn dump(o: &[FnDef]) -> String {
    let mut s = String::new();
    for f in o {
        s.push_str(&format!("{} @{}:{} [{}..={}]\n", f.name, f.line, f.col, f.start, f.end));
        for c in &f.calls {
            s.push_str(&format!("    {} q={:?} @{}:{} {:?}\n", c.name, c.qualified, c.line, c.col, c.sink));
        }
    }
    s
}
