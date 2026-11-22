//! Hostile input: deterministic fuzz and pathological shapes. Never panics; each case well under a
//! second in a debug build.

use std::time::{Duration, Instant};

use margin_md::{parse, MAX_SOURCE};

const SAMPLE: &str = include_str!("sample.md");

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const PIECES: &[&str] = &[
    "*", "**", "_", "__", "~~", "`", "```", "~~~", "[", "]", "(", ")", "![", "<", ">", "&amp;", "&#", "\\", "\n", "\n\n", "  \n",
    "- ", "1. ", "> ", "# ", "| ", "|---|", ":-:", "[^1]", "[^1]: ", "[x]: /u", "<!--", "-->", "<div>", "</div>", "http://a.b",
    "    ", "\t", "[ ] ", "[x] ", "---", "===", "\u{202e}", "\u{200b}", "é", "世", "\u{1F600}", "\r", "\0",
];

fn check(s: &str) {
    let t0 = Instant::now();
    let doc = parse(s);
    let _ = doc.dump();
    let _ = doc.headings();
    let _ = doc.anchor_line("#a");
    assert!(t0.elapsed() < Duration::from_secs(2), "slow on {} bytes", s.len());
}

#[test]
fn fuzz_mutations_of_the_sample() {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..400 {
        let mut s = SAMPLE.to_owned();
        for _ in 0..1 + r.below(12) {
            match r.below(3) {
                0 => {
                    let mut at = r.below(s.len());
                    while !s.is_char_boundary(at) {
                        at -= 1;
                    }
                    s.truncate(at);
                }
                1 => {
                    let mut at = r.below(s.len() + 1);
                    while !s.is_char_boundary(at) {
                        at -= 1;
                    }
                    let p = PIECES[r.below(PIECES.len())];
                    s.insert_str(at, p);
                }
                _ => {
                    let a = r.below(s.len());
                    let b = (a + r.below(64)).min(s.len());
                    if s.is_char_boundary(a) && s.is_char_boundary(b) {
                        s.replace_range(a..b, "");
                    }
                }
            }
        }
        check(&s);
    }
}

#[test]
fn fuzz_random_pieces_and_chars() {
    let mut r = Rng(42);
    for _ in 0..300 {
        let mut s = String::new();
        for _ in 0..r.below(400) {
            if r.below(4) == 0 {
                let c = char::from_u32((r.next() % 0x11_0000) as u32).unwrap_or('?');
                s.push(c);
            } else {
                s.push_str(PIECES[r.below(PIECES.len())]);
            }
        }
        check(&s);
    }
}

fn timed(name: &str, s: &str) {
    let t0 = Instant::now();
    let doc = parse(s);
    let el = t0.elapsed();
    let _ = doc.dump();
    eprintln!("{name:<28} {:>8} bytes  {:>8.1} ms  {} blocks", s.len(), el.as_secs_f64() * 1000.0, doc.block_count());
    assert!(el < Duration::from_millis(900), "{name}: {el:?}");
}

#[test]
fn pathological_inputs() {
    const MB: usize = 1 << 20;
    timed("1 MB of *", &"*".repeat(MB));
    timed("1 MB of _ a", &"_a".repeat(MB / 2));
    timed("*a_ alternating", &"*a_".repeat(MB / 3));
    timed("**_ nested openers", &"**_".repeat(MB / 6).chars().chain("x".chars()).chain("_**".repeat(MB / 6).chars()).collect::<String>());
    timed("1 MB of [", &"[".repeat(MB));
    timed("1 MB of ]", &"]".repeat(MB));
    timed("[]( repeated", &"[](".repeat(MB / 3));
    timed("[a](b repeated", &"[a](b ".repeat(MB / 6));
    timed("![ repeated", &"![".repeat(MB / 2));
    timed("1 MB of >", &">".repeat(MB));
    timed("> lines", &"> ".repeat(64).repeat(MB / 128).replace(&"> ".repeat(64), &("> ".repeat(64) + "\n")));
    timed("1 MB of backticks", &"`".repeat(MB));
    timed("backtick run ladder", &(1..1400).map(|k| "`".repeat(k) + "x").collect::<String>());
    timed("backticks per line", &"` a\n".repeat(MB / 4));
    timed("1 MB of <", &"<".repeat(MB));
    timed("<a repeated", &"<a ".repeat(MB / 3));
    timed("<!-- repeated", &"<!--".repeat(MB / 4));
    timed("&# repeated", &"&#".repeat(MB / 2));
    timed("1 MB of \\", &"\\".repeat(MB));
    timed("1 MB single line", &"word ".repeat(MB / 5));
    timed("100k - on one line", &"- ".repeat(100_000));
    timed("100k nested list lines", &(0..100_000).map(|i| format!("{}- x\n", "  ".repeat(i % 20))).collect::<String>());
    timed("100k nested quote lines", &(0..100_000).map(|i| format!("{}x\n", "> ".repeat(i % 20))).collect::<String>());
    timed("2 MB table row", &format!("{}\n|---|---|\n{}\n", "| a ".repeat(2 * MB / 4), "| b ".repeat(2 * MB / 4)));
    timed("table 2-col, long rows", &format!("| a | b |\n|---|---|\n{}", format!("| {} | x |\n", "y".repeat(10_000)).repeat(100)));
    timed("many tables rows", &format!("| a |\n|---|\n{}", "| r |\n".repeat(200_000)));
    timed("1 MB code line", &format!("```\n{}\n```\n", "x".repeat(MB)));
    timed("100k fence lines", &"```\n".repeat(100_000));
    timed("200k paragraphs", &"p\n\n".repeat(200_000));
    timed("300k blocks (cap)", &"# h\n".repeat(300_000));
    timed("ref defs", &(0..50_000).map(|i| format!("[r{i}]: /u{i}\n")).collect::<String>());
    timed("ref uses", &"[r][r] ".repeat(MB / 7));
    timed("footnote refs", &("[^1]: x\n\n".to_owned() + &"[^1]".repeat(MB / 4)));
    timed("bare urls", &"http://a.b/c ".repeat(MB / 13));
    timed("emphasis ladder", &(0..30_000).map(|_| "*a ").collect::<String>());
}

#[test]
fn input_is_cut_at_max_source() {
    let line = "abcdefghij".repeat(10) + "\n";
    let s = line.repeat(MAX_SOURCE / line.len() + 100);
    let t0 = Instant::now();
    let doc = parse(&s);
    assert!(doc.truncated());
    eprintln!("{} MB paragraph text: {:.1} ms", s.len() >> 20, t0.elapsed().as_secs_f64() * 1000.0);
}
