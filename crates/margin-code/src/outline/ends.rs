//! Ruby and Lua: blocks closed by `end`.
//!
//! | Lang | Definitions | Block openers (closed by `end`) | Calls |
//! |---|---|---|---|
//! | Ruby | `def name`, `def self.name`, `def x=`, `def ==`; endless `def f(x) = …` ends on its line | `def class module case begin do`; `if unless while until for` only at statement start or after `= ( return` | `name(`, and `name arg` / `a.b arg` at statement start |
//! | Lua | `function a.b:c(`, `local function f(`, `f = function(` | `function if do repeat` (`repeat` closed by `until`) | `name(`, `obj:m(`, `name "str"` |

use super::toks::{self, CalleeOpts, Engine, Feed, Tk, Tok};
use super::Out;
use crate::{FnDef, Lang};

const MAX_BLOCKS: usize = 1024;

#[derive(Clone, Copy)]
struct Blk {
    def: bool,
    opened: bool,
}

/// Shared block stack.
struct Blocks {
    v: Vec<Blk>,
    ovf: u32,
}

impl Blocks {
    fn push(&mut self, b: Blk) -> bool {
        if self.v.len() < MAX_BLOCKS {
            self.v.push(b);
            true
        } else {
            self.ovf += 1;
            false
        }
    }
    fn pop(&mut self, out: &mut Out, line: u32) {
        if self.ovf > 0 {
            self.ovf -= 1;
            return;
        }
        if let Some(b) = self.v.pop() {
            if b.def && b.opened {
                out.close_fn(line);
            }
        }
    }
}

// ---------------- Ruby ----------------

struct Rb {
    out: Out,
    blocks: Blocks,
    stmt_start: bool,
    loop_line: Option<u32>,
    /// Endless def opened on this line: close it at end of line.
    endless: Option<u32>,
    /// Line and column of the latest `def` name (not a call).
    def_name_pos: Option<(u32, u32)>,
    /// Statement-start call chain `a.b arg`: index of the last name in it.
    chain: Option<usize>,
    line_text: String,
}

pub(crate) fn ruby(text: &str) -> Vec<FnDef> {
    let mut e = Rb {
        out: Out::new(Lang::Ruby),
        blocks: Blocks { v: Vec::new(), ovf: 0 },
        stmt_start: true,
        loop_line: None,
        endless: None,
        def_name_pos: None,
        chain: None,
        line_text: String::new(),
    };
    let last = toks::run(Lang::Ruby, text, CalleeOpts { dcolon: true, ..CalleeOpts::default() }, &mut e);
    e.out.finish(last)
}

/// Keywords after which `if`/`while`… open a block (value position).
const RB_VALUE_KW: &[&str] = &["and", "else", "not", "or", "return", "then"];

impl Rb {
    /// Method name after `def`, from byte `from` of the current line: (name, column, end of the name).
    fn def_name(&self, from: usize) -> Option<(String, usize, usize)> {
        let rest = self.line_text.get(from..)?;
        let lead = rest.len() - rest.trim_start().len();
        let b = rest.trim_start().as_bytes();
        let mut len = 0;
        while let Some(&c) = b.get(len) {
            if c == b'(' || c == b';' || c.is_ascii_whitespace() {
                break;
            }
            // `def x=(v)`: a setter keeps its `=`.
            if c == b'=' && len > 0 && b.get(len - 1).is_some_and(|&p| crate::lex::is_ident_char(p)) {
                len += 1;
                break;
            }
            len += 1;
        }
        let full = rest.trim_start().get(..len)?;
        let dot = full.rfind('.').map_or(0, |p| p + 1);
        let name = full.get(dot..).filter(|n| !n.is_empty())?;
        Some((name.to_owned(), from + lead + dot, from + lead + len))
    }

