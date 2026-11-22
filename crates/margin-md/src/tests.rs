//! Parser tests: compact dumps per construct (`line [containers] kind content`), slugs, source lines.

use crate::parse;

fn d(s: &str) -> String {
    parse(s).dump()
}

#[track_caller]
fn eq(src: &str, want: &str) {
    let got = d(src);
    let want: String = want.lines().map(|l| l.trim_start()).filter(|l| !l.is_empty()).map(|l| format!("{l}\n")).collect();
    assert_eq!(got, want, "\n--- source ---\n{src}");
}

#[test]
fn atx_headings() {
    eq("# One\n## Two ##\n###### Six\n####### seven\n#nospace\n# \n", "
        0 h1 One
        1 h2 Two
        2 h6 Six
        3 p ####### seven #nospace
        5 h1
    ");
    eq("### a # b ###   \n", "0 h3 a # b");
}

#[test]
fn setext_headings() {
    eq("Title\n=====\n\nSub\ntitle\n---\n", "
        0 h1 Title
        3 h2 Sub title
    ");
    // Not setext: underline after a blank line is a rule.
    eq("Para\n\n---\n", "0 p Para\n2 hr");
}

#[test]
fn paragraphs_and_breaks() {
    eq("one\ntwo  \nthree\\\nfour\n\nnext\n", "
        0 p one two\\nthree\\nfour
        5 p next
    ");
    eq("   indented para\n", "0 p indented para");
}

#[test]
fn emphasis_strike_code() {
    eq("*a* **b** ***c*** ~~d~~ `e` ``f`g``\n", "0 p *a* **b** ***c*** ~~d~~ `e` `f`g`");
}

#[test]
fn thematic_breaks() {
    eq("***\n- - -\n___\n", "0 hr\n1 hr\n2 hr");
}

#[test]
fn links_refs_images() {
    eq("[a][x] [x] ![i](p.png \"t\")\n\n[x]: /dest \"Title\"\n", "0 p [a](/dest) [x](/dest) {img:i|p.png}");
    // Definitions alone produce no block; first definition wins.
    eq("[y]: /one\n[y]: /two\n\n[y]\n", "3 p [y](/one)");
    // Definition with the title on the next line.
    eq("[z]:\n  /zz\n  'tt'\n\n[z]\n", "4 p [z](/zz)");
}

#[test]
fn bullet_lists_nested_tight() {
    eq("- a\n- b\n  - c\n    * d\n- e\n", "
        0 [ul:t li] p a
        1 [ul:t li] p b
        2 [ul:t li ul:t li] p c
        3 [ul:t li ul:t li ul:t li] p d
        4 [ul:t li] p e
    ");
    // A different bullet char starts a new list.
    eq("- a\n+ b\n", "0 [ul:t li] p a\n1 [ul:t li] p b");
}

#[test]
fn ordered_lists() {
    eq("3. x\n4. y\n\n1) z\n", "
        0 [ol3:t li] p x
        1 [ol3:t li] p y
        3 [ol1:t li] p z
    ");
    // Only `1.` interrupts a paragraph.
    eq("para\n2. not a list\n", "0 p para 2. not a list");
    eq("para\n1. a list\n", "0 p para\n1 [ol1:t li] p a list");
}

#[test]
fn loose_lists_and_continuations() {
    eq("- a\n\n- b\n", "0 [ul:l li] p a\n2 [ul:l li] p b");
    eq("- a\n\n  more\n- b\n", "0 [ul:l li] p a\n2 [ul:l li] p more\n3 [ul:l li] p b");
    // Blank lines inside a nested list only loosen that list.
    eq("- a\n  - b\n\n  - c\n- d\n", "
        0 [ul:t li] p a
        1 [ul:t li ul:l li] p b
        3 [ul:t li ul:l li] p c
        4 [ul:t li] p d
    ");
    // Code inside an item; lazy continuation.
    eq("- item\n\n      code\n- x\nlazy\n", "
        0 [ul:l li] p item
        2 [ul:l li] code \"code\"
        3 [ul:l li] p x lazy
    ");
    eq("-\n- \n", "0 [ul:t li] empty\n1 [ul:t li] empty");
}

#[test]
fn task_items() {
    eq("- [ ] todo\n- [x] done\n- [X] Done\n- [y] no\n- [ ]\n", "
        0 [ul:t li[ ]] p todo
        1 [ul:t li[x]] p done
        2 [ul:t li[x]] p Done
        3 [ul:t li] p [y] no
        4 [ul:t li[ ]] empty
    ");
}

#[test]
fn block_quotes() {
    eq("> a\nlazy\n> > b\n>\n> - c\n", "
        0 [>] p a lazy
        2 [> >] p b
        4 [> ul:t li] p c
    ");
    eq("> # H\n> ```\n> code\n> ```\nafter\n", "
        0 [>] h1 H
        1 [>] code \"code\"
        4 p after
    ");
}

#[test]
fn alerts() {
    eq("> [!NOTE]\n> Body *x*.\n\n> [!caution]\n> Careful.\n\n> [!BOGUS]\n> plain\n", "
        0 [>!Note] title Note
        1 [>!Note] p Body *x*.
        3 [>!Caution] title Caution
        4 [>!Caution] p Careful.
        6 [>] p [!BOGUS] plain
    ");
}

#[test]
fn fenced_and_indented_code() {
    eq("```rust title\nfn a() {}\n\n  x\n```\n~~~~\n```\nstill\n~~~~\n", "
        0 code:rust \"fn a() {}\\n\\n  x\"
        5 code \"```\\nstill\"
    ");
    eq("  ```\n  a\n b\nc\n  ```\n", "0 code \"a\\nb\\nc\"");
    eq("    one\n\n    two\n\nend\n", "0 code \"one\\n\\ntwo\"\n4 p end");
    // Unclosed fence runs to the end; backtick fence info may not contain backticks.
    eq("```\nopen\n", "0 code \"open\"");
    eq("``` a`b\n", "0 p ``` a`b");
    eq("text\n    not code\n", "0 p text not code");
}

#[test]
fn tables() {
    eq("| a | b | c |\n|:--|:-:|--:|\n| 1 | **2** | `3\\|4` |\n| only |\n", "
        0 table lcr |a|b|c| |1|**2**|`3|4`| |only|||
    ");
    eq("a | b\n--- | ---\nx | y\n\nafter\n", "0 table -- |a|b| |x|y|\n4 p after");
    // Header and delimiter must agree; no pipe → not a table.
    eq("| a | b |\n| --- |\n", "0 p | a | b | | --- |");
    // Paragraph before the header line stays a paragraph.
    eq("intro\n| h |\n| - |\n| v |\n", "0 p intro\n1 table - |h| |v|");
    // A table ends at another block.
    eq("| h |\n|---|\n| v |\n# next\n", "0 table - |h| |v|\n3 h1 next");
}

#[test]
fn long_tables_and_code_are_chunked() {
    let mut s = String::from("| n |\n|---|\n");
    for i in 0..70 {
        s.push_str(&format!("| {i} |\n"));
    }
    let doc = parse(&s);
    let dump = doc.dump();
    let lines: Vec<&str> = dump.lines().collect();
    assert_eq!(lines.len(), 3, "{dump}");
    assert!(lines[0].starts_with("0 table -"));
    assert!(lines[1].starts_with("33 table |31|"), "{}", lines[1]);
    let mut c = String::from("```py\n");
    for i in 0..130 {
        c.push_str(&format!("x = {i}\n"));
    }
    c.push_str("```\n");
    assert_eq!(parse(&c).block_count(), 3);
}

#[test]
fn raw_html() {
    eq("<div>\n*not md*\n</div>\n\npara\n", "
        0 raw:html \"<div>\\n*not md*\\n</div>\"
        4 p para
    ");
    eq("<!-- c\nd -->\nafter\n", "0 raw:html \"<!-- c\\nd -->\"\n2 p after");
    eq("<script>\nx\n</script>\n", "0 raw:html \"<script>\\nx\\n</script>\"");
    eq("a <i>b</i> <!-- x --> c<br/>d\n", "0 p a {html:<i>}b{html:</i>} c\\nd");
    // Type 7 cannot interrupt a paragraph.
    eq("para\n<custom-tag>\n", "0 p para {html:<custom-tag>}");
}

#[test]
fn front_matter() {
    eq("---\na: 1\n---\n# T\n", "0 raw:front matter \"a: 1\"\n3 h1 T");
    // Not at the top or not closed: ordinary markdown.
    eq("x\n---\na: 1\n---\n", "0 h2 x\n2 h2 a: 1");
    eq("---\nno close\n", "0 hr\n1 p no close");
}

#[test]
fn footnotes() {
    eq("Text[^n] and[^missing].\n\n[^n]: The note\n    continues.\n\nAfter.\n", "
        0 p Text^1 and[^missing].
        5 p After.
        2 hr
        2 [fn1] p The note continues.
    ");
}

#[test]
fn entities_and_escapes() {
    eq("&amp; &nbsp; &#123; &#x1F600; &bad; \\# \\[x\\]\n", "0 p & \u{a0} { \u{1F600} &bad; # [x]");
}

#[test]
fn headings_and_slugs() {
    let doc = parse("# Intro\n\n## Intro\n\n## Hello, *World*!\n\nSetext\n===\n\n# Intro\n");
    assert_eq!(
        doc.headings(),
        vec![(1, "Intro".into(), 0), (2, "Intro".into(), 2), (2, "Hello, World!".into(), 4), (1, "Setext".into(), 6), (1, "Intro".into(), 9)]
    );
    assert_eq!(doc.anchor_line("#intro"), Some(0));
    assert_eq!(doc.anchor_line("intro-1"), Some(2));
    assert_eq!(doc.anchor_line("#intro-2"), Some(9));
    assert_eq!(doc.anchor_line("#hello-world"), Some(4));
    assert_eq!(doc.anchor_line("#Hello-World"), Some(4));
    assert_eq!(doc.anchor_line("#setext"), Some(6));
    assert_eq!(doc.anchor_line("#nope"), None);
    let doc = parse("# Café Ünïcode\n");
    assert_eq!(doc.anchor_line("#caf%C3%A9-%C3%BCn%C3%AFcode"), Some(0));
}

#[test]
fn source_lines_map_to_blocks() {
    let src = "# A\n\npara\nmore\n\n- x\n- y\n\n```\nc\n```\n";
    let doc = parse(src);
    let inner = &doc.inner;
    let lines: Vec<u32> = inner.leaves.iter().map(|l| l.line).collect();
    assert_eq!(lines, vec![0, 2, 5, 6, 8]);
    for (line, leaf) in [(0, 0), (1, 0), (2, 1), (3, 1), (5, 2), (6, 3), (9, 4), (99, 4)] {
        assert_eq!(inner.leaf_at_line(line), leaf, "line {line}");
    }
}

#[test]
fn crlf_and_tabs() {
    eq("# T\r\n\r\n-\tx\r\n\tcode\r\n", "0 h1 T\n2 [ul:t li] p x code");
    eq("\tcode\n", "0 code \"code\"");
}

#[test]
fn nesting_is_capped() {
    let s = ">".repeat(100) + " deep\n";
    let dump = d(&s);
    assert_eq!(dump.matches('>').count(), 16 + 84, "{dump}");
    let s = "- ".repeat(100) + "x\n";
    let doc = parse(&s);
    assert!(doc.dump().lines().count() == 1);
}

#[test]
fn sample_parses_fully() {
    let doc = parse(include_str!("../tests/sample.md"));
    let dump = doc.dump();
    for want in [
        "0 raw:front matter",
        "5 h1 Lighthouse Rewrite",
        "[ul:t li ul:t li ul:t li] p Warm white",
        "[ol7:t li] p Start counting at seven",
        "[ul:t li[x]] p Order spare bulbs",
        "[ul:l li] code:sh",
        "[> >] p Nested quotes work as well.",
        "[>!Warning] title Warning",
        "code:rust",
        "table lcr- |Part|",
        "h2 Setext heading",
        "raw:html",
        "[fn1] p The lamp",
    ] {
        assert!(dump.contains(want), "missing {want:?} in\n{dump}");
    }
    assert!(!doc.truncated());
}
