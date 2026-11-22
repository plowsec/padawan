//! Heuristic argument flow for call paths: intra-procedural, by name, token-based (strings and comments
//! never count). Pure; never panics; linear in the tokens looked at.
//!
//! | Item | What |
//! |---|---|
//! | `Src` | text + line starts; build once per file, then query |
//! | `params` / `Src::sig` | parameter names of a definition, by position (`""` = unreadable slot) |
//! | `call_args` / `Src::call_args` | byte ranges of the arguments of a call |
//! | `taint` / `Src::taint` | locals fed by tracked names, before a point |
//! | `Src::arg_flow` | per argument of a call: short text + tracked names that reach it |
//!
//! Taint rules (union, never cleared; flow-insensitive inside the function):
//!
//! | Statement | Taints |
//! |---|---|
//! | `x = e`, `T x = e`, `let x = e`, `x := e`, `x += e`, `var x T = e` | `x` |
//! | `a, b = e`, `let (a, b) = e`, `const {a, b} = e` | every name on the left |
//! | `x.f = e`, `x[i] = e`, `*x = e` | the base `x` |
//! | `for x in e`, `for (const x of e)`, `foreach (e as $k => $v)` | the loop names |
//! | `f(out)` (out-parameters), aliasing, fields read back | nothing |
//!
//! Caps: `MAX_PARAMS` parameters, `MAX_ARGS` arguments, `ARG_TEXT` bytes of argument text,
//! `MAX_FN_TOKENS` tokens per function scan, `ARG_BYTES` bytes from a call to its `)`, first `LEX_LINE_CAP`
//! bytes of a line (calls and assignments past that are not seen).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::lex::{floor_boundary, is_ident_char, is_ident_start, lex_line, LineState, Token, TokenKind, LEX_LINE_CAP};
use crate::{Call, FnDef, Lang};

/// Arguments read per call.
pub const MAX_ARGS: usize = 16;
/// Bytes of one argument's text.
pub const ARG_TEXT: usize = 96;
/// Parameters read per definition (also the tracked-name cap: one bit each).
pub const MAX_PARAMS: usize = 64;
/// Tokens scanned in one function body.
pub const MAX_FN_TOKENS: usize = 400_000;
/// Bytes from the callee name to the closing `)` of its call.
pub const ARG_BYTES: usize = 64 << 10;
/// Longest name kept (parameter or local).
pub const MAX_NAME: usize = 255;
/// Locals tracked per function.
const MAX_LOCALS: usize = 8192;
/// Header tokens read for a parameter list.
const HEADER_TOKENS: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum K {
    Id,
    Kw,
    Ty,
    Str,
    Lit,
    P(u8),
}

/// Tokens of a call and its argument token ranges.
type Args = (Vec<T>, Vec<(usize, usize)>);

#[derive(Clone, Copy, Debug)]
struct T {
    k: K,
    s: usize,
    e: usize,
    line: u32,
}

/// Parameters of a definition, by position.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sig {
    /// `""` where a slot has no readable name. `self` / `this` slots are dropped.
    pub names: Vec<String>,
    /// Last parameter takes the rest (`...`, `*args`, `vararg`, `params`).
    pub variadic: bool,
}

/// One argument of a call with the tracked names reaching it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArgFlow {
    /// Source text, whitespace collapsed, comments dropped, at most `ARG_TEXT` bytes.
    pub text: String,
    /// Sorted, distinct.
    pub from: Vec<String>,
}

/// A text with its line starts.
pub struct Src<'a> {
    lang: Lang,
    text: &'a str,
    starts: Vec<usize>,
}

/// Parameter names of `f` (see `Src::sig`).
pub fn params(lang: Lang, text: &str, f: &FnDef) -> Vec<String> {
    Src::new(lang, text).sig(f).names
}

/// Byte ranges (in `text`) of the arguments of call `c`. `None`: no argument list found.
pub fn call_args(lang: Lang, text: &str, c: &Call) -> Option<Vec<(usize, usize)>> {
    Src::new(lang, text).call_args(c.line, c.col)
}

/// Locals of `f` fed by `tracked` names on lines before `upto_line` → the tracked names behind each.
pub fn taint(lang: Lang, text: &str, f: &FnDef, tracked: &[String], upto_line: u32) -> BTreeMap<String, BTreeSet<String>> {
    Src::new(lang, text).taint(f, tracked, upto_line, 0)
}

fn uses_dollar(lang: Lang) -> bool {
    matches!(lang, Lang::Php | Lang::JavaScript | Lang::TypeScript)
}

/// `name: Type` annotations (on parameters and declarations).
fn annotates(lang: Lang) -> bool {
    matches!(lang, Lang::Rust | Lang::TypeScript | Lang::Python | Lang::Kotlin | Lang::Swift | Lang::Ruby)
}

/// `<T>` generic lists after a name.
fn generics(lang: Lang) -> bool {
    matches!(lang, Lang::Rust | Lang::TypeScript | Lang::Java | Lang::CSharp | Lang::Kotlin | Lang::Swift | Lang::Cpp)
}

fn is_this(s: &str) -> bool {
    matches!(s, "self" | "this" | "$this")
}

