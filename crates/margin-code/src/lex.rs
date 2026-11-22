//! Line lexer for syntax colouring. STUB: returns no tokens.

use crate::Lang;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// Identifiers and anything unclassified. Lexers may leave such ranges out entirely.
    Text,
    Keyword,
    /// Built-in and user type names where cheap to tell (`int`, `String`, capitalised names in Rust/Java…).
    Type,
    /// A name directly followed by `(` (call or definition).
    Function,
    String,
    /// Escape sequence inside a string.
    Escape,
    Number,
    Comment,
    Operator,
    Punct,
    /// `true`, `false`, `null`, `nil`, `None`, ALL_CAPS constants.
    Constant,
    /// `#[attr]`, `@decorator`, preprocessor directives, Rust macros `name!`.
    Attribute,
    /// Markdown heading line; YAML/TOML section header.
    Heading,
    /// Markdown `*emphasis*`.
    Emphasis,
    /// Markdown `**strong**`.
    Strong,
    /// Markdown link text / URL.
    Link,
    /// Markdown inline code and fenced code blocks.
    CodeSpan,
    /// HTML/XML tag name.
    Tag,
    /// Key of a key/value pair (JSON, TOML, YAML, HTML attribute, CSS property).
    Key,
    /// Markdown list bullet / quote marker; diff `+` line uses `Inserted`, `-` line `Deleted`.
    Marker,
    Inserted,
    Deleted,
}

/// Byte range `start..end` in the line, on char boundaries, sorted, non-overlapping. Gaps are plain text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub start: u32,
    pub end: u32,
    pub kind: TokenKind,
}

/// Lexer state at the start of a line (inside a block comment, a multi-line string, a code fence…).
/// Opaque. `LineState::default()` is the state at the start of a file.
/// Equal states + equal line text ⇒ equal tokens and equal next state (the editor relies on it to stop re-lexing).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct LineState(pub u64);

/// Tokens of `line` (no line break inside) appended to `out`; returns the state for the next line.
/// Lines longer than a cap (the implementer picks one, ≥ 4096 bytes) may be left partly untokenised.
pub fn lex_line(lang: Lang, state: LineState, line: &str, out: &mut Vec<Token>) -> LineState {
    let _ = (lang, line, out);
    state
}
