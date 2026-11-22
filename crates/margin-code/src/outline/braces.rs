//! Brace languages: one engine, one `Cfg` per language.
//!
//! | Lang | Definitions found |
//! |---|---|
//! | C, C++ | `name(params) [quals] {` at file/namespace/class level; `Class::method`, `~Dtor`; `#if/#else` branches re-synced |
//! | Java, C# | same, inside class bodies and anonymous classes `new X() {`; C# `F() => expr;` |
//! | Rust | `fn name` (impl/trait/nested) |
//! | Go | `func name(`, `func (r T) Name(` |
//! | JS, TS | `function name`, `export default function`, methods `name(…) {`, `name = (…) =>`, `name: function`, `name = async x =>` |
//! | PHP | `function name` |
//! | Kotlin | `fun [<T>] [Recv.]name(`, `constructor(`, `fun f() = expr` |
//! | Swift | `func name`, `init(` |
//!
//! Not definitions: prototypes (`;` before `{`), macro calls, control keywords, closures/lambdas (their calls
//! still count for the enclosing function), K&R parameter declarations.

use super::toks::{self, CalleeOpts, Engine, Feed, Tk, Tok};
use super::Out;
use crate::lex::is_all_caps;
use crate::{FnDef, Lang};

const MAX_SCOPES: usize = 1024;
const MAX_PARENS: usize = 256;
const MAX_PP: usize = 64;

struct Cfg {
    kw_defs: &'static [&'static str],
    /// Keywords that are themselves the name of a definition when followed by `(` (Swift `init`).
    name_kws: &'static [&'static str],
    c_style: bool,
    shorthand: bool,
    arrow: bool,
    expr_eq: bool,
    expr_arrow: bool,
    go_recv: bool,
    asi: bool,
    preproc: bool,
    containers: &'static [&'static str],
    ignore: &'static [&'static str],
    callee: CalleeOpts,
}

const BASE: Cfg = Cfg {
    kw_defs: &[],
    name_kws: &[],
    c_style: false,
    shorthand: false,
    arrow: false,
    expr_eq: false,
    expr_arrow: false,
    go_recv: false,
    asi: false,
    preproc: false,
    containers: &[],
    ignore: &[],
    callee: CalleeOpts { dcolon: false, arrow: false, colon: false, generics: false, new_kw: false, dollar: false, php_vars: false },
};

/// Words that look like `name(` before a C declarator but never name it.
const C_SKIP: &[&str] = &[
    "__attribute", "__attribute__", "__declspec", "_Alignas", "alignas", "decltype", "noexcept", "throw", "requires",
    "__asm__", "asm", "defined", "__typeof__", "typeof", "sizeof", "alignof",
];

fn cfg(lang: Lang) -> Cfg {
    let c_opts = CalleeOpts { arrow: true, ..BASE.callee };
    match lang {
        Lang::C => Cfg {
            c_style: true,
            preproc: true,
            containers: &["enum", "extern", "struct", "union"],
            callee: c_opts,
            ..BASE
        },
        Lang::Cpp => Cfg {
            c_style: true,
            preproc: true,
            containers: &["class", "enum", "extern", "namespace", "struct", "union"],
            callee: CalleeOpts { dcolon: true, generics: true, new_kw: true, ..c_opts },
            ..BASE
        },
        Lang::Java => Cfg {
            c_style: true,
            containers: &["class", "enum", "interface", "record"],
            callee: CalleeOpts { generics: true, new_kw: true, ..BASE.callee },
            ..BASE
        },
        Lang::CSharp => Cfg {
            c_style: true,
            expr_arrow: true,
            preproc: true,
            containers: &["class", "enum", "interface", "namespace", "record", "struct"],
            callee: CalleeOpts { generics: true, new_kw: true, ..BASE.callee },
            ..BASE
        },
        Lang::Rust => Cfg {
            kw_defs: &["fn"],
            ignore: &["Err", "Fn", "FnMut", "FnOnce", "Ok", "Some"],
            callee: CalleeOpts { dcolon: true, generics: true, ..BASE.callee },
            ..BASE
        },
        Lang::Go => Cfg { kw_defs: &["func"], go_recv: true, asi: true, ..BASE },
        Lang::JavaScript | Lang::TypeScript => Cfg {
            kw_defs: &["function"],
            shorthand: true,
            arrow: true,
            asi: true,
            callee: CalleeOpts {
                new_kw: true,
                dollar: true,
                generics: lang == Lang::TypeScript,
                ..BASE.callee
            },
            ..BASE
        },
        Lang::Php => Cfg {
            kw_defs: &["function"],
            callee: CalleeOpts { dcolon: true, arrow: true, new_kw: true, dollar: true, php_vars: true, ..BASE.callee },
            ..BASE
        },
        Lang::Kotlin => Cfg {
            kw_defs: &["fun"],
            name_kws: &["constructor"],
            expr_eq: true,
            asi: true,
            callee: CalleeOpts { generics: true, ..BASE.callee },
            ..BASE
        },
        Lang::Swift => Cfg {
            kw_defs: &["func"],
            name_kws: &["init"],
            asi: true,
            callee: CalleeOpts { generics: true, ..BASE.callee },
            ..BASE
        },
        _ => BASE,
    }
}

