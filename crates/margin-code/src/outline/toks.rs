//! Token feed for the outline engines: lexes each line (strings and comments become opaque), splits
//! operators into single-char tokens, keeps the last `RING` tokens for look-back and pairs `(`/`)`.

use super::MAX_QUALIFIED;
use crate::lex::{is_ident_char, is_ident_start, lex_limited, LineState, Token, TokenKind};
use crate::Lang;

/// Look-back window, in tokens.
const RING: usize = 64;
const MAX_PARENS: usize = 256;
pub(crate) const NONE: usize = usize::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tk {
    Ident,
    Kw,
    /// Builtin type word, capitalised non-call name, constant: walkable in a path, never a callee.
    Ty,
    /// `@decorator`, `#[attr]`, macro `name!`, preprocessor word.
    Attr,
    Str,
    /// Number or other literal.
    Lit,
    P(u8),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Tok {
    pub k: Tk,
    /// Byte range in the whole text.
    pub s: usize,
    pub e: usize,
    pub line: u32,
    pub col: u32,
    /// For `)`: index of the matching `(`, else `NONE`.
    pub mate: usize,
}

const BLANK: Tok = Tok { k: Tk::Lit, s: 0, e: 0, line: 0, col: 0, mate: NONE };

#[derive(Clone, Copy, Default)]
pub(crate) struct CalleeOpts {
    /// `a::b` paths (C++, Rust, PHP, Ruby).
    pub dcolon: bool,
    /// `a->b` (C, C++, PHP), normalised to `.`.
    pub arrow: bool,
    /// `obj:method` (Lua).
    pub colon: bool,
    /// `name<T>(` and `name::<T>(`.
    pub generics: bool,
    /// `new Foo(` → qualified `new Foo`.
    pub new_kw: bool,
    /// `$` in identifiers (PHP, JS).
    pub dollar: bool,
    /// `$name(` is a variable call, not a name (PHP).
    pub php_vars: bool,
}

pub(crate) struct Callee {
    pub name_idx: usize,
    pub name_s: usize,
    pub name_e: usize,
    pub line: u32,
    pub col: u32,
    pub qualified: String,
    /// No path before the name.
    pub bare: bool,
    pub is_new: bool,
}

pub(crate) struct Feed<'t> {
    pub text: &'t str,
    ring: [Tok; RING],
    /// Tokens fed so far; the last one has index `count - 1`.
    count: usize,
    parens: Vec<usize>,
    paren_overflow: u32,
    pub opts: CalleeOpts,
}

/// What the engines implement.
pub(crate) trait Engine {
    /// Start of line `no` (`text` without the line break). `normal` is false inside a multi-line string or
    /// comment. Return `false` to skip this line's tokens.
    fn line(&mut self, _f: &Feed, _no: u32, _text: &str, _normal: bool) -> bool {
        true
    }
    fn tok(&mut self, f: &Feed, idx: usize);
    fn eol(&mut self, _f: &Feed, _no: u32) {}
}

impl<'t> Feed<'t> {
    pub(crate) fn get(&self, idx: usize) -> Option<&Tok> {
        if idx >= self.count || self.count - idx > RING {
            return None;
        }
        self.ring.get(idx % RING)
    }

    /// Token before `idx`.
    pub(crate) fn prev(&self, idx: usize) -> Option<&Tok> {
        self.get(idx.checked_sub(1)?)
    }

