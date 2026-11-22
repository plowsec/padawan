//! One lexer for the C family, parameterised by a `Spec` per language.
//!
//! | Feature | Langs |
//! |---|---|
//! | `//`, `/* */` | all |
//! | nested `/* /* */ */` | Rust, Kotlin, Swift |
//! | `#` line comment | PHP |
//! | `#directive` at line start | C, C++, C# (`#include <x>` → string) |
//! | `#word` anywhere | Swift (`#if`, `#selector`) |
//! | `#[attr]` | Rust, PHP |
//! | `@Annotation` | Java, Kotlin, Swift, JS, TS |
//! | `'c'` char | C, C++, Java, C#, Go, Kotlin; Rust (vs `'lifetime`) |
//! | `'str'` | JS, TS, PHP |
//! | multi-line `"…"` | Rust, PHP (also `'…'`) |
//! | `` `tpl ${expr}` `` | JS, TS (expr brace depth carried across lines) |
//! | `` `raw` `` | Go |
//! | `"""…"""` | Java, Kotlin (raw), Swift, C# |
//! | `r#"…"#`, `b"…"`, `b'x'` | Rust |
//! | `R"d(…)d"`, `L"…"`, `u8"…"` | C, C++ (raw: C++) |
//! | `@"…"`, `$"…"`, `$@"…"` | C# |
//! | `/regex/` | JS, TS (after an operator or keyword) |
//! | `name!` macro | Rust |
//! | Capitalised → `Type` | Rust, Go, Java, C#, Kotlin, Swift, TS |
//!
//! Modes: 1 block comment (param depth), 2 `"` string, 3 `'` string, 4 Rust raw (param hashes),
//! 5 template text, 6 template expr (param brace depth), 7 Go raw, 8 triple quote (param quote char),
//! 9 C++ raw (param delim len, extra delim), 10 C# verbatim.

use super::{
    delim_matches, escape_end, extra, in_table, is_all_caps, is_ident_start, mode, next_char, number_end,
    pack_delim, param, skip_ws, st, string_body, word_end, Em, LineState, TokenKind as K,
};

const M_BLOCK: u8 = 1;
const M_DQ: u8 = 2;
const M_SQ: u8 = 3;
const M_RAW: u8 = 4;
const M_TMPL: u8 = 5;
const M_TEXPR: u8 = 6;
const M_GORAW: u8 = 7;
const M_TRIPLE: u8 = 8;
const M_CPPRAW: u8 = 9;
const M_VERB: u8 = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chars {
    None,
    Simple,
    Rust,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tick {
    None,
    Template,
    Raw,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hash {
    None,
    LineStart,
    Anywhere,
}

pub(crate) struct Spec {
    pub kw: &'static [&'static str],
    pub types: &'static [&'static str],
    pub consts: &'static [&'static str],
    pub hash_comment: bool,
    pub nested: bool,
    pub preproc: Hash,
    pub hash_attr: bool,
    pub at_attr: bool,
    pub chars: Chars,
    pub sq_string: bool,
    pub tick: Tick,
    pub triple: bool,
    pub triple_escapes: bool,
    pub ml_strings: bool,
    pub rust_raw: bool,
    pub c_prefix: bool,
    pub cpp_raw: bool,
    pub verbatim: bool,
    pub cap_types: bool,
    pub dollar: bool,
    pub macros: bool,
    pub regex: bool,
    pub quote_sep: bool,
    pub php_tags: bool,
}

const BASE: Spec = Spec {
    kw: &[],
    types: &[],
    consts: &[],
    hash_comment: false,
    nested: false,
    preproc: Hash::None,
    hash_attr: false,
    at_attr: false,
    chars: Chars::Simple,
    sq_string: false,
    tick: Tick::None,
    triple: false,
    triple_escapes: true,
    ml_strings: false,
    rust_raw: false,
    c_prefix: false,
    cpp_raw: false,
    verbatim: false,
    cap_types: false,
    dollar: false,
    macros: false,
    regex: false,
    quote_sep: false,
    php_tags: false,
};

// ---- tables (sorted, byte order: `A-Z` < `_` < `a-z`) ----

const C_KW: &[&str] = &[
    "_Alignas", "_Alignof", "_Atomic", "_Generic", "_Noreturn", "_Static_assert", "_Thread_local", "__asm__",
    "__attribute__", "__inline", "__restrict", "alignas", "alignof", "asm", "auto", "break", "case", "const",
    "constexpr", "continue", "default", "do", "else", "enum", "extern", "for", "goto", "if", "inline", "register",
    "restrict", "return", "signed", "sizeof", "static", "static_assert", "struct", "switch", "thread_local",
    "typedef", "typeof", "union", "unsigned", "volatile", "while",
];
const C_TYPES: &[&str] = &[
    "FILE", "_Bool", "bool", "char", "double", "float", "int", "int16_t", "int32_t", "int64_t", "int8_t",
    "intptr_t", "long", "off_t", "pid_t", "ptrdiff_t", "short", "size_t", "ssize_t", "uint16_t", "uint32_t",
    "uint64_t", "uint8_t", "uintptr_t", "void", "wchar_t",
];
const C_CONSTS: &[&str] = &["NULL", "false", "nullptr", "true"];