impl<'a> Src<'a> {
    pub fn new(lang: Lang, text: &'a str) -> Src<'a> {
        let mut starts = vec![0usize];
        starts.extend(text.bytes().enumerate().filter(|(_, b)| *b == b'\n').map(|(i, _)| i + 1));
        Src { lang, text, starts }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// Line `n` without its line break and `\r`, with its start offset.
    fn line(&self, n: u32) -> Option<(usize, &'a str)> {
        let s = *self.starts.get(n as usize)?;
        let e = self.starts.get(n as usize + 1).map_or(self.text.len(), |x| x.saturating_sub(1));
        let l = self.text.get(s..e.max(s))?;
        Some((s, l.strip_suffix('\r').unwrap_or(l)))
    }

    /// Byte offset of `line:col` (clamped to the line).
    pub fn offset(&self, line: u32, col: u32) -> Option<usize> {
        let (s, l) = self.line(line)?;
        Some(s + (col as usize).min(l.len()))
    }

    fn tx(&self, t: &T) -> &'a str {
        self.text.get(t.s..t.e).unwrap_or("")
    }

    /// Tokens of lines `l0..=l1`, starting in the default lexer state at `l0`. Stops at offset `stop`
    /// (exclusive), after `max` tokens, or past offset `max_off`.
    fn toks(&self, l0: u32, l1: u32, stop: usize, max: usize) -> Vec<T> {
        let dollar = uses_dollar(self.lang);
        let mut out: Vec<T> = Vec::new();
        let mut state = LineState::default();
        let mut buf: Vec<Token> = Vec::new();
        let mut no = l0;
        while no <= l1 {
            let Some((off, line)) = self.line(no) else { break };
            if off >= stop || out.len() >= max {
                break;
            }
            buf.clear();
            // Huge line: lex its head only (the state after it may be off; bounded work matters more).
            let cap = floor_boundary(line, LEX_LINE_CAP);
            let line = line.get(..cap).unwrap_or_default();
            state = lex_line(self.lang, state, line, &mut buf);
            let b = line.as_bytes();
            let mut pos = 0usize;
            let mut last_str: Option<usize> = None;
            let mut push = |out: &mut Vec<T>, k: K, s: usize, e: usize| {
                if off + s < stop && out.len() < max {
                    out.push(T { k, s: off + s, e: off + e, line: no });
                }
            };
            for t in &buf {
                let (s, e) = (t.start as usize, (t.end as usize).min(cap));
                if s >= e {
                    continue;
                }
                if s > pos {
                    gap(&mut out, &mut push, b, pos, s, dollar);
                    last_str = None;
                }
                let is_str = matches!(t.kind, TokenKind::String | TokenKind::Escape | TokenKind::CodeSpan);
                if is_str && last_str == Some(s) {
                    if let Some(l) = out.last_mut() {
                        if l.k == K::Str && l.e == off + s {
                            l.e = off + e;
                        }
                    }
                    last_str = Some(e);
                    pos = e;
                    continue;
                }
                last_str = if is_str { Some(e) } else { None };
                let ident = b.get(s).is_some_and(|&c| is_ident_start(c) || (dollar && c == b'$'));
                match t.kind {
                    TokenKind::Comment | TokenKind::Attribute => {}
                    TokenKind::String | TokenKind::Escape | TokenKind::CodeSpan => push(&mut out, K::Str, s, e),
                    TokenKind::Keyword => push(&mut out, K::Kw, s, e),
                    TokenKind::Function | TokenKind::Text if ident => push(&mut out, K::Id, s, e),
                    TokenKind::Type | TokenKind::Constant if ident => push(&mut out, K::Ty, s, e),
                    TokenKind::Operator | TokenKind::Punct | TokenKind::Text => {
                        for (k, &c) in b.get(s..e).unwrap_or_default().iter().enumerate() {
                            if c.is_ascii_punctuation() {
                                push(&mut out, K::P(c), s + k, s + k + 1);
                            }
                        }
                    }
                    _ => push(&mut out, K::Lit, s, e),
                }
                pos = e;
            }
            if pos < cap {
                gap(&mut out, &mut push, b, pos, cap, dollar);
            }
            no = match no.checked_add(1) {
                Some(n) => n,
                None => break,
            };
        }
        out
    }

    /// Index of the token at `line:col`.
    fn find_at(toks: &[T], off: usize) -> Option<usize> {
        let i = toks.partition_point(|t| t.s < off);
        toks.get(i).filter(|t| t.s == off).map(|_| i)
    }

    /// Parameters of `f`, by position.
    pub fn sig(&self, f: &FnDef) -> Sig {
        let Some(name_off) = self.offset(f.line, f.col) else { return Sig::default() };
        let last = f.end.min(f.line.saturating_add(256)).max(f.line);
        let toks = self.toks(f.line, last, usize::MAX, HEADER_TOKENS);
        let Some(ni) = Self::find_at(&toks, name_off) else { return Sig::default() };
        let Some((open, list)) = self.param_list(&toks, ni) else { return Sig::default() };
        let segs: Vec<&[T]> = if open {
            split_commas(list)
        } else {
            list.split(|t| t.k == K::P(b',')).filter(|s| !s.is_empty()).collect()
        };
        let mut sig = Sig::default();
        if segs.len() == 1 && segs.first().is_some_and(|s| s.len() == 1 && s.first().is_some_and(|t| self.tx(t) == "void")) {
            return sig;
        }
        for seg in segs {
            if sig.names.len() >= MAX_PARAMS {
                break;
            }
            let (name, variadic) = self.param_name(seg);
            if is_this(&name) {
                continue;
            }
            sig.variadic = variadic;
            sig.names.push(if name.len() <= MAX_NAME { name } else { String::new() });
        }
        sig
    }

    /// Tokens of the parameter list after the name at `ni`. `(true, inside)` for `( … )`; `(false, …)` for a
    /// bare list (Ruby `def f a, b`, JS `x => …`).
    fn param_list<'t>(&self, toks: &'t [T], ni: usize) -> Option<(bool, &'t [T])> {
        let mut j = ni + 1;
        for _ in 0..16 {
            let t = toks.get(j)?;
            match t.k {
                K::P(b'(') => break,
                K::P(b'<') if generics(self.lang) => j = skip_pair(toks, j, b'<', b'>')?,
                K::P(b'[') if self.lang == Lang::Go => j = skip_pair(toks, j, b'[', b']')?,
                K::P(b'=' | b':' | b'*' | b'!' | b'?') => j += 1,
                K::Kw if matches!(self.tx(t), "function" | "async" | "fn" | "func" | "fun") => j += 1,
                K::Id if self.lang == Lang::Ruby && t.line == toks.get(ni)?.line => {
                    let end = toks.iter().skip(j).position(|x| x.line != t.line).map_or(toks.len(), |p| p + j);
                    return Some((false, toks.get(j..end)?));
                }
                K::Id if matches!(self.lang, Lang::JavaScript | Lang::TypeScript) => {
                    // `x => …`
                    let arrow = toks.get(j + 1).is_some_and(|a| a.k == K::P(b'=')) && toks.get(j + 2).is_some_and(|b| b.k == K::P(b'>'));
                    return arrow.then(|| (false, toks.get(j..j + 1).unwrap_or_default()));
                }
                _ => return None,
            }
        }
        let close = match_close(toks, j)?;
        Some((true, toks.get(j + 1..close)?))
    }

