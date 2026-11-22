//! Shell: `name() { … }` and `function name [()] { … }`. Calls: command words inside a body
//! (the lexer marks them as `Function`), qualified always empty.

use super::toks::{CalleeOpts, Engine, Feed, Tk};
use super::Out;
use crate::{FnDef, Lang};

const MAX_BLOCKS: usize = 1024;

/// Definition head in progress.
#[derive(Clone, Copy)]
enum Head {
    None,
    /// After `function`.
    Kw,
    /// Name seen; `stage` 0 = right after the name, 1 = after `(`, 2 = after `()` (waits for `{`).
    Name { idx: usize, start: u32, stage: u8 },
}

struct Sh {
    out: Out,
    /// `true`: a function body.
    blocks: Vec<bool>,
    ovf: u32,
    head: Head,
}

pub(crate) fn run(text: &str) -> Vec<FnDef> {
    let mut e = Sh { out: Out::new(Lang::Shell), blocks: Vec::new(), ovf: 0, head: Head::None };
    let last = super::toks::run(Lang::Shell, text, CalleeOpts::default(), &mut e);
    e.out.finish(last)
}

impl Engine for Sh {
    fn tok(&mut self, f: &Feed, idx: usize) {
        let Some(&t) = f.get(idx) else { return };
        let text = f.text(&t);
        self.head = match (self.head, t.k) {
            (_, Tk::Kw) if text == "function" => Head::Kw,
            (Head::Kw, Tk::Ident) => Head::Name { idx, start: t.line, stage: 2 },
            (Head::Name { idx: n, start, stage: 0 | 2 }, Tk::P(b'(')) => Head::Name { idx: n, start, stage: 1 },
            (Head::Name { idx: n, start, stage: 1 }, Tk::P(b')')) => Head::Name { idx: n, start, stage: 2 },
            (Head::Name { stage: 2, .. }, Tk::P(b'{')) => self.head,
            (_, Tk::Ident) => {
                if self.out.in_fn() {
                    self.out.call(text, "", t.line, t.col);
                }
                Head::Name { idx, start: t.line, stage: 0 }
            }
            _ => Head::None,
        };
        match t.k {
            Tk::P(b'{') => {
                let head = std::mem::replace(&mut self.head, Head::None);
                if self.blocks.len() >= MAX_BLOCKS {
                    self.ovf += 1;
                    return;
                }
                let def = match head {
                    Head::Name { idx: n, start, stage: 2 } => f.get(n).copied().map(|nt| (nt, start)),
                    _ => None,
                };
                match def {
                    Some((nt, start)) => {
                        self.out.uncall(nt.line, nt.col);
                        let opened = self.out.open_fn(f.text(&nt), nt.line, nt.col, start);
                        self.blocks.push(opened);
                    }
                    None => self.blocks.push(false),
                }
            }
            Tk::P(b'}') => {
                if self.ovf > 0 {
                    self.ovf -= 1;
                } else if let Some(true) = self.blocks.pop() {
                    self.out.close_fn(t.line);
                }
            }
            _ => {}
        }
    }
}