const CPP_KW: &[&str] = &[
    "__asm__", "__attribute__", "alignas", "alignof", "and", "asm", "auto", "break", "case", "catch", "class",
    "co_await", "co_return", "co_yield", "concept", "const", "const_cast", "consteval", "constexpr", "constinit",
    "continue", "decltype", "default", "delete", "do", "dynamic_cast", "else", "enum", "explicit", "export",
    "extern", "final", "for", "friend", "goto", "if", "inline", "mutable", "namespace", "new", "noexcept", "not",
    "operator", "or", "override", "private", "protected", "public", "register", "reinterpret_cast", "requires",
    "return", "signed", "sizeof", "static", "static_assert", "static_cast", "struct", "switch", "template",
    "this", "thread_local", "throw", "try", "typedef", "typeid", "typename", "union", "unsigned", "using",
    "virtual", "volatile", "while",
];
const CPP_TYPES: &[&str] = &[
    "FILE", "bool", "char", "char16_t", "char32_t", "char8_t", "double", "float", "int", "int16_t", "int32_t",
    "int64_t", "int8_t", "intptr_t", "long", "ptrdiff_t", "short", "size_t", "ssize_t", "uint16_t", "uint32_t",
    "uint64_t", "uint8_t", "uintptr_t", "void", "wchar_t",
];

const RUST_KW: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "static",
    "struct", "super", "trait", "type", "union", "unsafe", "use", "where", "while", "yield",
];
const RUST_TYPES: &[&str] = &[
    "Self", "bool", "char", "f32", "f64", "i128", "i16", "i32", "i64", "i8", "isize", "str", "u128", "u16", "u32",
    "u64", "u8", "usize",
];
const RUST_CONSTS: &[&str] = &["None", "false", "true"];

const GO_KW: &[&str] = &[
    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go",
    "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type",
    "var",
];
const GO_TYPES: &[&str] = &[
    "any", "bool", "byte", "complex128", "complex64", "error", "float32", "float64", "int", "int16", "int32",
    "int64", "int8", "rune", "string", "uint", "uint16", "uint32", "uint64", "uint8", "uintptr",
];
const GO_CONSTS: &[&str] = &["false", "iota", "nil", "true"];

const JAVA_KW: &[&str] = &[
    "abstract", "assert", "break", "case", "catch", "class", "const", "continue", "default", "do", "else", "enum",
    "extends", "final", "finally", "for", "goto", "if", "implements", "import", "instanceof", "interface",
    "native", "new", "non-sealed", "package", "permits", "private", "protected", "public", "record", "return",
    "sealed", "static", "strictfp", "super", "switch", "synchronized", "this", "throw", "throws", "transient",
    "try", "var", "volatile", "while", "yield",
];
const JAVA_TYPES: &[&str] = &["boolean", "byte", "char", "double", "float", "int", "long", "short", "void"];
const NULL_CONSTS: &[&str] = &["false", "null", "true"];

const JS_KW: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete",
    "do", "else", "export", "extends", "finally", "for", "function", "if", "import", "in", "instanceof", "let",
    "new", "of", "return", "static", "super", "switch", "this", "throw", "try", "typeof", "var", "void", "while",
    "with", "yield",
];
const JS_CONSTS: &[&str] = &["Infinity", "NaN", "false", "null", "true", "undefined"];
const TS_KW: &[&str] = &[
    "abstract", "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger",
    "declare", "default", "delete", "do", "else", "enum", "export", "extends", "finally", "for", "function", "if",
    "implements", "import", "in", "infer", "instanceof", "interface", "is", "keyof", "let", "namespace", "new",
    "of", "override", "private", "protected", "public", "readonly", "return", "satisfies", "static", "super",
    "switch", "this", "throw", "try", "type", "typeof", "var", "void", "while", "with", "yield",
];
const TS_TYPES: &[&str] =
    &["any", "bigint", "boolean", "never", "number", "object", "string", "symbol", "unknown"];

