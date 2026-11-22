//! Functions of a file and the calls they make. Token-based: the lexer masks strings and comments, then a
//! per-family engine reads the token stream.
//!
//! | File | Langs | Extent from |
//! |---|---|---|
//! | `outline/braces.rs` | C, C++, Rust, Go, Java, JS, TS, C#, PHP, Kotlin, Swift | `{ }` (and `=> expr` / `= expr` bodies) |
//! | `outline/python.rs` | Python | indentation |
//! | `outline/ends.rs` | Ruby, Lua | `def`/`function` … `end` |
//! | `outline/shell.rs` | Shell | `name() { }` |
//! | `outline/toks.rs` | all | token feed, call-site resolution |
//!
//! Caps (hostile files): `MAX_FNS` functions per file, `MAX_CALLS` calls per function, `MAX_TOTAL_CALLS`
//! per file, names over `MAX_NAME` bytes skipped, `qualified` cut to `MAX_QUALIFIED` bytes, nesting of open
//! functions capped at `MAX_FN_DEPTH` (deeper definitions count as plain blocks).

use crate::{sink_kind, Lang, SinkKind};

mod braces;
mod ends;
mod python;
mod shell;
mod toks;

pub(crate) const MAX_FNS: usize = 20_000;
pub(crate) const MAX_CALLS: usize = 2_000;
pub(crate) const MAX_TOTAL_CALLS: usize = 400_000;
pub(crate) const MAX_NAME: usize = 256;
pub(crate) const MAX_QUALIFIED: usize = 128;
pub(crate) const MAX_FN_DEPTH: usize = 16;

/// A call `name(...)` inside a function body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// Last segment of the callee: `c` for `a.b.c(...)`, `new` for `Command::new(...)`.
    pub name: String,
    /// Dotted / `::` path as written, when there is one: `os.system`, `Command::new`. Else empty.
    pub qualified: String,
    /// 0-based line and byte column of `name`.
    pub line: u32,
    pub col: u32,
    /// Set when this call is a known dangerous sink for the language.
    pub sink: Option<SinkKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnDef {
    pub name: String,
    /// Line and byte column of the name in the definition (0-based). The hint line is drawn above `line`.
    pub line: u32,
    pub col: u32,
    /// First and last line of the whole definition, inclusive (signature to closing brace / last body line).
    pub start: u32,
    pub end: u32,
    /// Calls in the body, in source order. Nested functions/closures: their calls belong to the outer function too.
    pub calls: Vec<Call>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outline {
    /// In source order.
    pub fns: Vec<FnDef>,
}

/// Outline of a whole file. Token-based heuristics (no full parse): good on ordinary code, never panics on any input.
/// Empty for languages without `has_outline`.
pub fn outline(lang: Lang, text: &str) -> Outline {
    let fns = match lang {
        Lang::C
        | Lang::Cpp
        | Lang::Rust
        | Lang::Go
        | Lang::Java
        | Lang::JavaScript
        | Lang::TypeScript
        | Lang::CSharp
        | Lang::Php
        | Lang::Kotlin
        | Lang::Swift => braces::run(lang, text),
        Lang::Python => python::run(text),
        Lang::Ruby => ends::ruby(text),
        Lang::Lua => ends::lua(text),
        Lang::Shell => shell::run(text),
        _ => Vec::new(),
    };
    Outline { fns }
}

/// Collects definitions and calls; keeps the stack of open functions.
pub(crate) struct Out {
    lang: Lang,
    fns: Vec<FnDef>,
    /// Open functions, innermost last. `None`: past `MAX_FNS`, tracked but not recorded.
    open: Vec<Option<usize>>,
    total_calls: usize,
}

impl Out {
    pub(crate) fn new(lang: Lang) -> Out {
        Out { lang, fns: Vec::new(), open: Vec::new(), total_calls: 0 }
    }

    pub(crate) fn depth(&self) -> usize {
        self.open.len()
    }

    pub(crate) fn in_fn(&self) -> bool {
        !self.open.is_empty()
    }

    /// Opens a definition. `false`: nesting cap reached, nothing opened (treat the body as a block).
    pub(crate) fn open_fn(&mut self, name: &str, line: u32, col: u32, start: u32) -> bool {
        if self.open.len() >= MAX_FN_DEPTH {
            return false;
        }
        if name.is_empty() || name.len() > MAX_NAME || self.fns.len() >= MAX_FNS {
            self.open.push(None);
            return true;
        }
        self.fns.push(FnDef { name: name.to_owned(), line, col, start: start.min(line), end: line, calls: Vec::new() });
        self.open.push(Some(self.fns.len() - 1));
        true
    }

    /// Closes the innermost open definition at `end`.
    pub(crate) fn close_fn(&mut self, end: u32) {
        if let Some(Some(k)) = self.open.pop() {
            if let Some(f) = self.fns.get_mut(k) {
                f.end = end.max(f.line);
            }
        }
    }

    pub(crate) fn close_all(&mut self, end: u32) {
        while !self.open.is_empty() {
            self.close_fn(end);
        }
    }

    /// Records a call in every open function.
    pub(crate) fn call(&mut self, name: &str, qualified: &str, line: u32, col: u32) {
        if self.open.is_empty() || name.is_empty() || name.len() > MAX_NAME || self.total_calls >= MAX_TOTAL_CALLS {
            return;
        }
        let sink = sink_kind(self.lang, name, qualified);
        for k in self.open.iter().flatten() {
            if let Some(f) = self.fns.get_mut(*k) {
                if f.calls.len() < MAX_CALLS && self.total_calls < MAX_TOTAL_CALLS {
                    f.calls.push(Call { name: name.to_owned(), qualified: qualified.to_owned(), line, col, sink });
                    self.total_calls += 1;
                }
            }
        }
    }

    /// Takes back a call recorded at `line:col` (it turned out to be a definition).
    pub(crate) fn uncall(&mut self, line: u32, col: u32) {
        for k in self.open.iter().flatten() {
            if let Some(f) = self.fns.get_mut(*k) {
                let n = f.calls.len();
                let from = n.saturating_sub(8);
                if let Some(p) = f.calls.get(from..).and_then(|c| c.iter().rposition(|c| c.line == line && c.col == col)) {
                    f.calls.remove(from + p);
                    self.total_calls = self.total_calls.saturating_sub(1);
                }
            }
        }
    }

    pub(crate) fn finish(mut self, last_line: u32) -> Vec<FnDef> {
        self.close_all(last_line);
        self.fns
    }
}
