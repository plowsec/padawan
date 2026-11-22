//! Hostile input: garbage, mutated fixtures, huge lines. Nothing may panic; tokens keep the contract.

mod common;
use common::fixtures;
use common::check_tokens;
use margin_code::{lex_line, outline, Lang, LineState, Token};
use std::time::Instant;

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const INSERTS: &[&str] = &[
    "{", "}", "(", ")", "\"", "'", "/*", "*/", "\n", "`", "[", "]", "#", "\\", "r#\"", "\"\"\"", "'''", "<!--", "-->", "--[[",
    "]]", "${", "<<EOF\n", "=begin\n", "```\n", "é", "日本", "🦀", "\u{FFFD}", "\0", "\r", "\t", "<script>", "def ", "fn ",
    "function ", "end", "@", "$(", "=>", "::", "->",
];

fn fixtures() -> Vec<(Lang, &'static str)> {
    vec![
        (Lang::C, fixtures::C),
        (Lang::Cpp, fixtures::CPP),
        (Lang::Rust, fixtures::RUST),
        (Lang::Go, fixtures::GO),
        (Lang::Java, fixtures::JAVA),
        (Lang::JavaScript, fixtures::JS),
        (Lang::TypeScript, fixtures::TS),
        (Lang::Python, fixtures::PYTHON),
        (Lang::CSharp, fixtures::CSHARP),
        (Lang::Php, fixtures::PHP),
        (Lang::Ruby, fixtures::RUBY),
        (Lang::Kotlin, fixtures::KOTLIN),
        (Lang::Swift, fixtures::SWIFT),
        (Lang::Lua, fixtures::LUA),
        (Lang::Shell, fixtures::SHELL),
        (Lang::Markdown, "# T\n```\ncode\n```\n*a* **b** `c` [d](e) <f> <!-- g\n-->"),
        (Lang::Html, "<a href='x'>t</a><script>let a = `x${1}`;</script><style>p{c:d}</style><!-- c\n-->"),
        (Lang::Yaml, "a: |\n  x\nb: [1, {c: d}]\n- &x y\n"),
        (Lang::Toml, "[a]\nb = \"\"\"\nc\n\"\"\"\nd = 'e'"),
        (Lang::Json, "{\"a\": [1, true, null], \"b\": \"c\\n\"} /* x\n*/"),
        (Lang::Sql, "SELECT 'a''b' /* c\n*/ FROM t -- d"),
        (Lang::Css, "@media x { .a:hover { b: #fff !important; } } /* c\n*/"),
        (Lang::Makefile, "A := b\nall: x\n\t$(CC) $@ # c"),
        (Lang::Diff, "--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y"),
    ]
}

/// Lex every line of `text`; checks the contract and determinism.
fn lex_all(lang: Lang, text: &str, buf: &mut Vec<Token>) {
    let mut st = LineState::default();
    for line in text.split('\n') {
        buf.clear();
        let next = lex_line(lang, st, line, buf);
        check_tokens(line, buf);
        let n = buf.len();
        let again = lex_line(lang, st, line, buf);
        assert_eq!(again, next);
        assert_eq!(buf[..n], buf[n..], "relex differs for {lang:?}: {line:?}");
        st = next;
    }
}

fn check_outline(lang: Lang, text: &str) {
    let o = outline(lang, text);
    let lines = text.split('\n').count() as u32;
    for f in &o.fns {
        assert!(f.start <= f.line && f.line <= f.end && f.end < lines.max(1), "{lang:?} bad extent {f:?}");
        assert!(f.calls.len() <= 2000 && !f.name.is_empty() && f.name.len() <= 257);
        for c in &f.calls {
            assert!(c.line < lines.max(1) && !c.name.is_empty() && c.qualified.len() <= 140, "{c:?}");
        }
    }
}

fn boundaries(s: &str) -> impl Iterator<Item = usize> + '_ {
    (0..=s.len()).filter(|&i| s.is_char_boundary(i))
}

#[test]
fn truncations_never_panic() {
    let mut buf = Vec::new();
    for (k, (lang, src)) in fixtures().into_iter().enumerate() {
        for (j, cut) in boundaries(src).enumerate() {
            let t = &src[..cut];
            check_outline(lang, t);
            lex_all(lang, t, &mut buf);
            // Every language on a stride of the cuts.
            if (j + k) % 41 == 0 {
                for other in Lang::ALL {
                    check_outline(other, t);
                    lex_all(other, t, &mut buf);
                }
            }
        }
    }
}