const CS_KW: &[&str] = &[
    "abstract", "as", "async", "await", "base", "break", "case", "catch", "checked", "class", "const", "continue",
    "default", "delegate", "do", "else", "enum", "event", "explicit", "extern", "finally", "fixed", "for",
    "foreach", "get", "goto", "if", "implicit", "in", "init", "interface", "internal", "is", "lock", "namespace",
    "new", "operator", "out", "override", "params", "private", "protected", "public", "readonly", "record", "ref",
    "return", "sealed", "set", "sizeof", "stackalloc", "static", "struct", "switch", "this", "throw", "try",
    "typeof", "unchecked", "unsafe", "using", "var", "virtual", "volatile", "where", "while", "yield",
];
const CS_TYPES: &[&str] = &[
    "bool", "byte", "char", "decimal", "double", "dynamic", "float", "int", "long", "nint", "nuint", "object",
    "sbyte", "short", "string", "uint", "ulong", "ushort", "void",
];

/// `include`/`require`/`eval` stay out: `outline` treats `include(…)` as a call (File sink).
const PHP_KW: &[&str] = &[
    "abstract", "and", "as", "break", "callable", "case", "catch", "class", "clone", "const", "continue",
    "declare", "default", "do", "echo", "else", "elseif", "empty", "enddeclare", "endfor", "endforeach", "endif",
    "endswitch", "endwhile", "enum", "extends", "final", "finally", "fn", "for", "foreach", "function", "global",
    "goto", "if", "implements", "instanceof", "insteadof", "interface", "isset", "list", "match", "namespace",
    "new", "or", "print", "private", "protected", "public", "readonly", "return", "static", "switch", "throw",
    "trait", "try", "unset", "use", "var", "while", "xor", "yield",
];
const PHP_TYPES: &[&str] =
    &["array", "bool", "float", "int", "iterable", "mixed", "never", "object", "string", "void"];

const KT_KW: &[&str] = &[
    "abstract", "as", "break", "by", "catch", "class", "companion", "const", "constructor", "continue", "data",
    "do", "else", "enum", "external", "finally", "for", "fun", "if", "import", "in", "init", "inline", "interface",
    "internal", "is", "lateinit", "object", "open", "operator", "out", "override", "package", "private",
    "protected", "public", "return", "sealed", "super", "suspend", "this", "throw", "try", "typealias", "val",
    "var", "when", "where", "while",
];

const SWIFT_KW: &[&str] = &[
    "any", "as", "associatedtype", "async", "await", "break", "case", "catch", "class", "continue", "default",
    "defer", "deinit", "do", "else", "enum", "extension", "fallthrough", "fileprivate", "final", "for", "func",
    "guard", "if", "import", "in", "init", "inout", "internal", "is", "let", "mutating", "open", "operator",
    "override", "private", "protocol", "public", "repeat", "rethrows", "return", "self", "some", "static",
    "struct", "subscript", "super", "switch", "throw", "throws", "try", "typealias", "var", "weak", "where",
    "while",
];
const SWIFT_TYPES: &[&str] = &["Self"];
const NIL_CONSTS: &[&str] = &["false", "nil", "true"];

pub(crate) static C: Spec =
    Spec { kw: C_KW, types: C_TYPES, consts: C_CONSTS, preproc: Hash::LineStart, c_prefix: true, ..BASE };
pub(crate) static CPP: Spec = Spec {
    kw: CPP_KW,
    types: CPP_TYPES,
    consts: C_CONSTS,
    preproc: Hash::LineStart,
    c_prefix: true,
    cpp_raw: true,
    quote_sep: true,
    ..BASE
};
pub(crate) static RUST: Spec = Spec {
    kw: RUST_KW,
    types: RUST_TYPES,
    consts: RUST_CONSTS,
    nested: true,
    hash_attr: true,
    chars: Chars::Rust,
    ml_strings: true,
    rust_raw: true,
    cap_types: true,
    macros: true,
    ..BASE
};
pub(crate) static GO: Spec =
    Spec { kw: GO_KW, types: GO_TYPES, consts: GO_CONSTS, tick: Tick::Raw, cap_types: true, ..BASE };
pub(crate) static JAVA: Spec = Spec {
    kw: JAVA_KW,
    types: JAVA_TYPES,
    consts: NULL_CONSTS,
    at_attr: true,
    triple: true,
    cap_types: true,
    ..BASE
};
pub(crate) static JS: Spec = Spec {
    kw: JS_KW,
    consts: JS_CONSTS,
    at_attr: true,
    chars: Chars::None,
    sq_string: true,
    tick: Tick::Template,
    dollar: true,
    regex: true,
    ..BASE
};
pub(crate) static TS: Spec = Spec { kw: TS_KW, types: TS_TYPES, cap_types: true, ..JS };
pub(crate) static CSHARP: Spec = Spec {
    kw: CS_KW,
    types: CS_TYPES,
    consts: NULL_CONSTS,
    preproc: Hash::LineStart,
    triple: true,
    verbatim: true,
    cap_types: true,
    ..BASE
};
pub(crate) static PHP: Spec = Spec {
    kw: PHP_KW,
    types: PHP_TYPES,
    consts: NULL_CONSTS,
    hash_comment: true,
    hash_attr: true,
    chars: Chars::None,
    sq_string: true,
    ml_strings: true,
    dollar: true,
    php_tags: true,
    ..BASE
};
pub(crate) static KOTLIN: Spec = Spec {
    kw: KT_KW,
    consts: NULL_CONSTS,
    nested: true,
    at_attr: true,
    triple: true,
    triple_escapes: false,
    cap_types: true,
    ..BASE
};
pub(crate) static SWIFT: Spec = Spec {
    kw: SWIFT_KW,
    types: SWIFT_TYPES,
    consts: NIL_CONSTS,
    nested: true,
    preproc: Hash::Anywhere,
    at_attr: true,
    chars: Chars::None,
    triple: true,
    cap_types: true,
    ..BASE
};