    pub(crate) fn text(&self, t: &Tok) -> &'t str {
        self.text.get(t.s..t.e).unwrap_or("")
    }

    pub(crate) fn is_kw(&self, idx: usize, w: &str) -> bool {
        self.get(idx).is_some_and(|t| t.k == Tk::Kw && self.text(t) == w)
    }

    pub(crate) fn is_p(&self, idx: usize, c: u8) -> bool {
        self.get(idx).is_some_and(|t| t.k == Tk::P(c))
    }

    /// `a` directly followed by `b` (no space).
    fn adjacent(&self, a: usize, b: usize) -> bool {
        matches!((self.get(a), self.get(b)), (Some(x), Some(y)) if x.e == y.s)
    }

    fn push(&mut self, mut t: Tok) -> usize {
        let idx = self.count;
        match t.k {
            Tk::P(b'(') => {
                if self.parens.len() < MAX_PARENS {
                    self.parens.push(idx);
                } else {
                    self.paren_overflow += 1;
                }
            }
            Tk::P(b')') => {
                if self.paren_overflow > 0 {
                    self.paren_overflow -= 1;
                } else if let Some(m) = self.parens.pop() {
                    t.mate = m;
                }
            }
            _ => {}
        }
        if let Some(slot) = self.ring.get_mut(idx % RING) {
            *slot = t;
        }
        self.count += 1;
        idx
    }

    /// Ident before `<` … `>` ending at `gt` (generic call `name<T>(` / turbofish `name::<T>(`).
    fn generic_name(&self, gt: usize) -> Option<usize> {
        let mut depth = 0u32;
        let mut brackets = 0u32;
        let mut k = gt;
        for _ in 0..32 {
            let t = self.get(k)?;
            match t.k {
                Tk::P(b']') => brackets += 1,
                Tk::P(b'[') => brackets = brackets.saturating_sub(1),
                // `[u8; 4]` inside the type list.
                Tk::Lit => {}
                Tk::P(b';') if brackets > 0 => {}
                Tk::P(b'>') => depth += 1,
                Tk::P(b'<') => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let mut n = k.checked_sub(1)?;
                        if self.is_p(n, b':') && self.is_p(n.checked_sub(1)?, b':') {
                            n = n.checked_sub(2)?;
                        }
                        let nt = self.get(n)?;
                        return matches!(nt.k, Tk::Ident | Tk::Ty).then_some(n);
                    }
                }
                Tk::Ident | Tk::Ty | Tk::Kw => {}
                Tk::P(b',' | b'.' | b':' | b'?' | b'*' | b'&') => {}
                _ => return None,
            }
            k = k.checked_sub(1)?;
        }
        None
    }

    /// Separator before token `j`: (normalised text, index of the token before it).
    fn sep_before(&self, j: usize) -> Option<(&'static str, usize)> {
        let a = j.checked_sub(1)?;
        let t = self.get(a)?;
        match t.k {
            Tk::P(b'.') => {
                let b = a.checked_sub(1)?;
                if self.is_p(b, b'?') && self.adjacent(b, a) {
                    Some((".", b.checked_sub(1)?))
                } else {
                    Some((".", b))
                }
            }
            Tk::P(b':') => {
                let b = a.checked_sub(1)?;
                if self.opts.dcolon && self.is_p(b, b':') && self.adjacent(b, a) {
                    Some(("::", b.checked_sub(1)?))
                } else if self.opts.colon {
                    Some((":", b))
                } else {
                    None
                }
            }
            Tk::P(b'>') if self.opts.arrow => {
                let b = a.checked_sub(1)?;
                (self.is_p(b, b'-') && self.adjacent(b, a)).then_some((".", b.checked_sub(1)?))
            }
            _ => None,
        }
    }

    /// The callee of the `(` at `p`, if `p` follows a name.
    pub(crate) fn callee(&self, p: usize) -> Option<Callee> {
        let before = p.checked_sub(1)?;
        let bt = self.get(before)?;
        let ni = match bt.k {
            Tk::Ident => before,
            Tk::P(b'>') if self.opts.generics => self.generic_name(before)?,
            _ => return None,
        };
        let nt = *self.get(ni)?;
        let name = self.text(&nt);
        if name.is_empty() || name.len() > super::MAX_NAME || (self.opts.php_vars && name.starts_with('$')) {
            return None;
        }
        let (qualified, bare, start) = self.qualify(ni, name);
        let is_new = self.opts.new_kw && start.checked_sub(1).is_some_and(|k| self.is_kw(k, "new"));
        let qualified = if is_new { format!("new {}", if bare { name } else { &qualified }) } else { qualified };
        Some(Callee { name_idx: ni, name_s: nt.s, name_e: nt.e, line: nt.line, col: nt.col, qualified, bare, is_new })
    }

    /// Path written before the name at `ni`: (qualified, bare, index of the first token of the path).
    pub(crate) fn qualify(&self, ni: usize, name: &str) -> (String, bool, usize) {
        // (segment, separator after it, call, single string argument)
        let mut parts: Vec<(&str, &str, bool, &str)> = Vec::new();
        let mut total = name.len();
        let mut j = ni;
        for _ in 0..16 {
            let Some((sep, b)) = self.sep_before(j) else { break };
            let Some(t) = self.get(b) else { break };
            let (seg, call, arg, next_j) = match t.k {
                Tk::Ident | Tk::Kw | Tk::Ty => (self.text(t), false, "", b),
                Tk::P(b')') if t.mate != NONE => {
                    let Some(c) = t.mate.checked_sub(1) else { break };
                    // `require('child_process').exec`: keep a lone string argument.
                    let arg = match self.get(t.mate + 1) {
                        Some(a) if a.k == Tk::Str && t.mate + 2 == b => self.text(a),
                        _ => "",
                    };
                    match self.get(c) {
                        Some(ct) if ct.k == Tk::Ident => (self.text(ct), true, arg, c),
                        _ => break,
                    }
                }
                _ => break,
            };
            let add = seg.len() + sep.len() + if call { 2 + arg.len() } else { 0 };
            if seg.is_empty() || total + add > MAX_QUALIFIED {
                break;
            }
            total += add;
            parts.push((seg, sep, call, arg));
            j = next_j;
        }
        if parts.is_empty() {
            return (String::new(), true, ni);
        }
        let mut q = String::with_capacity(total);
        for (seg, sep, call, arg) in parts.iter().rev() {
            q.push_str(seg);
            if *call {
                q.push('(');
                q.push_str(arg);
                q.push(')');
            }
            q.push_str(sep);
        }
        q.push_str(name);
        (q, false, j)
    }
}