pub(crate) fn run(lang: Lang, text: &str) -> Vec<FnDef> {
    let cfg = cfg(lang);
    let mut e = Eng {
        cfg: &cfg,
        out: Out::new(lang),
        scopes: Vec::new(),
        brace_ovf: 0,
        pd: 0,
        head: Head::default(),
        kd: Kd::Idle,
        ar: Ar::None,
        ar_colon: false,
        eq: None,
        pstack: Vec::new(),
        povf: 0,
        last_close: None,
        clean: false,
        last_attr: false,
        last_tok: None,
        pp: Vec::new(),
        pp_ovf: 0,
        pp_cont: false,
    };
    let last = toks::run(lang, text, cfg.callee, &mut e);
    while !e.scopes.is_empty() {
        e.close_scope(last);
    }
    e.out.finish(last)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sk {
    Container,
    Fn,
    Block,
    /// Expression body (`=> expr`, `= expr`): no brace, ends at `;`, `,`, a closing bracket or end of line.
    Expr,
}

#[derive(Clone, Copy)]
struct Scope {
    k: Sk,
    pd: u32,
    opened: bool,
}

#[derive(Clone, Copy)]
struct Name {
    s: usize,
    e: usize,
    line: u32,
    col: u32,
    idx: usize,
}

impl Name {
    fn of(t: &Tok, idx: usize) -> Name {
        Name { s: t.s, e: t.e, line: t.line, col: t.col, idx }
    }
}

#[derive(Clone, Copy)]
struct Def {
    name: Name,
    fixed: Option<&'static str>,
    tilde: bool,
    start: u32,
}

/// Declaration head at the current scope level (C-style candidates).
#[derive(Default)]
struct Head {
    start: Option<u32>,
    cand: Option<Name>,
    tilde: bool,
    closed: bool,
    dead: bool,
    after_qual: bool,
    container: bool,
    saw_eq: bool,
}

/// Keyword-led definition in progress.
enum Kd {
    Idle,
    Kw { start: u32, pd: u32, sl: usize, kw: Name, name: Option<Name>, angle: u32, deflt: bool, arrow: Option<(Name, u32)> },
    Recv { start: u32, pd: u32, sl: usize },
    AfterRecv { start: u32, pd: u32, sl: usize, name: Option<Name> },
    Sig { def: Def, pd: u32, sl: usize, line: u32 },
}

/// JS/TS `name = (…) =>` in progress.
enum Ar {
    None,
    Named { name: Name, start: u32, pd: u32 },
    Open { name: Name, start: u32, pd: u32 },
    Body { name: Name, start: u32 },
}

/// What an open `(` was preceded by.
#[derive(Clone, Copy)]
struct Lite {
    name: Option<Name>,
    bare: bool,
    is_new: bool,
    attr: bool,
}

struct Close {
    lite: Lite,
    idx: usize,
    pd: u32,
}

struct Pp {
    saved_len: usize,
    saved_pd: u32,
    skip: bool,
    inherited: bool,
    if0: bool,
}

struct Eng<'c> {
    cfg: &'c Cfg,
    out: Out,
    scopes: Vec<Scope>,
    brace_ovf: u32,
    /// Paren + bracket depth.
    pd: u32,
    head: Head,
    kd: Kd,
    ar: Ar,
    /// `ar` was set by `name:` (a type annotation may follow before `=`).
    ar_colon: bool,
    /// A `=` waiting for the next token (`=>`, `==` or assignment).
    eq: Option<usize>,
    pstack: Vec<Lite>,
    povf: u32,
    last_close: Option<Close>,
    /// Only type-annotation tokens since `last_close` (JS/TS methods).
    clean: bool,
    last_attr: bool,
    last_tok: Option<Tok>,
    pp: Vec<Pp>,
    pp_ovf: u32,
    pp_cont: bool,
}

