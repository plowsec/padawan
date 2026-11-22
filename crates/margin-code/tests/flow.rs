//! `flow` on hostile input: fixtures, mutations, garbage, unbalanced brackets, huge lines.
//! Nothing may panic; outputs keep their caps.

mod common;
use common::fixtures;
use margin_code::flow::{Src, ARG_TEXT, MAX_ARGS};
use margin_code::{outline, Lang};
use std::time::Instant;

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

const INSERTS: &[&str] = &["(", ")", "{", "}", "[", "]", ",", "=", "\"", "'", "/*", "*/", "\n", "x = ", "...", "*", "&", "$", "é", "\0", "\r"];

/// Every function, every call: params, args, flow, taint. Checks the output contract.
fn exercise(lang: Lang, text: &str) {
    let o = outline(lang, text);
    let src = Src::new(lang, text);
    for f in o.fns.iter().take(200) {
        let sig = src.sig(f);
        assert!(sig.names.len() <= 64);
        let tracked: Vec<String> = sig.names.iter().filter(|n| !n.is_empty()).cloned().collect();
        for c in f.calls.iter().take(50) {
            if let Some(r) = src.call_args(c.line, c.col) {
                assert!(r.len() <= MAX_ARGS);
                for (a, b) in r {
                    assert!(a <= b && b <= text.len() && text.is_char_boundary(a) && text.is_char_boundary(b), "{lang:?} {a}..{b}");
                }
            }
            if let Some(args) = src.arg_flow(f, c.line, c.col, &tracked) {
                assert!(args.len() <= MAX_ARGS);
                for a in &args {
                    assert!(a.text.len() <= ARG_TEXT && !a.text.contains('\n'), "{a:?}");
                    assert!(a.from.windows(2).all(|w| w[0] < w[1]), "{a:?}");
                    assert!(a.from.iter().all(|n| tracked.contains(n)), "{a:?} not in {tracked:?}");
                }
            }
            let t = src.taint(f, &tracked, c.line, c.col);
            assert!(t.values().flatten().all(|n| tracked.contains(n)));
        }
    }
}

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
    ]
}

#[test]
fn fixtures_and_mutations() {
    let mut rng = Rng(0x51_7c_c1_b7_27_22_0a_95);
    for (lang, src) in fixtures() {
        exercise(lang, src);
        for _ in 0..40 {
            let mut s = src.to_string();
            for _ in 0..1 + rng.below(10) {
                let cuts: Vec<usize> = (0..=s.len()).filter(|&i| s.is_char_boundary(i)).collect();
                let at = cuts[rng.below(cuts.len())];
                s.insert_str(at, INSERTS[rng.below(INSERTS.len())]);
            }
            exercise(lang, &s);
        }
    }
}

#[test]
fn garbage_and_unbalanced() {
    let mut rng = Rng(99);
    let alphabet: Vec<char> = "abf_ (){}[],;=+-*&$.:<>\"'`/\\\n\té\u{0}".chars().collect();
    for _ in 0..200 {
        let len = rng.below(600);
        let body: String = (0..len).map(|_| alphabet[rng.below(alphabet.len())]).collect();
        for (lang, head) in [
            (Lang::C, "int f(int a, char *b) {\n"),
            (Lang::Python, "def f(a, b):\n    "),
            (Lang::JavaScript, "function f(a, b) {\n"),
            (Lang::Go, "func f(a int, b string) {\n"),
            (Lang::Ruby, "def f(a, b)\n"),
            (Lang::Php, "<?php\nfunction f($a, $b) {\n"),
        ] {
            exercise(lang, &format!("{head}{body}"));
        }
    }
    for t in ["int f(int a) { g(a, (((((( \n", "int f(int a) { g(a, ]]]] ) ; }", "def f(a:\n  g(a,\n", "int f(", "f(a, b) { g(a))))))) }"] {
        for lang in Lang::ALL {
            exercise(lang, t);
        }
    }
}

#[test]
fn huge_lines_are_bounded() {
    // A 1 MB argument list, a 1 MB statement, a 1 MB parameter list.
    let mb = 1 << 20;
    let args = format!("void f(int a) {{\n  g({});\n}}\n", "a, ".repeat(mb / 3));
    let stmt = format!("void f(int a) {{\n  int x = {}a;\n  g(x);\n}}\n", "a + ".repeat(mb / 4));
    let ps = format!("void f({}int z) {{\n  g(z);\n}}\n", "int a, ".repeat(mb / 7));
    let one_line = format!("void f(int a) {{ int x = a; {} g(x); }}", "h(a); ".repeat(mb / 6));
    let t = Instant::now();
    for text in [&args, &stmt, &ps, &one_line] {
        for lang in [Lang::C, Lang::JavaScript, Lang::Python, Lang::Go] {
            exercise(lang, text);
        }
    }
    let s = t.elapsed().as_secs_f64();
    eprintln!("flow on 1 MB lines: {s:.2}s");
    assert!(s < 30.0);
}
