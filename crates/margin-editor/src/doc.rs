//! `Document` internals: edit groups, undo/redo, lexing. Everything that changes text goes through `raw`.

use std::time::Duration;

use margin_code::{lex_line, Token};

use crate::buffer::{Buffer, Edit};
use crate::undo::{Kind, Sel};
use crate::{Document, Pos};

impl Document {
    pub(crate) fn from_buffer(mut buf: Buffer, lang: margin_code::Lang) -> Document {
        let pristine = buf.original.take();
        Document {
            buf,
            lang,
            hist: Default::default(),
            lex: Default::default(),
            revision: 1,
            saved_id: 0,
            pristine,
            pristine_id: 0,
            read_only: false,
        }
    }

    /// True while the text equals what was loaded (lets `text()` return the exact original bytes).
    pub(crate) fn at_pristine(&self) -> bool {
        self.pristine.is_some() && self.hist.state_id() == self.pristine_id
    }

    pub(crate) fn clamp(&self, p: Pos) -> Pos {
        self.buf.clamp(p)
    }

    /// The primitive with bookkeeping (lexer cache, revision). `None` when read-only.
    pub(crate) fn raw(&mut self, e: &Edit) -> Option<Edit> {
        if self.read_only {
            return None;
        }
        let (inv, (first, removed, inserted)) = self.buf.apply(e);
        self.lex.on_edit(first, removed, inserted);
        self.revision += 1;
        Some(inv)
    }

    /// Run `f` as one undo group. `f` applies edits with `tx.edit(..)` and returns the selection after.
    /// Returns `Some(sel_after)` when something changed.
    pub(crate) fn group(&mut self, kind: Kind, time: f64, before: Sel, f: impl FnOnce(&mut Tx<'_>) -> Sel) -> Option<Sel> {
        if self.read_only {
            return None;
        }
        let mut tx = Tx { doc: self, inv: Vec::new() };
        let after = f(&mut tx);
        let inv = std::mem::take(&mut tx.inv);
        if inv.is_empty() {
            return None;
        }
        let saved = self.saved_id;
        self.hist.record(inv, kind, time, before, after, saved);
        Some(after)
    }

    /// Undo one group; returns the selection to restore.
    pub(crate) fn undo_group(&mut self) -> Option<Sel> {
        if self.read_only {
            return None;
        }
        let g = self.hist.pop_undo()?;
        let mut redo = Vec::with_capacity(g.edits.len());
        for e in g.edits.iter().rev() {
            if let Some(inv) = self.raw(e) {
                redo.push(inv);
            }
        }
        let sel = g.before;
        self.hist.push_redo(g, redo);
        Some(sel)
    }

    pub(crate) fn redo_group(&mut self) -> Option<Sel> {
        if self.read_only {
            return None;
        }
        let g = self.hist.pop_redo()?;
        let mut undo = Vec::with_capacity(g.edits.len());
        for e in g.edits.iter().rev() {
            if let Some(inv) = self.raw(e) {
                undo.push(inv);
            }
        }
        let sel = g.after;
        self.hist.push_undo(g, undo);
        Some(sel)
    }

    /// Lex until line `upto` has a start state or the budget is spent. True when done.
    pub(crate) fn lex_upto(&mut self, upto: u32, budget: Duration) -> bool {
        let lang = self.lang;
        let n = self.buf.lines.len();
        let lines = &self.buf.lines;
        let mut scratch = Vec::new();
        self.lex.advance(upto as usize + 1, n, budget, |s, i| {
            scratch.clear();
            lex_line(lang, s, lines.get(i).map_or("", String::as_str), &mut scratch)
        })
    }

    /// Tokens of `line` when its start state is known (else plain: returns false).
    pub(crate) fn tokens(&self, line: u32, out: &mut Vec<Token>) -> bool {
        out.clear();
        let Some(s) = self.lex.state(line as usize) else { return false };
        lex_line(self.lang, s, self.buf.line(line), out);
        // Defensive: keep only sorted, in-bounds, non-overlapping tokens on char boundaries.
        let l = self.buf.line(line);
        let mut end = 0u32;
        out.retain(|t| {
            let ok = t.start >= end && t.start < t.end && (t.end as usize) <= l.len() && l.is_char_boundary(t.start as usize) && l.is_char_boundary(t.end as usize);
            if ok {
                end = t.end;
            }
            ok
        });
        true
    }
}

/// Edit transaction inside `Document::group`.
pub(crate) struct Tx<'a> {
    pub doc: &'a mut Document,
    inv: Vec<Edit>,
}

impl Tx<'_> {
    /// Replace `a..b` with `text`; returns the end of the inserted text.
    pub fn edit(&mut self, a: Pos, b: Pos, text: &str) -> Pos {
        let e = Edit::new(a, b, text);
        match self.doc.raw(&e) {
            Some(inv) => {
                let end = inv.end;
                // Skip no-ops so they do not create undo steps.
                if !(inv.text.is_empty() && inv.start == inv.end) {
                    self.inv.push(inv);
                }
                end
            }
            None => a,
        }
    }

    pub fn line(&self, i: u32) -> &str {
        self.doc.buf.line(i)
    }
}