#[cfg(test)]
pub(crate) fn tables() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        ("C_KW", C_KW),
        ("C_TYPES", C_TYPES),
        ("C_CONSTS", C_CONSTS),
        ("CPP_KW", CPP_KW),
        ("CPP_TYPES", CPP_TYPES),
        ("RUST_KW", RUST_KW),
        ("RUST_TYPES", RUST_TYPES),
        ("RUST_CONSTS", RUST_CONSTS),
        ("GO_KW", GO_KW),
        ("GO_TYPES", GO_TYPES),
        ("GO_CONSTS", GO_CONSTS),
        ("JAVA_KW", JAVA_KW),
        ("JAVA_TYPES", JAVA_TYPES),
        ("NULL_CONSTS", NULL_CONSTS),
        ("JS_KW", JS_KW),
        ("JS_CONSTS", JS_CONSTS),
        ("TS_KW", TS_KW),
        ("TS_TYPES", TS_TYPES),
        ("CS_KW", CS_KW),
        ("CS_TYPES", CS_TYPES),
        ("PHP_KW", PHP_KW),
        ("PHP_TYPES", PHP_TYPES),
        ("KT_KW", KT_KW),
        ("SWIFT_KW", SWIFT_KW),
        ("SWIFT_TYPES", SWIFT_TYPES),
        ("NIL_CONSTS", NIL_CONSTS),
    ]
}

/// Keywords after which `/` starts a regex (JS).
const REGEX_AFTER: &[&str] =
    &["await", "case", "delete", "do", "else", "in", "instanceof", "new", "of", "return", "throw", "typeof", "void", "yield"];

fn is_op(c: u8) -> bool {
    matches!(c, b'+' | b'-' | b'*' | b'/' | b'%' | b'=' | b'<' | b'>' | b'!' | b'&' | b'|' | b'^' | b'~' | b'?' | b':')
}

