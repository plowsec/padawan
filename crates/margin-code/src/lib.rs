//! Text → tokens and outlines. Pure, safe Rust, no dependency: this code reads hostile files.
//!
//! | Item | What |
//! |---|---|
//! | `Lang` | language of a file, from its name |
//! | `lex_line` | syntax tokens of one line, resumable (state carried line to line) |
//! | `outline` | functions of a file, with the calls each one makes and which calls are sinks |
//! | `sink_kind` | is a call a dangerous sink (tables per language in `sinks.rs`) |
//! | `Lang::ALL`, `LEX_LINE_CAP` | added: every variant; bytes of a line that get tokens |
//!
//! Rules for every function here: never panic, never recurse on input depth, linear time,
//! any byte sequence that is valid UTF-8 is acceptable input.
//!
//! PUBLIC API IS FROZEN (other crates compile against it). Add items if needed; do not change these.

mod lang;
mod lex;
mod outline;
mod sinks;

pub use lang::Lang;
pub use lex::{lex_line, LineState, Token, TokenKind, LEX_LINE_CAP};
pub use outline::{outline, Call, FnDef, Outline};
pub use sinks::{sink_kind, SinkKind};