/// Lexes `text` line by line and feeds `eng`. Returns the last line index.
pub(crate) fn run(lang: Lang, text: &str, opts: CalleeOpts, eng: &mut impl Engine) -> u32 {
    let mut f = Feed { text, ring: [BLANK; RING], count: 0, parens: Vec::new(), paren_overflow: 0, opts };
    let mut state = LineState::default();
    let mut toks: Vec<Token> = Vec::new();
    let mut off = 0usize;
    let mut no: u32 = 0;
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let normal = state.0 & 0xFF == 0;
        toks.clear();
        let next = lex_limited(lang, state, line, &mut toks, line.len());
        if eng.line(&f, no, line, normal) {
            feed_line(&mut f, eng, line, off, no, &toks, lang == Lang::Shell);
        }
        eng.eol(&f, no);
        state = next;
        off = off.saturating_add(raw.len() + 1);
        no = no.saturating_add(1);
    }
    no.saturating_sub(1)
}

struct LineCtx {
    off: usize,
    no: u32,
    shell: bool,
}

fn emit<E: Engine>(f: &mut Feed, eng: &mut E, cx: &LineCtx, k: Tk, s: usize, e: usize) {
    let col = u32::try_from(s).unwrap_or(u32::MAX);
    let idx = f.push(Tok { k, s: cx.off + s, e: cx.off + e, line: cx.no, col, mate: NONE });
    eng.tok(f, idx);
}

fn feed_line<E: Engine>(f: &mut Feed, eng: &mut E, line: &str, off: usize, no: u32, toks: &[Token], shell: bool) {
    let b = line.as_bytes();
    let dollar = f.opts.dollar;
    let cx = LineCtx { off, no, shell };
    let mut pos = 0usize;
    let mut last_str_end: Option<usize> = None;
    for t in toks {
        let (s, e) = (t.start as usize, t.end as usize);
        if s > pos {
            gap(f, eng, &cx, b, pos, s, dollar);
            last_str_end = None;
        }
        let ident_like = b.get(s).is_some_and(|&c| is_ident_start(c) || (dollar && c == b'$'));
        let is_str = matches!(t.kind, TokenKind::String | TokenKind::Escape | TokenKind::CodeSpan);
        if is_str && last_str_end == Some(s) {
            // Next piece of the same string.
            last_str_end = Some(e);
            f.extend_last(off + e);
            pos = e;
            continue;
        }
        last_str_end = if is_str { Some(e) } else { None };
        let k = match t.kind {
            TokenKind::Comment => None,
            TokenKind::String | TokenKind::Escape | TokenKind::CodeSpan => Some(Tk::Str),
            TokenKind::Keyword => Some(Tk::Kw),
            TokenKind::Function if shell => Some(Tk::Ident),
            TokenKind::Function | TokenKind::Text if ident_like && !shell => Some(Tk::Ident),
            TokenKind::Type | TokenKind::Constant if ident_like => Some(Tk::Ty),
            TokenKind::Attribute => Some(Tk::Attr),
            TokenKind::Operator | TokenKind::Punct => {
                for (k, &c) in b.get(s..e).unwrap_or(b"").iter().enumerate() {
                    if c.is_ascii_punctuation() {
                        emit(f, eng, &cx, Tk::P(c), s + k, s + k + 1);
                    }
                }
                None
            }
            _ => Some(Tk::Lit),
        };
        if let Some(k) = k {
            emit(f, eng, &cx, k, s, e);
        }
        pos = e;
    }
    if pos < b.len() {
        gap(f, eng, &cx, b, pos, b.len(), dollar);
    }
}

/// Untokenised range: identifiers, numbers, punctuation.
fn gap<E: Engine>(f: &mut Feed, eng: &mut E, cx: &LineCtx, b: &[u8], from: usize, to: usize, dollar: bool) {
    let mut i = from;
    while i < to {
        let Some(&c) = b.get(i) else { break };
        if is_ident_start(c) || (dollar && c == b'$') {
            let mut e = i + 1;
            while e < to && b.get(e).is_some_and(|&d| is_ident_char(d) || (dollar && d == b'$')) {
                e += 1;
            }
            // Shell: only command words (lexer `Function` tokens) are names.
            emit(f, eng, cx, if cx.shell { Tk::Lit } else { Tk::Ident }, i, e);
            i = e;
        } else if c.is_ascii_digit() {
            let mut e = i + 1;
            while e < to && b.get(e).is_some_and(|&d| is_ident_char(d) || d == b'.') {
                e += 1;
            }
            emit(f, eng, cx, Tk::Lit, i, e);
            i = e;
        } else if c.is_ascii_punctuation() {
            emit(f, eng, cx, Tk::P(c), i, i + 1);
            i += 1;
        } else {
            i += 1;
        }
    }
}

impl Feed<'_> {
    fn extend_last(&mut self, e: usize) {
        if let Some(last) = self.count.checked_sub(1) {
            if let Some(t) = self.ring.get_mut(last % RING) {
                t.e = e;
            }
        }
    }
}