pub(crate) fn lex(em: &mut Em, line: &str, state: LineState, sp: &Spec) -> LineState {
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0;
    let mut tdepth: u32 = 0;
    let mut regex_ok = true;
    let mut regex_dead = false;
    match mode(state) {
        M_BLOCK => match block_comment(em, b, 0, 0, u32::from(param(state).max(1)), sp.nested) {
            Ok(e) => i = e,
            Err(d) => return st(M_BLOCK, d, 0),
        },
        M_DQ | M_SQ => {
            let q = if mode(state) == M_DQ { b'"' } else { b'\'' };
            let (e, closed) = string_body(em, b, 0, 0, q, true);
            if !closed {
                return st(mode(state), 0, 0);
            }
            i = e;
            regex_ok = false;
        }
        M_RAW => {
            let h = usize::from(param(state));
            match raw_close(b, 0, h) {
                Some(e) => {
                    em.push(0, e, K::String);
                    i = e;
                }
                None => {
                    em.push(0, n, K::String);
                    return st(M_RAW, param(state), 0);
                }
            }
        }
        M_TMPL => match template(em, b, 0, 0) {
            Tpl::Closed(e) => i = e,
            Tpl::Interp(e) => {
                i = e;
                tdepth = 1;
            }
            Tpl::Eol => return st(M_TMPL, 0, 0),
        },
        M_TEXPR => tdepth = u32::from(param(state).max(1)),
        M_GORAW => match b.iter().position(|&c| c == b'`') {
            Some(p) => {
                em.push(0, p + 1, K::String);
                i = p + 1;
            }
            None => {
                em.push(0, n, K::String);
                return st(M_GORAW, 0, 0);
            }
        },
        M_TRIPLE => {
            let q = if param(state) == b'\'' { b'\'' } else { b'"' };
            let (e, closed) = triple_body(em, b, 0, 0, q, sp.triple_escapes);
            if !closed {
                return st(M_TRIPLE, q, 0);
            }
            i = e;
        }
        M_CPPRAW => {
            let len = usize::from(param(state));
            match cppraw_close(b, 0, len, extra(state)) {
                Some(e) => {
                    em.push(0, e, K::String);
                    i = e;
                }
                None => {
                    em.push(0, n, K::String);
                    return state;
                }
            }
        }
        M_VERB => {
            let (e, closed) = verbatim_body(em, b, 0, 0);
            if !closed {
                return st(M_VERB, 0, 0);
            }
            i = e;
        }
        _ => {}
    }
    let first = skip_ws(b, 0);
    while let Some(&c) = b.get(i) {
        match c {
            b' ' | b'\t' | b'\r' | 0x0c => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                em.push(i, n, K::Comment);
                i = n;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => match block_comment(em, b, i, i + 2, 1, sp.nested) {
                Ok(e) => i = e,
                Err(d) => return st(M_BLOCK, d, 0),
            },
            b'/' if sp.regex && regex_ok && !regex_dead => match regex_end(b, i) {
                Some(e) => {
                    em.push(i, e, K::String);
                    i = e;
                    regex_ok = false;
                }
                // No regex closes on this line: stop looking (keeps the line linear).
                None => regex_dead = true,
            },
            b'#' if sp.hash_attr && (b.get(i + 1) == Some(&b'[') || b.get(i + 1..i + 3) == Some(b"![")) => {
                let e = attr_end(b, i);
                em.push(i, e, K::Attribute);
                i = e;
            }
            b'#' if sp.hash_comment => {
                em.push(i, n, K::Comment);
                i = n;
            }
            b'#' if (sp.preproc == Hash::LineStart && i == first) || sp.preproc == Hash::Anywhere => {
                let j = skip_ws(b, i + 1);
                let we = word_end(b, j, false);
                em.push(i, we, K::Attribute);
                i = we;
                let w = b.get(j..we).unwrap_or(b"");
                if matches!(w, b"include" | b"import" | b"include_next") {
                    let k = skip_ws(b, we);
                    if b.get(k) == Some(&b'<') {
                        if let Some(p) = b.get(k..).and_then(|r| r.iter().position(|&c| c == b'>')) {
                            em.push(k, k + p + 1, K::String);
                            i = k + p + 1;
                        }
                    }
                }
            }
            b'@' if sp.verbatim && b.get(i + 1) == Some(&b'"') => {
                let (e, closed) = verbatim_body(em, b, i, i + 2);
                if !closed {
                    return st(M_VERB, 0, 0);
                }
                i = e;
                regex_ok = false;
            }
            b'@' if sp.verbatim && b.get(i + 1) == Some(&b'$') && b.get(i + 2) == Some(&b'"') => {
                let (e, closed) = verbatim_body(em, b, i, i + 3);
                if !closed {
                    return st(M_VERB, 0, 0);
                }
                i = e;
            }
            b'$' if sp.verbatim && b.get(i + 1) == Some(&b'@') && b.get(i + 2) == Some(&b'"') => {
                let (e, closed) = verbatim_body(em, b, i, i + 3);
                if !closed {
                    return st(M_VERB, 0, 0);
                }
                i = e;
            }
            b'$' if sp.verbatim && b.get(i + 1) == Some(&b'"') => {
                // Interpolated string: `{expr}` left inside the string.
                if sp.triple && b.get(i + 2..i + 4) == Some(b"\"\"") {
                    let (e, closed) = triple_body(em, b, i, i + 4, b'"', true);
                    if !closed {
                        return st(M_TRIPLE, b'"', 0);
                    }
                    i = e;
                } else {
                    let (e, _) = string_body(em, b, i, i + 2, b'"', true);
                    i = e;
                }
            }
            b'@' if sp.at_attr && b.get(i + 1).is_some_and(|&d| is_ident_start(d)) => {
                let mut e = word_end(b, i + 1, false);
                while b.get(e) == Some(&b'.') && b.get(e + 1).is_some_and(|&d| is_ident_start(d)) {
                    e = word_end(b, e + 1, false);
                }
                em.push(i, e, K::Attribute);
                i = e;
            }
            b'"' => {
                if sp.triple && b.get(i + 1..i + 3) == Some(b"\"\"") {
                    let (e, closed) = triple_body(em, b, i, i + 3, b'"', sp.triple_escapes);
                    if !closed {
                        return st(M_TRIPLE, b'"', 0);
                    }
                    i = e;
                } else {
                    let (e, closed) = string_body(em, b, i, i + 1, b'"', true);
                    if !closed && sp.ml_strings {
                        return st(M_DQ, 0, 0);
                    }
                    i = e;
                }
                regex_ok = false;
            }
            b'\'' if sp.sq_string => {
                let (e, closed) = string_body(em, b, i, i + 1, b'\'', true);
                if !closed && sp.ml_strings {
                    return st(M_SQ, 0, 0);
                }
                i = e;
                regex_ok = false;
            }
            b'\'' if sp.chars == Chars::Rust => {
                i = rust_quote(em, b, i);
                regex_ok = false;
            }
            b'\'' if sp.chars == Chars::Simple => {
                let (e, _) = string_body(em, b, i, i + 1, b'\'', true);
                i = e;
            }
            b'`' if sp.tick == Tick::Template => {
                if tdepth > 0 {
                    // Nested template inside `${…}`: no interpolation tracking.
                    let e = b.get(i + 1..).and_then(|r| r.iter().position(|&c| c == b'`')).map_or(n, |p| i + p + 2);
                    em.push(i, e, K::String);
                    i = e;
                    continue;
                }
                match template(em, b, i, i + 1) {
                    Tpl::Closed(e) => i = e,
                    Tpl::Interp(e) => {
                        i = e;
                        tdepth = 1;
                    }
                    Tpl::Eol => return st(M_TMPL, 0, 0),
                }
                regex_ok = false;
            }
            b'`' if sp.tick == Tick::Raw => match b.get(i + 1..).and_then(|r| r.iter().position(|&c| c == b'`')) {
                Some(p) => {
                    em.push(i, i + p + 2, K::String);
                    i = i + p + 2;
                }
                None => {
                    em.push(i, n, K::String);
                    return st(M_GORAW, 0, 0);
                }
            },
            b'0'..=b'9' => {
                let e = number_end(b, i, sp.quote_sep);
                em.push(i, e, K::Number);
                i = e;
                regex_ok = false;
            }
            b'.' if b.get(i + 1).is_some_and(u8::is_ascii_digit) && !b.get(i.wrapping_sub(1)).is_some_and(|&p| p.is_ascii_alphanumeric() || p == b'_') => {
                let e = number_end(b, i, sp.quote_sep);
                em.push(i, e, K::Number);
                i = e;
                regex_ok = false;
            }
            b'<' if sp.php_tags && b.get(i + 1) == Some(&b'?') => {
                let e = if b.get(i + 2..i + 5).is_some_and(|w| w.eq_ignore_ascii_case(b"php")) { i + 5 } else { i + 2 };
                let e = if b.get(e) == Some(&b'=') { e + 1 } else { e };
                em.push(i, e, K::Attribute);
                i = e;
            }
            b'?' if sp.php_tags && b.get(i + 1) == Some(&b'>') => {
                em.push(i, i + 2, K::Attribute);
                i += 2;
            }
            _ if is_ident_start(c) || (sp.dollar && c == b'$') => {
                let we = word_end(b, i, sp.dollar);
                let w = b.get(i..we).unwrap_or(b"");
                if let Some(e) = prefixed_string(em, b, i, we, w, sp) {
                    match e {
                        Ok(e) => i = e,
                        Err(s) => return s,
                    }
                    regex_ok = false;
                    continue;
                }
                if sp.macros && b.get(we) == Some(&b'!') && b.get(we + 1) != Some(&b'=') {
                    em.push(i, we + 1, K::Attribute);
                    i = we + 1;
                    continue;
                }
                let ws = line.get(i..we).unwrap_or("");
                let kind = classify(sp, ws, w, b, we);
                if let Some(k) = kind {
                    em.push(i, we, k);
                }
                regex_ok = sp.regex && kind == Some(K::Keyword) && in_table(REGEX_AFTER, ws);
                i = we;
            }
            b'{' => {
                if tdepth > 0 {
                    tdepth = (tdepth + 1).min(255);
                }
                em.push(i, i + 1, K::Punct);
                i += 1;
                regex_ok = true;
            }
            b'}' => {
                em.push(i, i + 1, K::Punct);
                i += 1;
                regex_ok = false;
                if tdepth > 0 {
                    tdepth -= 1;
                    if tdepth == 0 {
                        match template(em, b, i, i) {
                            Tpl::Closed(e) => i = e,
                            Tpl::Interp(e) => {
                                i = e;
                                tdepth = 1;
                            }
                            Tpl::Eol => return st(M_TMPL, 0, 0),
                        }
                    }
                }
            }
            b'(' | b'[' | b',' | b';' => {
                em.push(i, i + 1, K::Punct);
                i += 1;
                regex_ok = true;
            }
            b')' | b']' | b'.' => {
                em.push(i, i + 1, K::Punct);
                i += 1;
                regex_ok = false;
            }
            _ if is_op(c) => {
                let mut j = i + 1;
                while let Some(&d) = b.get(j) {
                    if !is_op(d) || (d == b'/' && matches!(b.get(j + 1), Some(b'/' | b'*'))) {
                        break;
                    }
                    if sp.php_tags && d == b'?' && b.get(j + 1) == Some(&b'>') {
                        break;
                    }
                    j += 1;
                }
                em.push(i, j, K::Operator);
                i = j;
                regex_ok = true;
            }
            _ => i = next_char(b, i),
        }
    }
    if tdepth > 0 {
        st(M_TEXPR, tdepth.min(255) as u8, 0)
    } else {
        LineState::default()
    }
}

