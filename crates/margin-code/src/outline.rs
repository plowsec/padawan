//! Functions of a file and the calls they make. STUB: returns nothing.

use crate::{Lang, SinkKind};

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
    let _ = (lang, text);
    Outline::default()
}
