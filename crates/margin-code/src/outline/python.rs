//! Python: `def` blocks by indentation.
//!
//! | Rule | Detail |
//! |---|---|
//! | definition | `def name` / `async def name` first on a logical line |
//! | extent | until the next logical line indented ≤ the `def` (blank and comment lines don't count) |
//! | `start` | first decorator line when `@…` lines precede the `def` |
//! | logical line | continues inside `( [ {`, after a trailing `\`, inside triple-quoted strings |
//! | calls | `name(` inside any open def (nested defs: counted for every enclosing def) |

use super::toks::{self, CalleeOpts, Engine, Feed, Tk};
use super::Out;
use crate::{FnDef, Lang};

const MAX_STACK: usize = 64;

struct Py {
    out: Out,
    /// Indent of each open def (or of a def past the caps, `opened = false`).
    stack: Vec<(usize, bool)>,
    deco_start: Option<u32>,
    last_code_line: u32,
    bracket: u32,
    cont: bool,
    first_tok: bool,
    indent: usize,
    after_def: Option<u32>,
    def_name_idx: usize,
}

pub(crate) fn run(text: &str) -> Vec<FnDef> {
    let mut e = Py {
        out: Out::new(Lang::Python),
        stack: Vec::new(),
        deco_start: None,
        last_code_line: 0,
        bracket: 0,
        cont: false,
        first_tok: false,
        indent: 0,
        after_def: None,
        def_name_idx: toks::NONE,
    };
    let last = toks::run(Lang::Python, text, CalleeOpts::default(), &mut e);
    let end = e.last_code_line.min(last);
    while let Some((_, opened)) = e.stack.pop() {
        if opened {
            e.out.close_fn(end);
        }
    }
    e.out.finish(end)
}

fn indent_of(text: &str) -> usize {
    let mut n = 0usize;
    for c in text.bytes() {
        match c {
            b' ' => n += 1,
            b'\t' => n = (n / 8 + 1) * 8,
            0x0c => n = 0,
            _ => break,
        }
    }
    n
}

impl Engine for Py {
    fn line(&mut self, _f: &Feed, no: u32, text: &str, normal: bool) -> bool {
        let t = text.trim();
        let blank = t.is_empty() || (normal && t.starts_with('#'));
        let logical = normal && self.bracket == 0 && !self.cont;
        if logical && !blank {
            let indent = indent_of(text);
            while let Some(&(ind, opened)) = self.stack.last() {
                if ind < indent {
                    break;
                }
                self.stack.pop();
                if opened {
                    self.out.close_fn(self.last_code_line);
                }
            }
            self.first_tok = true;
            self.indent = indent;
            self.after_def = None;
        }
        if !blank {
            self.last_code_line = no;
        }
        // A trailing `\` outside a comment continues the logical line.
        self.cont = !t.contains('#') && t.ends_with('\\');
        true
    }

    fn tok(&mut self, f: &Feed, idx: usize) {
        let Some(&t) = f.get(idx) else { return };
        let text = f.text(&t);
        if self.first_tok {
            self.first_tok = false;
            match t.k {
                Tk::Attr => {
                    self.deco_start.get_or_insert(t.line);
                }
                Tk::Kw if text == "async" => self.first_tok = true,
                Tk::Kw if text == "def" => self.after_def = Some(self.deco_start.take().unwrap_or(t.line)),
                _ => self.deco_start = None,
            }
        } else if let Some(start) = self.after_def.take() {
            if t.k == Tk::Ident && self.stack.len() < MAX_STACK && self.out.depth() < super::MAX_FN_DEPTH {
                let opened = self.out.open_fn(text, t.line, t.col, start);
                self.stack.push((self.indent, opened));
                self.def_name_idx = idx;
            }
        }
        match t.k {
            Tk::P(b'(' | b'[' | b'{') => {
                self.bracket = self.bracket.saturating_add(1);
                if t.k == Tk::P(b'(') && self.out.in_fn() {
                    if let Some(c) = f.callee(idx) {
                        let after_kw = c.name_idx.checked_sub(1).is_some_and(|p| f.is_kw(p, "def") || f.is_kw(p, "class"));
                        if c.name_idx != self.def_name_idx && !after_kw {
                            let name = f.text.get(c.name_s..c.name_e).unwrap_or("");
                            self.out.call(name, &c.qualified, c.line, c.col);
                        }
                    }
                }
            }
            Tk::P(b')' | b']' | b'}') => self.bracket = self.bracket.saturating_sub(1),
            _ => {}
        }
    }
}