fn classify(sp: &Spec, ws: &str, w: &[u8], b: &[u8], we: usize) -> Option<K> {
    if in_table(sp.kw, ws) {
        return Some(K::Keyword);
    }
    if in_table(sp.types, ws) {
        return Some(K::Type);
    }
    if in_table(sp.consts, ws) {
        return Some(K::Constant);
    }
    if b.get(skip_ws(b, we)) == Some(&b'(') {
        return Some(K::Function);
    }
    if is_all_caps(w) {
        return Some(K::Constant);
    }
    if sp.cap_types && w.first().is_some_and(u8::is_ascii_uppercase) {
        return Some(K::Type);
    }
    None
}

/// String literals that start with an identifier-like prefix. `None`: not one.
/// `Some(Ok(end))` lexed; `Some(Err(state))` unclosed multi-line literal.
fn prefixed_string(em: &mut Em, b: &[u8], i: usize, we: usize, w: &[u8], sp: &Spec) -> Option<Result<usize, LineState>> {
    let next = b.get(we).copied();
    if sp.rust_raw {
        if matches!(w, b"r" | b"br" | b"cr") && matches!(next, Some(b'#' | b'"')) {
            let mut h = we;
            while b.get(h) == Some(&b'#') {
                h += 1;
            }
            if b.get(h) != Some(&b'"') {
                return None;
            }
            let hashes = h - we;
            return Some(match raw_close(b, h + 1, hashes) {
                Some(e) => {
                    em.push(i, e, K::String);
                    Ok(e)
                }
                None => {
                    em.push(i, b.len(), K::String);
                    Err(st(M_RAW, hashes.min(255) as u8, 0))
                }
            });
        }
        if matches!(w, b"b" | b"c") && next == Some(b'"') {
            let (e, closed) = string_body(em, b, i, we + 1, b'"', true);
            return Some(if closed { Ok(e) } else { Err(st(M_DQ, 0, 0)) });
        }
        if w == b"b" && next == Some(b'\'') {
            let (e, _) = string_body(em, b, i, we + 1, b'\'', true);
            return Some(Ok(e));
        }
    }
    if sp.cpp_raw && matches!(w, b"R" | b"LR" | b"uR" | b"UR" | b"u8R") && next == Some(b'"') {
        let ds = we + 1;
        let mut p = ds;
        while p < ds + 17 {
            match b.get(p) {
                Some(b'(') => break,
                Some(b' ' | b')' | b'\\' | b'\t' | b'"') | None => return None,
                Some(_) => p += 1,
            }
        }
        if b.get(p) != Some(&b'(') {
            return None;
        }
        let d = b.get(ds..p).unwrap_or(b"");
        let (len, packed) = (d.len(), pack_delim(d));
        return Some(match cppraw_close(b, p + 1, len, packed) {
            Some(e) => {
                em.push(i, e, K::String);
                Ok(e)
            }
            None => {
                em.push(i, b.len(), K::String);
                Err(st(M_CPPRAW, len as u8, packed))
            }
        });
    }
    if sp.c_prefix && matches!(w, b"L" | b"u" | b"U" | b"u8") && matches!(next, Some(b'"' | b'\'')) {
        let q = next.unwrap_or(b'"');
        let (e, _) = string_body(em, b, i, we + 1, q, true);
        return Some(Ok(e));
    }
    None
}