    /// Endless def: `= expr` after the name and its parameter list.
    fn is_endless(&self, after_name: usize) -> bool {
        let Some(rest) = self.line_text.get(after_name..) else { return false };
        let mut r = rest.trim_start();
        if r.starts_with('(') {
            let mut depth = 0u32;
            let mut end = None;
            for (k, c) in r.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            end = Some(k + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            r = match end {
                Some(e) => r.get(e..).unwrap_or("").trim_start(),
                None => return false,
            };
        }
        r.starts_with('=') && !r.starts_with("==") && !r.starts_with("=~")
    }

    fn chain_step(&mut self, f: &Feed, idx: usize, t: &Tok, at_start: bool) {
        let text = f.text(t);
        let Some(ni) = self.chain else {
            if at_start && (t.k == Tk::Ident || (t.k == Tk::Kw && text == "self")) {
                self.chain = Some(idx);
            }
            return;
        };
        let prev_sep = f.prev(idx).is_some_and(|p| matches!(p.k, Tk::P(b'.' | b':')));
        let spaced = f.get(ni).is_some_and(|n| n.e < t.s) && idx == ni + 1;
        match t.k {
            Tk::P(b'.' | b':') => {}
            Tk::Ident | Tk::Ty if prev_sep => self.chain = Some(idx),
            Tk::Str | Tk::Lit | Tk::Ident | Tk::Ty | Tk::P(b'@' | b'$') if spaced => self.flush_chain(f),
            Tk::Kw if spaced && matches!(text, "self" | "nil" | "true" | "false") => self.flush_chain(f),
            _ => self.chain = None,
        }
    }

    fn flush_chain(&mut self, f: &Feed) {
        let Some(ni) = self.chain.take() else { return };
        if !self.out.in_fn() {
            return;
        }
        if let Some(nt) = f.get(ni).copied() {
            if nt.k == Tk::Kw {
                return;
            }
            let name = f.text(&nt);
            let (q, _, _) = f.qualify(ni, name);
            self.out.call(name, &q, nt.line, nt.col);
        }
    }
}

impl Engine for Rb {
    fn line(&mut self, _f: &Feed, _no: u32, text: &str, _normal: bool) -> bool {
        self.stmt_start = true;
        self.chain = None;
        self.line_text.clear();
        self.line_text.push_str(text);
        true
    }

    fn tok(&mut self, f: &Feed, idx: usize) {
        let Some(&t) = f.get(idx) else { return };
        let line_off = t.s.saturating_sub(t.col as usize);
        let at_start = std::mem::replace(&mut self.stmt_start, false);
        let text = f.text(&t);
        let prev = f.prev(idx).copied();
        let dotted = prev.is_some_and(|p| p.k == Tk::P(b'.') || (p.k == Tk::P(b':') && p.e == t.s));
        self.chain_step(f, idx, &t, at_start);
        match t.k {
            Tk::Kw if !dotted => match text {
                "def" => {
                    let from = t.e.saturating_sub(line_off);
                    if let Some((name, col, name_end)) = self.def_name(from) {
                        let col = u32::try_from(col).unwrap_or(u32::MAX);
                        let opened = self.out.open_fn(&name, t.line, col, t.line);
                        self.def_name_pos = Some((t.line, col));
                        if self.is_endless(name_end) {
                            if opened {
                                self.endless = Some(t.line);
                            }
                        } else if !self.blocks.push(Blk { def: true, opened }) && opened {
                            self.out.close_fn(t.line);
                        }
                    }
                }
                "class" | "module" | "case" | "begin" => {
                    self.blocks.push(Blk { def: false, opened: false });
                    self.stmt_start = text == "begin";
                }
                "if" | "unless" | "while" | "until" | "for" => {
                    let value_pos = at_start
                        || prev.is_some_and(|p| {
                            matches!(p.k, Tk::P(b'=' | b'(' | b'|' | b'&' | b','))
                                || (p.k == Tk::Kw && RB_VALUE_KW.contains(&f.text(&p)))
                        });
                    if value_pos {
                        self.blocks.push(Blk { def: false, opened: false });
                        if matches!(text, "while" | "until" | "for") {
                            self.loop_line = Some(t.line);
                        }
                    }
                }
                "do" => {
                    if self.loop_line == Some(t.line) {
                        self.loop_line = None;
                    } else {
                        self.blocks.push(Blk { def: false, opened: false });
                    }
                }
                "end" => self.blocks.pop(&mut self.out, t.line),
                "then" | "else" | "ensure" => self.stmt_start = true,
                _ => {}
            },
            Tk::P(b';') => self.stmt_start = true,
            Tk::P(b'(') => {
                if let Some(c) = f.callee(idx) {
                    if self.chain == Some(c.name_idx) {
                        self.chain = None;
                    }
                    if self.out.in_fn() && self.def_name_pos != Some((c.line, c.col)) {
                        let name = f.text.get(c.name_s..c.name_e).unwrap_or("");
                        self.out.call(name, &c.qualified, c.line, c.col);
                    }
                }
            }
            _ => {}
        }
    }