fn is_op_char(c: u8) -> bool {
    matches!(c, b'=' | b'!' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^' | b'~' | b'?' | b':' | b'.')
}

impl Eng<'_> {
    fn base(&self) -> u32 {
        self.scopes.last().map_or(0, |s| s.pd)
    }

    fn decl_level(&self) -> bool {
        self.scopes.last().is_none_or(|s| s.k == Sk::Container)
    }

    fn reset_head(&mut self) {
        self.head = Head::default();
    }

    fn open(&mut self, f: &Feed, def: Def, k: Sk) {
        if self.scopes.len() >= MAX_SCOPES {
            if k == Sk::Fn {
                self.brace_ovf += 1;
            }
            return;
        }
        let text = def.fixed.unwrap_or_else(|| f.text.get(def.name.s..def.name.e).unwrap_or(""));
        let opened = if def.tilde {
            let n = format!("~{text}");
            self.out.open_fn(&n, def.name.line, def.name.col, def.start)
        } else {
            self.out.open_fn(text, def.name.line, def.name.col, def.start)
        };
        let k = if opened { k } else if k == Sk::Expr { return } else { Sk::Block };
        self.scopes.push(Scope { k, pd: self.pd, opened });
    }

    fn close_scope(&mut self, line: u32) {
        if let Some(s) = self.scopes.pop() {
            if s.opened {
                self.out.close_fn(line);
            }
            self.pd = s.pd;
        }
    }

    fn top_is_expr(&self) -> bool {
        self.scopes.last().is_some_and(|s| s.k == Sk::Expr)
    }

    fn skipping(&self) -> bool {
        self.pp.iter().any(|p| p.skip)
    }

    fn reset_states(&mut self) {
        self.reset_head();
        self.kd = Kd::Idle;
        self.ar = Ar::None;
        self.eq = None;
        self.last_close = None;
    }

    fn directive(&mut self, rest: &str, no: u32) {
        let word_len = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        let word = rest.get(..word_len).unwrap_or("");
        let arg = rest.get(word_len..).unwrap_or("").trim();
        match word {
            "if" | "ifdef" | "ifndef" => {
                if self.pp.len() >= MAX_PP {
                    self.pp_ovf += 1;
                    return;
                }
                let inherited = self.skipping();
                let if0 = word == "if" && (arg == "0" || arg.starts_with("0 ") || arg.starts_with("0/"));
                self.pp.push(Pp { saved_len: self.scopes.len(), saved_pd: self.pd, skip: inherited || if0, inherited, if0 });
            }
            "elif" | "else" => {
                if self.pp_ovf > 0 {
                    return;
                }
                let cur = self.scopes.len();
                let Some(top) = self.pp.last_mut() else { return };
                if top.inherited {
                    return;
                }
                if top.if0 {
                    top.if0 = false;
                    top.skip = false;
                    return;
                }
                if top.skip {
                    return;
                }
                let (saved_len, saved_pd) = (top.saved_len, top.saved_pd);
                if cur > saved_len {
                    while self.scopes.len() > saved_len {
                        self.close_scope(no.saturating_sub(1));
                    }
                    self.pd = saved_pd;
                    self.reset_states();
                } else if cur < saved_len {
                    if let Some(top) = self.pp.last_mut() {
                        top.skip = true;
                    }
                } else {
                    self.pd = saved_pd;
                }
            }
            "endif" => {
                if self.pp_ovf > 0 {
                    self.pp_ovf -= 1;
                } else {
                    self.pp.pop();
                }
            }
            _ => {}
        }
    }

    // ---- token steps ----

    fn on_assign(&mut self, f: &Feed, eq: usize) {
        let pd = self.pd;
        if self.cfg.c_style && pd == self.base() {
            if self.head.closed {
                self.head.dead = true;
            }
            self.head.saw_eq = true;
        }
        if self.cfg.expr_eq {
            if let Kd::Sig { def, pd: spd, .. } = self.kd {
                if spd == pd {
                    self.kd = Kd::Idle;
                    self.open(f, def, Sk::Expr);
                    return;
                }
            }
        }
        if self.cfg.arrow {
            let prev = f.prev(eq).copied();
            let typed = self.ar_colon && matches!(self.ar, Ar::Named { pd: apd, .. } if apd == pd);
            self.ar_colon = false;
            match prev {
                // `const name: Type = …`: keep `name`.
                _ if typed => {}
                Some(p) if matches!(p.k, Tk::Ident | Tk::Ty) => {
                    let start = self.head.start.unwrap_or(p.line);
                    self.ar = Ar::Named { name: Name::of(&p, eq - 1), start, pd };
                }
                _ => {
                    if !matches!(self.ar, Ar::Named { pd: apd, .. } if apd == pd) {
                        self.ar = Ar::None;
                    }
                }
            }
        }
    }

    fn on_fat_arrow(&mut self, f: &Feed) {
        let pd = self.pd;
        if self.cfg.arrow {
            self.ar = match self.ar {
                Ar::Named { name, start, pd: apd } if apd == pd => Ar::Body { name, start },
                _ => Ar::None,
            };
        }
        if self.cfg.expr_arrow && self.cfg.c_style && self.decl_level() && pd == self.base() && self.head.closed && !self.head.dead {
            if let Some(c) = self.head.cand {
                let def = Def { name: c, fixed: None, tilde: self.head.tilde, start: self.head.start.unwrap_or(c.line) };
                self.out.uncall(c.line, c.col);
                self.reset_head();
                self.open(f, def, Sk::Expr);
            }
        }
    }

    fn kd_step(&mut self, f: &Feed, idx: usize, t: &Tok) {
        let pd = self.pd;
        let text = f.text(t);
        // `func(T) T` / `fn(u8)` inside a signature is a type, not a new definition.
        let in_head = match self.kd {
            Kd::Kw { pd: k, .. } | Kd::Recv { pd: k, .. } | Kd::AfterRecv { pd: k, .. } | Kd::Sig { pd: k, .. } => pd > k,
            Kd::Idle => false,
        };
        let starts_def = t.k == Tk::Kw
            && !in_head
            && self.cfg.kw_defs.contains(&text)
            && !f.prev(idx).is_some_and(|p| p.k == Tk::P(b'.'));
        if starts_def {
            let arrow = match self.ar {
                Ar::Named { name, start, pd: apd } if apd == pd => Some((name, start)),
                _ => None,
            };
            let deflt = idx.checked_sub(1).is_some_and(|p| f.is_kw(p, "default"));
            self.kd = Kd::Kw {
                start: self.head.start.unwrap_or(t.line),
                pd,
                sl: self.scopes.len(),
                kw: Name::of(t, idx),
                name: None,
                angle: 0,
                deflt,
                arrow,
            };
            return;
        }
        if t.k == Tk::Kw && self.cfg.name_kws.contains(&text) && matches!(self.kd, Kd::Idle) {
            self.kd = Kd::Kw {
                start: self.head.start.unwrap_or(t.line),
                pd,
                sl: self.scopes.len(),
                kw: Name::of(t, idx),
                name: Some(Name::of(t, idx)),
                angle: 0,
                deflt: false,
                arrow: None,
            };
            return;
        }
        let first_on_line = self.last_tok.is_none_or(|l| l.line != t.line);
        self.kd = match std::mem::replace(&mut self.kd, Kd::Idle) {
            Kd::Idle => Kd::Idle,
            Kd::Kw { start, pd: kpd, sl, kw, mut name, mut angle, deflt, arrow } => match t.k {
                Tk::Ident | Tk::Ty if angle == 0 && pd == kpd => {
                    name = Some(Name::of(t, idx));
                    Kd::Kw { start, pd: kpd, sl, kw, name, angle, deflt, arrow }
                }
                Tk::Ident | Tk::Ty | Tk::Kw => Kd::Kw { start, pd: kpd, sl, kw, name, angle, deflt, arrow },
                Tk::P(b'<') => {
                    angle += 1;
                    Kd::Kw { start, pd: kpd, sl, kw, name, angle, deflt, arrow }
                }
                Tk::P(b'>') => {
                    angle = angle.saturating_sub(1);
                    Kd::Kw { start, pd: kpd, sl, kw, name, angle, deflt, arrow }
                }
                Tk::P(b'(') if pd == kpd && angle == 0 => {
                    if let Some(n) = name {
                        Kd::Sig { def: Def { name: n, fixed: None, tilde: false, start }, pd: kpd, sl, line: t.line }
                    } else if self.cfg.go_recv {
                        Kd::Recv { start, pd: kpd, sl }
                    } else if deflt {
                        Kd::Sig { def: Def { name: kw, fixed: Some("default"), tilde: false, start }, pd: kpd, sl, line: t.line }
                    } else if let Some((n, s)) = arrow {
                        self.ar = Ar::None;
                        Kd::Sig { def: Def { name: n, fixed: None, tilde: false, start: s }, pd: kpd, sl, line: t.line }
                    } else {
                        Kd::Idle
                    }
                }
                Tk::P(b'.' | b'*' | b'&' | b'?' | b':' | b',' | b'[' | b']' | b'(' | b')') => {
                    Kd::Kw { start, pd: kpd, sl, kw, name, angle, deflt, arrow }
                }
                _ if angle > 0 => Kd::Kw { start, pd: kpd, sl, kw, name, angle, deflt, arrow },
                _ => Kd::Idle,
            },
            Kd::Recv { start, pd: kpd, sl } => {
                if t.k == Tk::P(b')') && pd == kpd + 1 {
                    Kd::AfterRecv { start, pd: kpd, sl, name: None }
                } else {
                    Kd::Recv { start, pd: kpd, sl }
                }
            }
            Kd::AfterRecv { start, pd: kpd, sl, name } => match t.k {
                Tk::Ident | Tk::Ty if pd == kpd => Kd::AfterRecv { start, pd: kpd, sl, name: Some(Name::of(t, idx)) },
                Tk::P(b'(') if pd == kpd => match name {
                    Some(n) => Kd::Sig { def: Def { name: n, fixed: None, tilde: false, start }, pd: kpd, sl, line: t.line },
                    None => Kd::Idle,
                },
                Tk::P(b'[' | b']') => Kd::AfterRecv { start, pd: kpd, sl, name },
                _ if pd > kpd => Kd::AfterRecv { start, pd: kpd, sl, name },
                _ => Kd::Idle,
            },
            Kd::Sig { def, pd: spd, sl, line } => {
                let cont = matches!(t.k, Tk::P(b'{' | b'-' | b':' | b'=' | b'.' | b',' | b'>' | b'|'))
                    || (t.k == Tk::Kw && matches!(text, "where" | "throws" | "rethrows" | "async"));
                let ends = (t.k == Tk::P(b';') && pd == spd) || (t.k == Tk::P(b'}') && pd <= spd);
                // Newline-terminated languages: a new line that does not continue the signature ends it.
                let asi_end = self.cfg.asi && first_on_line && t.line > line && pd == spd && !cont;
                if ends || asi_end {
                    Kd::Idle
                } else {
                    Kd::Sig { def, pd: spd, sl, line }
                }
            }
        };
    }

    fn ar_step(&mut self, t: &Tok) {
        let pd = self.pd;
        self.ar = match std::mem::replace(&mut self.ar, Ar::None) {
            Ar::None => Ar::None,
            Ar::Named { name, start, pd: apd } => {
                if pd != apd {
                    Ar::Named { name, start, pd: apd }
                } else {
                    match t.k {
                        Tk::P(b'(') => Ar::Open { name, start, pd: apd },
                        Tk::Ident | Tk::Ty => Ar::Named { name, start, pd: apd },
                        Tk::P(b'.' | b'<' | b'>' | b'|' | b'&' | b'[' | b']' | b':' | b'=') => Ar::Named { name, start, pd: apd },
                        Tk::Kw => Ar::Named { name, start, pd: apd },
                        _ => Ar::None,
                    }
                }
            }
            Ar::Open { name, start, pd: apd } => {
                if t.k == Tk::P(b')') && pd == apd + 1 {
                    Ar::Named { name, start, pd: apd }
                } else {
                    Ar::Open { name, start, pd: apd }
                }
            }
            b @ Ar::Body { .. } => b,
        };
    }

    fn on_open_paren(&mut self, f: &Feed, idx: usize) {
        let ce = f.callee(idx);
        let pd = self.pd;
        let attr = f.prev(idx).is_some_and(|p| p.k == Tk::Attr);
        let mut lite = Lite { name: None, bare: false, is_new: false, attr };
        if let Some(c) = &ce {
            let name = f.text.get(c.name_s..c.name_e).unwrap_or("");
            lite = Lite { name: f.get(c.name_idx).map(|nt| Name::of(nt, c.name_idx)), bare: c.bare, is_new: c.is_new, attr };
            let is_def = matches!(self.kd, Kd::Sig { def, .. } if def.name.idx == c.name_idx && def.fixed.is_none());
            let after_kw = c.name_idx.checked_sub(1).is_some_and(|p| {
                f.get(p).is_some_and(|pt| pt.k == Tk::Kw && matches!(f.text(pt), "fn" | "func" | "function" | "fun" | "class" | "struct"))
            });
            if self.out.in_fn() && !is_def && !after_kw && !self.cfg.ignore.contains(&name) {
                self.out.call(name, &c.qualified, c.line, c.col);
            }
            if self.cfg.c_style && self.decl_level() && pd == self.base() && !self.head.dead && !C_SKIP.contains(&name) {
                let tilde = c.name_idx.checked_sub(1).is_some_and(|p| f.is_p(p, b'~'));
                let replace = match self.head.cand {
                    None => !self.head.saw_eq,
                    Some(old) => {
                        let old_text = f.text.get(old.s..old.e).unwrap_or("");
                        self.head.closed && !self.head.after_qual && (is_all_caps(old_text.as_bytes()) || C_SKIP.contains(&old_text))
                    }
                };
                if replace {
                    self.head.cand = lite.name;
                    self.head.tilde = tilde;
                    self.head.closed = false;
                }
            }
        }
        if self.pstack.len() < MAX_PARENS {
            self.pstack.push(lite);
        } else {
            self.povf += 1;
        }
        self.pd = self.pd.saturating_add(1);
    }

    fn on_close_paren(&mut self, idx: usize, t: &Tok) {
        while self.top_is_expr() && self.scopes.last().is_some_and(|s| self.pd <= s.pd) {
            self.close_scope(t.line);
        }
        self.pd = self.pd.saturating_sub(1).max(self.base());
        if t.k != Tk::P(b')') {
            return;
        }
        let lite = if self.povf > 0 {
            self.povf -= 1;
            None
        } else {
            self.pstack.pop()
        };
        let Some(lite) = lite else { return };
        if self.pd == self.base() {
            self.last_close = Some(Close { lite, idx, pd: self.pd });
        }
        if let (Some(c), Some(n)) = (self.head.cand, lite.name) {
            if c.idx == n.idx {
                self.head.closed = true;
            }
        }
        self.last_attr = lite.attr;
    }

    fn on_lbrace(&mut self, f: &Feed, idx: usize) {
        let pd = self.pd;
        let base = self.base();
        if self.scopes.len() >= MAX_SCOPES {
            self.brace_ovf += 1;
            self.reset_states();
            return;
        }
        if let Ar::Body { name, start } = self.ar {
            self.ar = Ar::None;
            self.reset_head();
            self.open(f, Def { name, fixed: None, tilde: false, start }, Sk::Fn);
            return;
        }
        if let Kd::Sig { def, pd: spd, .. } = self.kd {
            if spd == pd {
                self.kd = Kd::Idle;
                self.reset_head();
                self.open(f, def, Sk::Fn);
                return;
            }
        }
        if matches!(self.kd, Kd::Kw { pd: kpd, .. } | Kd::Recv { pd: kpd, .. } | Kd::AfterRecv { pd: kpd, .. } if kpd == pd) {
            self.kd = Kd::Idle;
        }
        if self.cfg.c_style && self.decl_level() && pd == base && self.head.closed && !self.head.dead {
            if let Some(c) = self.head.cand {
                let def = Def { name: c, fixed: None, tilde: self.head.tilde, start: self.head.start.unwrap_or(c.line) };
                self.out.uncall(c.line, c.col);
                self.reset_head();
                self.open(f, def, Sk::Fn);
                return;
            }
        }
        let lc = self.last_close.take();
        if self.cfg.shorthand && pd == base && self.clean {
            if let Some(lc) = &lc {
                if lc.pd == pd && lc.lite.bare && !lc.lite.is_new && !lc.lite.attr {
                    if let Some(n) = lc.lite.name {
                        let start = self.head.start.unwrap_or(n.line);
                        self.out.uncall(n.line, n.col);
                        self.reset_head();
                        self.open(f, Def { name: n, fixed: None, tilde: false, start }, Sk::Fn);
                        return;
                    }
                }
            }
        }
        let anon_class = lc.is_some_and(|c| c.lite.is_new && c.idx + 1 == idx);
        let k = if anon_class || (pd == base && self.head.container && !self.head.saw_eq) { Sk::Container } else { Sk::Block };
        self.scopes.push(Scope { k, pd, opened: false });
        if pd == base {
            self.reset_head();
        }
        self.ar = Ar::None;
    }

    fn on_rbrace(&mut self, line: u32) {
        while self.top_is_expr() {
            self.close_scope(line);
        }
        if self.brace_ovf > 0 {
            self.brace_ovf -= 1;
            return;
        }
        let sl = self.scopes.len();
        let Some(s) = self.scopes.last().copied() else {
            self.pd = 0;
            self.reset_states();
            return;
        };
        self.close_scope(line);
        let inner = s.pd > self.base();
        if !inner {
            self.reset_head();
        }
        let drop_kd = match self.kd {
            Kd::Kw { sl: k, .. } | Kd::Recv { sl: k, .. } | Kd::AfterRecv { sl: k, .. } | Kd::Sig { sl: k, .. } => k >= sl,
            Kd::Idle => false,
        };
        if drop_kd {
            self.kd = Kd::Idle;
        }
        self.ar = Ar::None;
        self.eq = None;
        self.last_close = None;
    }
}