/// Rust `'`: char literal, byte escape, or lifetime/label.
fn rust_quote(em: &mut Em, b: &[u8], i: usize) -> usize {
    match b.get(i + 1) {
        Some(b'\\') => string_body(em, b, i, i + 1, b'\'', true).0,
        Some(&d) => {
            let cl = next_char(b, i + 1);
            if b.get(cl) == Some(&b'\'') {
                em.push(i, cl + 1, K::String);
                cl + 1
            } else if is_ident_start(d) {
                let e = word_end(b, i + 1, false);
                em.push(i, e, K::Type);
                e
            } else {
                i + 1
            }
        }
        None => i + 1,
    }
}

/// Block comment text from `i` (after the opener). `Ok(end)` when closed, `Err(depth)` at end of line.
fn block_comment(em: &mut Em, b: &[u8], seg: usize, i: usize, depth: u32, nested: bool) -> Result<usize, u8> {
    let mut d = depth;
    let mut j = i;
    while let Some(&c) = b.get(j) {
        if c == b'*' && b.get(j + 1) == Some(&b'/') {
            d -= 1;
            j += 2;
            if d == 0 {
                em.push(seg, j, K::Comment);
                return Ok(j);
            }
        } else if nested && c == b'/' && b.get(j + 1) == Some(&b'*') {
            d = (d + 1).min(255);
            j += 2;
        } else {
            j += 1;
        }
    }
    em.push(seg, b.len(), K::Comment);
    Err(d.clamp(1, 255) as u8)
}