    /// Name of one parameter segment, and whether it is variadic.
    fn param_name(&self, seg: &[T]) -> (String, bool) {
        let dots = seg.windows(3).any(|w| w.iter().all(|t| t.k == K::P(b'.')));
        let star = seg.first().is_some_and(|t| t.k == K::P(b'*')) && matches!(self.lang, Lang::Python | Lang::Ruby);
        let kwvar = seg.iter().any(|t| matches!(self.tx(t), "vararg" | "params") && t.k != K::Str);
        let variadic = dots || star || kwvar;
        // Default value: cut at the first top-level lone `=`.
        let mut end = seg.len();
        let mut d = 0i32;
        for (i, t) in seg.iter().enumerate() {
            match t.k {
                K::P(b'(' | b'[' | b'{') => d += 1,
                K::P(b')' | b']' | b'}') => d -= 1,
                K::P(b'=') if d == 0 && !next_is(seg, i, b'=') && !next_is(seg, i, b'>') && !prev_is(seg, i, b'=') => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
        let seg = seg.get(..end).unwrap_or_default();
        let name_of = |t: &T| self.tx(t).to_owned();
        if annotates(self.lang) {
            if let Some(c) = top_colon(seg) {
                let before = seg.get(..c).unwrap_or_default();
                return (before.iter().rev().find(|t| t.k == K::Id || is_this(self.tx(t))).map(name_of).unwrap_or_default(), variadic);
            }
        }
        let name = match self.lang {
            Lang::Php => seg.iter().find(|t| self.tx(t).starts_with('$')).map(name_of),
            Lang::Go | Lang::Python | Lang::Ruby | Lang::Lua | Lang::JavaScript | Lang::TypeScript => {
                seg.iter().find(|t| t.k == K::Id || is_this(self.tx(t))).map(name_of)
            }
            _ => {
                // C family: `int (*cb)(int)` → name inside the first group; else the last word at depth 0.
                let fptr = seg.windows(3).find(|w| w[0].k == K::P(b'(') && w[1].k == K::P(b'*') && w[2].k == K::Id);
                match fptr {
                    Some(w) => Some(name_of(&w[2])),
                    None => {
                        let mut d = 0i32;
                        let mut last: Option<&T> = None;
                        for t in seg {
                            match t.k {
                                K::P(b'(' | b'[' | b'{' | b'<') => d += 1,
                                K::P(b')' | b']' | b'}' | b'>') => d -= 1,
                                K::Id if d == 0 => last = Some(t),
                                K::Kw | K::Ty if d == 0 && is_this(self.tx(t)) => last = Some(t),
                                _ => {}
                            }
                        }
                        last.map(name_of)
                    }
                }
            }
        };
        (name.unwrap_or_default(), variadic)
    }

    /// Tokens from the call name at `line:col` to its `)`, and the argument token ranges.
    fn args_at(&self, line: u32, col: u32) -> Option<Args> {
        let off = self.offset(line, col)?;
        let stop = off.saturating_add(ARG_BYTES);
        let toks = self.toks(line, line.saturating_add(4096), stop, 16_384);
        let ni = Self::find_at(&toks, off)?;
        if toks.get(ni)?.k != K::Id {
            return None;
        }
        let mut j = ni + 1;
        for _ in 0..8 {
            let t = toks.get(j)?;
            match t.k {
                K::P(b'(') => break,
                K::P(b'<') if generics(self.lang) => j = skip_pair(&toks, j, b'<', b'>')?,
                K::P(b'!' | b'?' | b'.' | b':') => j += 1,
                K::Str if self.lang == Lang::Lua => return Some((toks.clone(), vec![(j, j + 1)])),
                K::P(b'{') if self.lang == Lang::Lua => {
                    let close = skip_pair(&toks, j, b'{', b'}')?;
                    return Some((toks.clone(), vec![(j, close)]));
                }
                _ if self.lang == Lang::Ruby && t.line == line && !matches!(t.k, K::P(_)) && !matches!(self.tx(t), "do" | "if" | "unless") => {
                    // `system cmd, arg` without parentheses: to the end of the line.
                    let end = toks.iter().skip(j).position(|x| x.line != line || matches!(self.tx(x), "do" | "if" | "unless")).map_or(toks.len(), |p| p + j);
                    let list = toks.get(j..end)?;
                    let ranges = arg_ranges(list, j);
                    return Some((toks.clone(), ranges));
                }
                _ => return None,
            }
        }
        let close = match_close(&toks, j)?;
        let list = toks.get(j + 1..close)?;
        let ranges = arg_ranges(list, j + 1);
        Some((toks, ranges))
    }

    /// Byte ranges of the arguments of the call whose name is at `line:col`.
    pub fn call_args(&self, line: u32, col: u32) -> Option<Vec<(usize, usize)>> {
        let (toks, r) = self.args_at(line, col)?;
        Some(r.iter().filter_map(|&(a, b)| Some((toks.get(a)?.s, toks.get(b.checked_sub(1)?)?.e))).collect())
    }

    /// Short text of tokens `toks[a..b]`.
    fn arg_text(&self, toks: &[T], a: usize, b: usize) -> String {
        let mut s = String::new();
        let mut prev_e: Option<usize> = None;
        for t in toks.get(a..b).unwrap_or_default() {
            if s.len() > ARG_TEXT {
                break;
            }
            if prev_e.is_some_and(|p| p < t.s) {
                s.push(' ');
            }
            let piece = self.tx(t);
            let mut ws = false;
            for c in piece.chars() {
                if c.is_whitespace() {
                    if !ws {
                        s.push(' ');
                    }
                    ws = true;
                } else {
                    s.push(c);
                    ws = false;
                }
                if s.len() > ARG_TEXT + 4 {
                    break;
                }
            }
            prev_e = Some(t.e);
        }
        let t = s.trim();
        t.get(..floor_boundary(t, ARG_TEXT)).unwrap_or_default().trim_end().to_owned()
    }

    /// Locals of `f` fed by `tracked` before `stop_line:stop_col` → tracked names behind each.
    pub fn taint(&self, f: &FnDef, tracked: &[String], stop_line: u32, stop_col: u32) -> BTreeMap<String, BTreeSet<String>> {
        let tr = Tracked::new(tracked);
        let st = self.scan(f, &tr, stop_line, stop_col);
        st.into_iter().map(|(k, m)| (k.to_owned(), tr.names(m).into_iter().collect())).collect()
    }

    fn scan(&self, f: &FnDef, tr: &Tracked, stop_line: u32, stop_col: u32) -> HashMap<&'a str, u64> {
        let mut map: HashMap<&'a str, u64> = HashMap::new();
        if tr.list.is_empty() {
            return map;
        }
        let stop = self.offset(stop_line, stop_col).unwrap_or(usize::MAX);
        let toks = self.toks(f.start.min(f.line), f.end.min(stop_line), stop, MAX_FN_TOKENS);
        let mut st_start = 0usize;
        let mut depth: Vec<u8> = Vec::new();
        let mut i = 0usize;
        while i < toks.len() {
            let Some(t) = toks.get(i) else { break };
            // Newline at depth 0 ends a statement unless the line continues.
            if i > st_start && depth.is_empty() {
                if let Some(p) = toks.get(i - 1) {
                    if p.line != t.line && !continues(p) && !matches!(t.k, K::P(b'.' | b')' | b']')) {
                        self.stmt(toks.get(st_start..i).unwrap_or_default(), tr, &mut map);
                        st_start = i;
                    }
                }
            }
            match t.k {
                K::P(b'(') => depth.push(b'('),
                K::P(b'[') => depth.push(b'['),
                K::P(b')' | b']') => {
                    depth.pop();
                }
                K::P(b'{') => {
                    if !depth.is_empty() || self.expr_brace(&toks, st_start, i) {
                        depth.push(b'{');
                    } else {
                        self.stmt(toks.get(st_start..i).unwrap_or_default(), tr, &mut map);
                        st_start = i + 1;
                    }
                }
                K::P(b'}') => {
                    if depth.last() == Some(&b'{') {
                        depth.pop();
                    } else {
                        depth.clear();
                        self.stmt(toks.get(st_start..i).unwrap_or_default(), tr, &mut map);
                        st_start = i + 1;
                    }
                }
                K::P(b';') if depth.is_empty() => {
                    self.stmt(toks.get(st_start..i).unwrap_or_default(), tr, &mut map);
                    st_start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        self.stmt(toks.get(st_start..).unwrap_or_default(), tr, &mut map);
        map
    }

    /// `{` at `i` opens an expression (object literal, closure body inside an expression), not a block.
    fn expr_brace(&self, toks: &[T], st_start: usize, i: usize) -> bool {
        if i <= st_start {
            return false;
        }
        let Some(p) = toks.get(i - 1) else { return false };
        match p.k {
            // `=> {`: a function body.
            K::P(b'>') if i >= 2 && toks.get(i - 2).is_some_and(|q| q.k == K::P(b'=') && q.e == p.s) => false,
            K::P(b'=' | b'(' | b',' | b':' | b'[' | b'?' | b'|' | b'&' | b'+' | b'-' | b'*' | b'/' | b'%' | b'<' | b'>' | b'!') => true,
            // `return {`, `const {a} = …` (destructuring).
            K::Kw => matches!(self.tx(p), "return" | "yield" | "await" | "in" | "of" | "const" | "let" | "var" | "val"),
            _ => false,
        }
    }

    fn names_in(&self, s: &[T], tr: &Tracked, map: &HashMap<&'a str, u64>) -> u64 {
        let mut m = 0u64;
        for (i, t) in s.iter().enumerate() {
            if is_name(self, t) && !member(self.lang, s, i) {
                m |= tr.mask(self.tx(t)) | map.get(self.tx(t)).copied().unwrap_or(0);
            }
        }
        m
    }

    fn stmt(&self, s: &[T], tr: &Tracked, map: &mut HashMap<&'a str, u64>) {
        if s.is_empty() {
            return;
        }
        // Loops: `for x in e`, `for (const x of e)`, `foreach (e as $v)`.
        if s.first().is_some_and(|t| matches!(self.tx(t), "for" | "foreach") && t.k != K::Str) {
            if let Some(k) = s.iter().position(|t| matches!(self.tx(t), "in" | "of" | "as") && t.k != K::Str) {
                let (a, b) = (s.get(1..k).unwrap_or_default(), s.get(k + 1..).unwrap_or_default());
                let (lhs, rhs) = if s.get(k).is_some_and(|t| self.tx(t) == "as") { (b, a) } else { (a, b) };
                let m = self.names_in(rhs, tr, map);
                if m != 0 {
                    for t in lhs.iter().filter(|t| t.k == K::Id) {
                        add(map, self.tx(t), m);
                    }
                }
                return;
            }
        }
        // First top-level assignment.
        let mut d = 0i32;
        let mut at = None;
        for (i, t) in s.iter().enumerate() {
            match t.k {
                K::P(b'(' | b'[' | b'{') => d += 1,
                K::P(b')' | b']' | b'}') => d -= 1,
                K::P(b'=') if d == 0 && assign_at(s, i) => {
                    at = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let Some(a) = at else { return };
        let m = self.names_in(s.get(a + 1..).unwrap_or_default(), tr, map);
        if m == 0 {
            return;
        }
        for part in split_commas(s.get(..a).unwrap_or_default()) {
            for name in self.targets(part) {
                add(map, name, m);
            }
        }
    }

    /// Names assigned by one comma part of a left side.
    fn targets(&self, part: &[T]) -> Vec<&'a str> {
        let part = match (annotates(self.lang), top_colon(part)) {
            (true, Some(c)) => part.get(..c).unwrap_or_default(),
            _ => part,
        };
        let first = part.iter().position(|t| !matches!(t.k, K::Kw | K::P(b'*' | b'&')) || is_this(self.tx(t)));
        if let Some(f) = first {
            if part.get(f).is_some_and(|t| matches!(t.k, K::P(b'(' | b'[' | b'{'))) {
                return part.iter().skip(f).filter(|t| t.k == K::Id).map(|t| self.tx(t)).collect();
            }
        }
        if self.lang == Lang::Go {
            if let Some(v) = part.iter().position(|t| t.k == K::Kw && self.tx(t) == "var") {
                return part.iter().skip(v + 1).find(|t| t.k == K::Id).map(|t| self.tx(t)).into_iter().collect();
            }
        }
        let mut d = 0i32;
        let mut last = None;
        for (i, t) in part.iter().enumerate() {
            match t.k {
                K::P(b'(' | b'[' | b'{') => d += 1,
                K::P(b')' | b']' | b'}') => d -= 1,
                _ if d == 0 && is_name(self, t) && !member(self.lang, part, i) => last = Some(self.tx(t)),
                _ => {}
            }
        }
        last.into_iter().filter(|n| n.len() <= MAX_NAME).collect()
    }

    /// Arguments of the call at `line:col` inside `f`, each with the `tracked` names (parameters of `f`)
    /// reaching it. `None`: the argument list could not be read.
    pub fn arg_flow(&self, f: &FnDef, line: u32, col: u32, tracked: &[String]) -> Option<Vec<ArgFlow>> {
        let (toks, ranges) = self.args_at(line, col)?;
        let tr = Tracked::new(tracked);
        let map = self.scan(f, &tr, line, col);
        Some(
            ranges
                .iter()
                .map(|&(a, b)| {
                    let m = self.names_in(toks.get(a..b).unwrap_or_default(), &tr, &map);
                    ArgFlow { text: self.arg_text(&toks, a, b), from: tr.names(m) }
                })
                .collect(),
        )
    }
}

/// Tracked names → bit positions.
struct Tracked<'n> {
    list: Vec<&'n str>,
    idx: HashMap<&'n str, u32>,
}

impl<'n> Tracked<'n> {
    fn new(names: &'n [String]) -> Self {
        let mut list: Vec<&str> = Vec::new();
        let mut idx = HashMap::new();
        for n in names.iter().filter(|n| !n.is_empty() && n.len() <= MAX_NAME) {
            if list.len() >= 64 {
                break;
            }
            if !idx.contains_key(n.as_str()) {
                idx.insert(n.as_str(), list.len() as u32);
                list.push(n.as_str());
            }
        }
        Tracked { list, idx }
    }

    fn mask(&self, n: &str) -> u64 {
        self.idx.get(n).map_or(0, |&i| 1u64 << i)
    }

    /// Sorted names of a mask, at most `MAX_ARGS`.
    fn names(&self, m: u64) -> Vec<String> {
        let mut v: Vec<&str> = (0..self.list.len()).filter(|i| m & (1u64 << i) != 0).filter_map(|i| self.list.get(i).copied()).collect();
        v.sort_unstable();
        v.dedup();
        v.into_iter().take(MAX_ARGS).map(str::to_owned).collect()
    }
}

fn add<'a>(map: &mut HashMap<&'a str, u64>, name: &'a str, m: u64) {
    if let Some(e) = map.get_mut(name) {
        *e |= m;
    } else if map.len() < MAX_LOCALS {
        map.insert(name, m);
    }
}

fn is_name(src: &Src, t: &T) -> bool {
    t.k == K::Id || (matches!(t.k, K::Kw | K::Ty) && is_this(src.tx(t)))
}

/// Token `i` is a member name: after `.` (not PHP / Lua `..` concatenation), `->` or `::`.
fn member(lang: Lang, s: &[T], i: usize) -> bool {
    let Some(p) = i.checked_sub(1).and_then(|j| s.get(j)) else { return false };
    match p.k {
        K::P(b'.') => lang != Lang::Php && !(i >= 2 && s.get(i - 2).is_some_and(|q| q.k == K::P(b'.') && q.e == p.s)),
        K::P(b'>') => i >= 2 && s.get(i - 2).is_some_and(|q| q.k == K::P(b'-') && q.e == p.s),
        K::P(b':') => i >= 2 && s.get(i - 2).is_some_and(|q| q.k == K::P(b':') && q.e == p.s),
        _ => false,
    }
}

/// Line ends but the statement goes on.
fn continues(t: &T) -> bool {
    matches!(t.k, K::P(b'=' | b',' | b'(' | b'[' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^' | b'<' | b'>' | b'?' | b'.' | b'\\'))
}

fn next_is(s: &[T], i: usize, c: u8) -> bool {
    match (s.get(i), s.get(i + 1)) {
        (Some(a), Some(b)) => b.k == K::P(c) && a.e == b.s,
        _ => false,
    }
}

fn prev_is(s: &[T], i: usize, c: u8) -> bool {
    match (i.checked_sub(1).and_then(|j| s.get(j)), s.get(i)) {
        (Some(a), Some(b)) => a.k == K::P(c) && a.e == b.s,
        _ => false,
    }
}

/// `=` at `i` assigns (`=`, `:=`, `+=`, `<<=`, `??=`), not compares (`==`, `<=`, `!=`) or arrows (`=>`).
fn assign_at(s: &[T], i: usize) -> bool {
    if next_is(s, i, b'=') || next_is(s, i, b'>') {
        return false;
    }
    if prev_is(s, i, b'=') || prev_is(s, i, b'!') {
        return false;
    }
    if prev_is(s, i, b'<') || prev_is(s, i, b'>') {
        let c = if prev_is(s, i, b'<') { b'<' } else { b'>' };
        return i >= 1 && prev_is(s, i - 1, c);
    }
    true
}

/// Position of a top-level single `:` (not `::`).
fn top_colon(s: &[T]) -> Option<usize> {
    let mut d = 0i32;
    for (i, t) in s.iter().enumerate() {
        match t.k {
            K::P(b'(' | b'[' | b'{' | b'<') => d += 1,
            K::P(b')' | b']' | b'}' | b'>') => d -= 1,
            K::P(b':') if d == 0 && !next_is(s, i, b':') && !prev_is(s, i, b':') => return Some(i),
            _ => {}
        }
    }
    None
}

/// Split at top-level commas (depth over `()[]{}`, and `<>` when it looks like a type list).
fn split_commas(s: &[T]) -> Vec<&[T]> {
    let mut out = Vec::new();
    let mut d = 0i32;
    let mut a = 0usize;
    for (i, t) in s.iter().enumerate() {
        match t.k {
            K::P(b'(' | b'[' | b'{') => d += 1,
            K::P(b')' | b']' | b'}') => d -= 1,
            K::P(b'<') if !prev_is(s, i, b'<') && !next_is(s, i, b'<') && !next_is(s, i, b'=') => d += 1,
            K::P(b'>') if d > 0 && !prev_is(s, i, b'-') && !prev_is(s, i, b'=') && !prev_is(s, i, b'>') && !next_is(s, i, b'>') && !next_is(s, i, b'=') => d -= 1,
            K::P(b',') if d <= 0 => {
                if let Some(p) = s.get(a..i) {
                    out.push(p);
                }
                a = i + 1;
                d = 0;
            }
            _ => {}
        }
    }
    if let Some(p) = s.get(a..) {
        if !p.is_empty() {
            out.push(p);
        }
    }
    out
}

/// Argument token ranges (absolute indices, `base` = index of `list[0]`), commas at depth 0, at most `MAX_ARGS`.
fn arg_ranges(list: &[T], base: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut d = 0i32;
    let mut a = 0usize;
    for (i, t) in list.iter().enumerate() {
        match t.k {
            K::P(b'(' | b'[' | b'{') => d += 1,
            K::P(b')' | b']' | b'}') => d -= 1,
            K::P(b',') if d <= 0 => {
                if i > a {
                    out.push((base + a, base + i));
                }
                a = i + 1;
                d = 0;
                if out.len() >= MAX_ARGS {
                    return out;
                }
            }
            _ => {}
        }
    }
    if list.len() > a && out.len() < MAX_ARGS {
        out.push((base + a, base + list.len()));
    }
    out
}

/// Index of the `)` closing the `(` at `open` (balanced over `()[]{}`).
fn match_close(toks: &[T], open: usize) -> Option<usize> {
    let mut stack: Vec<u8> = Vec::new();
    for (i, t) in toks.iter().enumerate().skip(open) {
        match t.k {
            K::P(c @ (b'(' | b'[' | b'{')) => stack.push(c),
            K::P(c @ (b')' | b']' | b'}')) => {
                let want = match c {
                    b')' => b'(',
                    b']' => b'[',
                    _ => b'{',
                };
                // Unbalanced inner bracket: drop to the matching opener if there is one.
                while let Some(&top) = stack.last() {
                    if top == want {
                        break;
                    }
                    stack.pop();
                }
                stack.pop();
                if stack.is_empty() {
                    return (c == b')').then_some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Index of the closer matching the opener at `i` (`<…>`, `[…]`, `{…}`), within 256 tokens.
fn skip_pair(toks: &[T], i: usize, open: u8, close: u8) -> Option<usize> {
    let mut d = 0u32;
    for (j, t) in toks.iter().enumerate().skip(i).take(256) {
        if t.k == K::P(open) {
            d += 1;
        } else if t.k == K::P(close) {
            if open == b'<' && j > 0 && toks.get(j - 1).is_some_and(|p| p.k == K::P(b'-') && p.e == t.s) {
                continue;
            }
            d = d.saturating_sub(1);
            if d == 0 {
                return Some(j + 1);
            }
        }
    }
    None
}

/// Untokenised range: identifiers, numbers, punctuation.
fn gap(out: &mut Vec<T>, push: &mut impl FnMut(&mut Vec<T>, K, usize, usize), b: &[u8], from: usize, to: usize, dollar: bool) {
    let mut i = from;
    while i < to {
        let Some(&c) = b.get(i) else { break };
        if is_ident_start(c) || (dollar && c == b'$') {
            let mut e = i + 1;
            while e < to && b.get(e).is_some_and(|&d| is_ident_char(d) || (dollar && d == b'$')) {
                e += 1;
            }
            push(out, K::Id, i, e);
            i = e;
        } else if c.is_ascii_digit() {
            let mut e = i + 1;
            while e < to && b.get(e).is_some_and(|&d| is_ident_char(d) || d == b'.') {
                e += 1;
            }
            push(out, K::Lit, i, e);
            i = e;
        } else if c.is_ascii_punctuation() {
            push(out, K::P(c), i, i + 1);
            i += 1;
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outline;

    fn def<'o>(o: &'o crate::Outline, name: &str) -> &'o FnDef {
        o.fns.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no fn {name} in {o:?}"))
    }

    fn ps(lang: Lang, text: &str, name: &str) -> Vec<String> {
        let o = outline(lang, text);
        params(lang, text, def(&o, name))
    }

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn params_per_family() {
        let cases: &[(Lang, &str, &str, &[&str])] = &[
            (Lang::C, "int copy(char *dst, const char *src, size_t n) {\n  return 0;\n}\n", "copy", &["dst", "src", "n"]),
            (Lang::C, "void f(void) {\n}\n", "f", &[]),
            (Lang::C, "void f(int (*cb)(int), int a[4]) {\n}\n", "f", &["cb", "a"]),
            (Lang::Cpp, "void f(const std::string& s, std::map<int, int> m, int k = 3) {\n}\n", "f", &["s", "m", "k"]),
            (Lang::Java, "class A {\n  void run(final String cmd, List<String> args, String... rest) {\n  }\n}\n", "run", &["cmd", "args", "rest"]),
            (Lang::CSharp, "class A {\n  void Run(this string s, ref int x, params string[] more) {\n  }\n}\n", "Run", &["s", "x", "more"]),
            (Lang::Go, "func (s *Srv) Handle(w http.ResponseWriter, r *http.Request, a, b int) {\n}\n", "Handle", &["w", "r", "a", "b"]),
            (Lang::Rust, "fn go<T: Copy>(&mut self, mut buf: Vec<u8>, n: usize) -> u8 {\n    0\n}\n", "go", &["buf", "n"]),
            (Lang::TypeScript, "function f<T>(a: string, b?: number, c = 2, ...rest: T[]): void {\n}\n", "f", &["a", "b", "c", "rest"]),
            (Lang::JavaScript, "const f = (a, b) => {\n  return a;\n};\n", "f", &["a", "b"]),
            (Lang::JavaScript, "function g(req, res) {\n}\n", "g", &["req", "res"]),
            (Lang::Python, "def f(self, a, b: int = 3, *args, **kw):\n    pass\n", "f", &["a", "b", "args", "kw"]),
            (Lang::Php, "<?php\nfunction f(int $a, ?Foo $b = null, &$c, ...$d) {\n}\n", "f", &["$a", "$b", "$c", "$d"]),
            (Lang::Kotlin, "fun f(a: Int, vararg xs: String) {\n}\n", "f", &["a", "xs"]),
            (Lang::Swift, "func f(_ a: Int, label b: String) {\n}\n", "f", &["a", "b"]),
            (Lang::Ruby, "def f(a, b = 2, *rest, key:)\nend\n", "f", &["a", "b", "rest", "key"]),
            (Lang::Ruby, "def g a, b\nend\n", "g", &["a", "b"]),
            (Lang::Lua, "function M.f(a, b, ...)\nend\n", "f", &["a", "b", ""]),
        ];
        for (lang, text, name, want) in cases {
            assert_eq!(ps(*lang, text, name), v(want), "{lang:?} {text:?}");
        }
        let o = outline(Lang::Python, "def f(*args):\n    pass\n");
        assert!(Src::new(Lang::Python, "def f(*args):\n    pass\n").sig(def(&o, "f")).variadic);
    }

    #[test]
    fn call_args_split() {
        let text = "void f(void) {\n  g(a, h(b, c), \"x, y\" /* , */, d[1, 2],\n    e);\n}\n";
        let o = outline(Lang::C, text);
        let c = def(&o, "f").calls.iter().find(|c| c.name == "g").unwrap();
        let r = call_args(Lang::C, text, c).unwrap();
        let texts: Vec<&str> = r.iter().map(|&(a, b)| &text[a..b]).collect();
        assert_eq!(texts, vec!["a", "h(b, c)", "\"x, y\"", "d[1, 2]", "e"]);
        let empty = "void f(void) {\n  g();\n}\n";
        let o = outline(Lang::C, empty);
        assert_eq!(call_args(Lang::C, empty, &def(&o, "f").calls[0]), Some(vec![]));
        // Unbalanced: no closing paren.
        let bad = "void f(void) {\n  g(a, (b\n}\n";
        let o = outline(Lang::C, bad);
        if let Some(c) = def(&o, "f").calls.first() {
            let _ = call_args(Lang::C, bad, c);
        }
    }

    fn flow(lang: Lang, text: &str, fname: &str, callee: &str) -> Vec<(String, Vec<String>)> {
        let o = outline(lang, text);
        let f = def(&o, fname);
        let c = f.calls.iter().find(|c| c.name == callee).unwrap_or_else(|| panic!("no call {callee} in {f:?}"));
        let src = Src::new(lang, text);
        let tracked = src.sig(f).names;
        src.arg_flow(f, c.line, c.col, &tracked).unwrap().into_iter().map(|a| (a.text, a.from)).collect()
    }

    fn a(t: &str, f: &[&str]) -> (String, Vec<String>) {
        (t.to_owned(), v(f))
    }

    #[test]
    fn taint_c_like() {
        let c = "int handle(struct req *req, int n) {\n  char *p = req->body;\n  int k;\n  k = n * 2;\n  char buf[64];\n  copy(buf, p, k);\n  memcpy(buf, \"req\", 4 /* n */);\n}\n";
        assert_eq!(flow(Lang::C, c, "handle", "copy"), vec![a("buf", &[]), a("p", &["req"]), a("k", &["n"])]);
        assert_eq!(flow(Lang::C, c, "handle", "memcpy"), vec![a("buf", &[]), a("\"req\"", &[]), a("4", &[])]);
        let java = "class A {\n  void run(String cmd, int x) {\n    String full = \"sh -c \" + cmd;\n    for (String part : parts) {}\n    Runtime.getRuntime().exec(full);\n  }\n}\n";
        assert_eq!(flow(Lang::Java, java, "run", "exec"), vec![a("full", &["cmd"])]);
        let go = "func run(w http.ResponseWriter, r *http.Request) {\n\tq := r.URL.Query()\n\tvar name string = q.Get(\"n\")\n\tfor _, v := range r.Form {\n\t}\n\texec.Command(name, v)\n}\n";
        assert_eq!(flow(Lang::Go, go, "run", "Command"), vec![a("name", &["r"]), a("v", &["r"])]);
        let rs = "fn run(input: &str, n: usize) {\n    let (a, b) = split(input);\n    let mut c: String = String::with_capacity(8);\n    c += b;\n    Command::new(c).arg(n);\n}\n";
        assert_eq!(flow(Lang::Rust, rs, "run", "new"), vec![a("c", &["input"])]);
    }

    #[test]
    fn taint_scripts() {
        let py = "def view(self, request, extra=None):\n    cmd = request.args.get('c')\n    parts = [cmd, 'x']\n    for p in parts:\n        pass\n    os.system(p)\n";
        assert_eq!(flow(Lang::Python, py, "view", "system"), vec![a("p", &["request"])]);
        let js = "function h(req, res) {\n  const { q, z } = req.query;\n  let s = `ls ${q}`;\n  child_process.exec(s, res);\n}\n";
        assert_eq!(flow(Lang::JavaScript, js, "h", "exec"), vec![a("s", &[]), a("res", &["res"])]);
        let js2 = "function h(req) {\n  const { q } = req.query;\n  const s = 'ls ' + q;\n  exec(s);\n}\n";
        assert_eq!(flow(Lang::JavaScript, js2, "h", "exec"), vec![a("s", &["req"])]);
        let php = "<?php\nfunction f($rows, $n) {\n  foreach ($rows as $k => $v) {\n    $q = \"SELECT \" . $v;\n  }\n  mysqli_query($db, $q);\n}\n";
        assert_eq!(flow(Lang::Php, php, "f", "mysqli_query"), vec![a("$db", &[]), a("$q", &["$rows"])]);
        let rb = "def run(cmd, opts)\n  full = \"x #{1}\" + cmd\n  system full, opts\nend\n";
        assert_eq!(flow(Lang::Ruby, rb, "run", "system"), vec![a("full", &["cmd"]), a("opts", &["opts"])]);
        let lua = "function run(cmd)\n  local c = cmd .. ' -x'\n  os.execute(c)\nend\n";
        assert_eq!(flow(Lang::Lua, lua, "run", "execute"), vec![a("c", &["cmd"])]);
        let kt = "fun run(cmd: String) {\n    val full: String = \"sh \" + cmd\n    Runtime.getRuntime().exec(full)\n}\n";
        assert_eq!(flow(Lang::Kotlin, kt, "run", "exec"), vec![a("full", &["cmd"])]);
    }

    #[test]
    fn taint_map_and_text() {
        let c = "void f(int a, int b) {\n  int x = a;\n  int y = x + b;\n  int z = 3;\n  g(y);\n}\n";
        let o = outline(Lang::C, c);
        let t = taint(Lang::C, c, def(&o, "f"), &v(&["a", "b"]), 4);
        assert_eq!(t.get("y").map(|s| s.iter().cloned().collect::<Vec<_>>()), Some(v(&["a", "b"])));
        assert!(!t.contains_key("z"));
        // Only lines before `upto_line` count.
        let t = taint(Lang::C, c, def(&o, "f"), &v(&["a", "b"]), 2);
        assert!(t.contains_key("x") && !t.contains_key("y"));
        // Long argument text is cut, whitespace collapsed, comments dropped.
        let long = format!("void f(int a) {{\n  g(a   +\n  /* c */ {});\n}}\n", "é".repeat(200));
        let r = flow(Lang::C, &long, "f", "g");
        assert!(r[0].0.len() <= ARG_TEXT && r[0].0.starts_with("a + é"), "{:?}", r[0].0);
        assert_eq!(r[0].1, v(&["a"]));
    }
}