impl Engine for Eng<'_> {
    fn line(&mut self, _f: &Feed, no: u32, text: &str, normal: bool) -> bool {
        if self.cfg.preproc {
            if self.pp_cont {
                self.pp_cont = text.trim_end().ends_with('\\');
                return false;
            }
            if normal {
                if let Some(rest) = text.trim_start().strip_prefix('#') {
                    self.directive(rest.trim_start(), no);
                    self.pp_cont = text.trim_end().ends_with('\\');
                    return false;
                }
            }
            if self.skipping() {
                return false;
            }
        }
        if self.cfg.asi
            && self.pd == self.base()
            && matches!(self.kd, Kd::Idle)
            && matches!(self.ar, Ar::None)
            && self.head.cand.is_none()
            && !self.last_attr
            && self.eq.is_none()
        {
            self.head.start = None;
        }
        true
    }

    fn tok(&mut self, f: &Feed, idx: usize) {
        let Some(&t) = f.get(idx) else { return };
        if let Some(eq) = self.eq.take() {
            let adjacent = f.get(eq).is_some_and(|e| e.e == t.s);
            if adjacent && t.k == Tk::P(b'>') {
                self.on_fat_arrow(f);
                self.after(&t, idx);
                return;
            }
            if !(adjacent && t.k == Tk::P(b'=')) {
                self.on_assign(f, eq);
            }
        }
        if let Ar::Body { name, start } = self.ar {
            if t.k != Tk::P(b'{') {
                self.ar = Ar::None;
                self.open(f, Def { name, fixed: None, tilde: false, start }, Sk::Expr);
            }
        }
        if self.head.start.is_none() {
            self.head.start = Some(t.line);
        }
        self.kd_step(f, idx, &t);
        self.ar_step(&t);
        let text = f.text(&t);
        match t.k {
            Tk::P(b'(') => self.on_open_paren(f, idx),
            Tk::P(b'[') => self.pd = self.pd.saturating_add(1),
            Tk::P(b')' | b']') => self.on_close_paren(idx, &t),
            Tk::P(b'{') => self.on_lbrace(f, idx),
            Tk::P(b'}') => self.on_rbrace(t.line),
            Tk::P(b';') => {
                while self.top_is_expr() && self.scopes.last().is_some_and(|s| s.pd == self.pd) {
                    self.close_scope(t.line);
                }
                if self.pd == self.base() {
                    self.reset_head();
                    self.ar = Ar::None;
                    self.last_close = None;
                }
            }
            Tk::P(b',') => {
                if self.top_is_expr() && self.scopes.last().is_some_and(|s| s.pd == self.pd) {
                    self.close_scope(t.line);
                }
                if matches!(self.ar, Ar::Named { pd, .. } if pd == self.pd) {
                    self.ar = Ar::None;
                }
            }
            Tk::P(b'=') => {
                let compound = f.prev(idx).is_some_and(|p| p.e == t.s && matches!(p.k, Tk::P(c) if is_op_char(c)));
                if !compound {
                    self.eq = Some(idx);
                }
            }
            Tk::P(b':') => {
                if self.pd == self.base() && self.head.closed {
                    self.head.after_qual = true;
                }
                if self.cfg.arrow && matches!(self.ar, Ar::None) {
                    if let Some(p) = f.prev(idx).copied().filter(|p| matches!(p.k, Tk::Ident | Tk::Ty)) {
                        let start = self.head.start.unwrap_or(p.line);
                        self.ar = Ar::Named { name: Name::of(&p, idx - 1), start, pd: self.pd };
                        self.ar_colon = true;
                    }
                }
            }
            Tk::Kw => {
                if self.pd == self.base() && self.cfg.containers.contains(&text) {
                    self.head.container = true;
                }
                if !matches!(text, "async" | "function") && matches!(self.ar, Ar::Named { pd, .. } if pd == self.pd) {
                    self.ar = Ar::None;
                }
            }
            _ => {}
        }
        self.after(&t, idx);
    }

    fn eol(&mut self, _f: &Feed, no: u32) {
        if !self.cfg.asi {
            return;
        }
        let Some(last) = self.last_tok.filter(|l| l.line == no) else { return };
        let cont = matches!(last.k, Tk::P(c) if is_op_char(c) || c == b'(' || c == b',');
        if !cont && self.top_is_expr() && self.scopes.last().is_some_and(|s| s.pd == self.pd) && self.eq.is_none() {
            self.close_scope(no);
        }
        if !cont && matches!(self.ar, Ar::Named { pd, .. } if pd == self.pd) && self.eq.is_none() {
            self.ar = Ar::None;
        }
    }
}

impl Eng<'_> {
    fn after(&mut self, t: &Tok, idx: usize) {
        let is_close = self.last_close.as_ref().is_some_and(|c| c.idx == idx);
        if is_close {
            self.clean = true;
        } else if self.pd == self.base() || matches!(t.k, Tk::P(b')' | b']')) {
            let ok = matches!(
                t.k,
                Tk::Ident | Tk::Ty | Tk::Kw | Tk::Str | Tk::Lit | Tk::P(b':' | b'<' | b'>' | b'|' | b'&' | b'[' | b']' | b'.' | b',' | b'?')
            );
            if !ok {
                self.clean = false;
            }
        }
        if t.k != Tk::P(b')') {
            self.last_attr = t.k == Tk::Attr;
        }
        self.last_tok = Some(*t);
    }
}