    fn eol(&mut self, _f: &Feed, no: u32) {
        self.chain = None;
        if self.endless.take().is_some() {
            self.out.close_fn(no);
        }
    }
}

// ---------------- Lua ----------------

struct Lua {
    out: Out,
    blocks: Blocks,
    /// After `function`: (start line, keyword index, assigned name index).
    head: Option<(u32, usize, Option<usize>)>,
    def_name_idx: usize,
}

pub(crate) fn lua(text: &str) -> Vec<FnDef> {
    let mut e = Lua { out: Out::new(Lang::Lua), blocks: Blocks { v: Vec::new(), ovf: 0 }, head: None, def_name_idx: toks::NONE };
    let last = toks::run(Lang::Lua, text, CalleeOpts { colon: true, ..CalleeOpts::default() }, &mut e);
    e.out.finish(last)
}

impl Engine for Lua {
    fn tok(&mut self, f: &Feed, idx: usize) {
        let Some(&t) = f.get(idx) else { return };
        let text = f.text(&t);
        if let Some((start, kw, assigned)) = self.head {
            match t.k {
                Tk::Ident | Tk::P(b'.' | b':') => return,
                Tk::P(b'(') => {
                    self.head = None;
                    // Name: last ident before `(` when one follows `function`, else the assigned name.
                    let named = f.prev(idx).is_some_and(|p| p.k == Tk::Ident) && idx > kw + 1;
                    let ni = if named { Some(idx - 1) } else { assigned };
                    let opened = match ni.and_then(|k| f.get(k).copied()) {
                        Some(nt) => {
                            self.def_name_idx = if named { idx - 1 } else { toks::NONE };
                            let start = if named { start } else { nt.line.min(start) };
                            self.out.open_fn(f.text(&nt), nt.line, nt.col, start)
                        }
                        None => false,
                    };
                    let def = opened;
                    if !self.blocks.push(Blk { def, opened }) && opened {
                        self.out.close_fn(t.line);
                    }
                }
                _ => {
                    self.head = None;
                    self.blocks.push(Blk { def: false, opened: false });
                }
            }
            if t.k == Tk::P(b'(') {
                return;
            }
        }
        match t.k {
            Tk::Kw => match text {
                "function" => {
                    // `name = function(`: the assigned name.
                    let assigned = idx.checked_sub(2).filter(|&k| f.is_p(k + 1, b'=') && f.get(k).is_some_and(|nt| nt.k == Tk::Ident));
                    let start = idx.checked_sub(1).and_then(|k| f.get(k)).filter(|p| p.k == Tk::Kw && f.text(p) == "local").map_or(t.line, |p| p.line);
                    self.head = Some((start, idx, assigned));
                }
                "if" | "do" | "repeat" => {
                    self.blocks.push(Blk { def: false, opened: false });
                }
                "end" | "until" => self.blocks.pop(&mut self.out, t.line),
                _ => {}
            },
            Tk::P(b'(') if self.out.in_fn() => {
                if let Some(c) = f.callee(idx) {
                    if c.name_idx != self.def_name_idx {
                        let name = f.text.get(c.name_s..c.name_e).unwrap_or("");
                        self.out.call(name, &c.qualified, c.line, c.col);
                    }
                }
            }
            Tk::Str if self.out.in_fn() => {
                // `require "x"` call without parens.
                if let Some(p) = f.prev(idx).copied().filter(|p| p.k == Tk::Ident) {
                    let before = idx.checked_sub(2).and_then(|k| f.get(k));
                    if !before.is_some_and(|b| b.k == Tk::Kw && f.text(b) == "local") {
                        let name = f.text(&p);
                        let (q, _, _) = f.qualify(idx - 1, name);
                        self.out.call(name, &q, p.line, p.col);
                    }
                }
            }
            _ => {}
        }
    }
}