enum Tpl {
    Closed(usize),
    Interp(usize),
    Eol,
}

/// Template text from `i`; `seg` is where the `String` piece starts.
fn template(em: &mut Em, b: &[u8], seg: usize, i: usize) -> Tpl {
    let mut seg = seg;
    let mut j = i;
    while let Some(&c) = b.get(j) {
        match c {
            b'`' => {
                em.push(seg, j + 1, K::String);
                return Tpl::Closed(j + 1);
            }
            b'\\' => {
                em.push(seg, j, K::String);
                let e = escape_end(b, j);
                em.push(j, e, K::Escape);
                j = e;
                seg = e;
            }
            b'$' if b.get(j + 1) == Some(&b'{') => {
                em.push(seg, j, K::String);
                em.push(j, j + 2, K::Punct);
                return Tpl::Interp(j + 2);
            }
            _ => j += 1,
        }
    }
    em.push(seg, b.len(), K::String);
    Tpl::Eol
}

fn triple_body(em: &mut Em, b: &[u8], seg: usize, i: usize, q: u8, escapes: bool) -> (usize, bool) {
    let mut seg = seg;
    let mut j = i;
    while let Some(&c) = b.get(j) {
        if c == q && b.get(j + 1) == Some(&q) && b.get(j + 2) == Some(&q) {
            em.push(seg, j + 3, K::String);
            return (j + 3, true);
        }
        if escapes && c == b'\\' {
            em.push(seg, j, K::String);
            let e = escape_end(b, j);
            em.push(j, e, K::Escape);
            j = e;
            seg = e;
            continue;
        }
        j += 1;
    }
    em.push(seg, b.len(), K::String);
    (b.len(), false)
}

/// C# verbatim string: `""` is a quote, no other escape.
fn verbatim_body(em: &mut Em, b: &[u8], seg: usize, i: usize) -> (usize, bool) {
    let mut j = i;
    while let Some(&c) = b.get(j) {
        if c == b'"' {
            if b.get(j + 1) == Some(&b'"') {
                j += 2;
                continue;
            }
            em.push(seg, j + 1, K::String);
            return (j + 1, true);
        }
        j += 1;
    }
    em.push(seg, b.len(), K::String);
    (b.len(), false)
}

/// End of a Rust raw string body: `"` followed by `h` hashes.
fn raw_close(b: &[u8], i: usize, h: usize) -> Option<usize> {
    let mut j = i;
    while let Some(p) = b.get(j..).and_then(|r| r.iter().position(|&c| c == b'"')) {
        let q = j + p;
        let tail = b.get(q + 1..q + 1 + h);
        if tail.is_some_and(|t| t.iter().all(|&c| c == b'#')) {
            return Some(q + 1 + h);
        }
        j = q + 1;
    }
    None
}

/// End of a C++ raw string body: `)` + delimiter + `"`.
fn cppraw_close(b: &[u8], i: usize, len: usize, packed: u64) -> Option<usize> {
    let mut j = i;
    while let Some(p) = b.get(j..).and_then(|r| r.iter().position(|&c| c == b')')) {
        let q = j + p;
        if let Some(d) = b.get(q + 1..q + 1 + len) {
            if delim_matches(d, len, packed) && b.get(q + 1 + len) == Some(&b'"') {
                return Some(q + 2 + len);
            }
        }
        j = q + 1;
    }
    None
}

/// `#[…]` / `#![…]` up to the matching `]` (strings inside skipped), else end of line.
fn attr_end(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    let mut depth = 0u32;
    while let Some(&c) = b.get(j) {
        match c {
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return j + 1;
                }
            }
            b'"' => {
                j += 1;
                while let Some(&d) = b.get(j) {
                    if d == b'\\' {
                        j += 1;
                    } else if d == b'"' {
                        break;
                    }
                    j += 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    b.len()
}

/// End of a JS regex literal starting at `b[i] == '/'`, if it closes on this line.
fn regex_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    let mut class = false;
    if matches!(b.get(j), Some(b'/' | b'*') | None) {
        return None;
    }
    while let Some(&c) = b.get(j) {
        match c {
            b'\\' => j = next_char(b, j + 1),
            b'[' => {
                class = true;
                j += 1;
            }
            b']' => {
                class = false;
                j += 1;
            }
            b'/' if !class => {
                let mut e = j + 1;
                while b.get(e).is_some_and(u8::is_ascii_alphabetic) {
                    e += 1;
                }
                return Some(e);
            }
            _ => j += 1,
        }
    }
    None
}