#[test]
fn mutations_never_panic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut buf = Vec::new();
    for (lang, src) in fixtures() {
        for _ in 0..60 {
            let mut s = src.to_string();
            for _ in 0..1 + rng.below(12) {
                let cuts: Vec<usize> = boundaries(&s).collect();
                let at = cuts[rng.below(cuts.len())];
                match rng.below(3) {
                    0 | 1 => s.insert_str(at, INSERTS[rng.below(INSERTS.len())]),
                    _ => {
                        // Delete a char.
                        if let Some(c) = s[at..].chars().next() {
                            s.replace_range(at..at + c.len_utf8(), "");
                        }
                    }
                }
            }
            check_outline(lang, &s);
            lex_all(lang, &s, &mut buf);
            let other = Lang::ALL[rng.below(Lang::ALL.len())];
            check_outline(other, &s);
            lex_all(other, &s, &mut buf);
        }
    }
}

#[test]
fn garbage_never_panics() {
    let mut rng = Rng(42);
    let alphabet: Vec<char> = "abcXYZ_09 \t{}()[]<>\"'`/*\\#@$%^&|!?:;.,=+-~\né日🦀\u{FFFD}\u{0}\u{7F}\r".chars().collect();
    let mut buf = Vec::new();
    for round in 0..300 {
        let len = rng.below(if round % 10 == 0 { 20_000 } else { 400 });
        let s: String = (0..len).map(|_| alphabet[rng.below(alphabet.len())]).collect();
        for lang in Lang::ALL {
            lex_all(lang, &s, &mut buf);
            check_outline(lang, &s);
        }
    }
}

#[test]
fn random_states_never_panic() {
    let mut rng = Rng(7);
    let mut buf = Vec::new();
    for _ in 0..2000 {
        let st = LineState(rng.next());
        let line = ["a /* b", "\"x", "`${", "]==]", "*/ q", "EOF", "  - k: |", "```", "-->", "'''"][rng.below(10)];
        for lang in Lang::ALL {
            buf.clear();
            lex_line(lang, st, line, &mut buf);
            check_tokens(line, &buf);
        }
    }
}

#[test]
fn huge_single_line() {
    let unit = "f(a, \"s\\\"\", 'c', /* c */ b[1]) { x = y->z::w; } `t ${u}` # ' ";
    let line = unit.repeat(5 * 1024 * 1024 / unit.len());
    let mut buf = Vec::new();
    let t = Instant::now();
    for lang in Lang::ALL {
        buf.clear();
        lex_line(lang, LineState::default(), &line, &mut buf);
        check_tokens(&line, &buf);
    }
    let lex_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    for lang in [Lang::C, Lang::JavaScript, Lang::Python, Lang::Ruby, Lang::Shell] {
        check_outline(lang, &line);
    }
    let out_s = t.elapsed().as_secs_f64();
    eprintln!("5 MB line: lex_line x{} langs {lex_s:.2}s, outline x5 langs {out_s:.2}s", Lang::ALL.len());
    assert!(lex_s < 30.0 && out_s < 30.0);
}

#[test]
fn many_lines() {
    let text = "int f(int x) { return g(x) + h(\"y\"); } // c\n".repeat(200_000);
    let t = Instant::now();
    let o = outline(Lang::C, &text);
    let s = t.elapsed().as_secs_f64();
    assert_eq!(o.fns.len(), 20_000);
    let t = Instant::now();
    let mut buf = Vec::new();
    lex_all(Lang::Python, &text, &mut buf);
    let l = t.elapsed().as_secs_f64();
    eprintln!("200k lines: outline {s:.2}s, lex {l:.2}s");
    assert!(s < 30.0 && l < 30.0);
}

#[test]
fn pathological_patterns_are_linear() {
    // Each would be quadratic with naive re-scanning.
    let pats = ["/ ( ", "* _ ", "[a](", "` `` ", "<<", "%w[", "r#", "\"\"", "${", "<a ", "'", "[==[", "(*", "f(", "{"];
    let mut buf = Vec::new();
    for p in pats {
        let line = p.repeat(200_000 / p.len());
        let t = Instant::now();
        for lang in Lang::ALL {
            buf.clear();
            lex_line(lang, LineState::default(), &line, &mut buf);
            check_tokens(&line, &buf);
        }
        for lang in [Lang::C, Lang::JavaScript, Lang::Ruby, Lang::Python, Lang::Lua] {
            check_outline(lang, &line);
        }
        let e = t.elapsed().as_secs_f64();
        assert!(e < 10.0, "{p:?} took {e:.2}s");
    }
}
