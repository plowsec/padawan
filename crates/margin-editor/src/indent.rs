//! Auto-indent (`EditorOptions::auto_indent`). Pure, no egui. Each trigger is one undo group.
//!
//! | Trigger | Result |
//! |---|---|
//! | Enter | new line starts with the line's leading whitespace, verbatim |
//! | Enter after an opener | + one level (`tab_width` spaces or a tab) |
//! | Enter between `{}` `[]` `()` | indented middle line; the closer on its own line at the outer indent |
//! | `}` `]` `)` typed as the first non-blank char | line re-indented to the opener's line (bounded scan); no match: one level less (code only) |
//! | prose: Enter on `- ` `* ` `+ ` `1. ` `1) ` `- [ ] ` `> ` | next marker (numbers + 1, tasks unchecked) |
//! | prose: Enter on an empty item | marker removed (ends the list), no new line |
//! | off | bare `\n` |
//!
//! | Opener | Languages |
//! |---|---|
//! | `{` `[` `(` at the end (trailing comment ignored) | all |
//! | `:` | Python, YAML |
//! | last word `then` `do` `else` `repeat`; `function …)` | Lua |
//! | last word `do` `then` `else` `begin`, `do \|x\|`; first word `def` `class` `module` `if` … (no `end`) | Ruby |
//! | last word `then` `do` `else` | Shell |

use margin_code::Lang;

use crate::buffer::order;
use crate::ops::{insert, Tabs};
use crate::text::{floor_boundary, indent_len, is_ident};
use crate::undo::{Kind, Sel};
use crate::{Document, Pos};

/// Bracket scan limits for the closer dedent.
const SCAN_LINES: u32 = 5_000;
const SCAN_BYTES: usize = 1 << 20;
/// Lines longer than this are not searched for a trailing comment.
const COMMENT_SCAN: usize = 10_000;

pub fn newline(doc: &mut Document, s: Sel, tabs: &Tabs, auto: bool, time: f64) -> Option<Sel> {
    if !auto {
        return insert(doc, s, "\n", Kind::Other, time);
    }
    let (a, b) = order(s.anchor, s.head);
    let lang = doc.lang();
    let line = doc.line(a.0);
    let col = floor_boundary(line, a.1 as usize);
    let before = &line[..col];
    let tail_line = doc.line(b.0);
    let after = &tail_line[floor_boundary(tail_line, b.1 as usize)..];
    let ind = &line[..indent_len(line).min(col)];

    if lang.is_prose() {
        if let Some(item) = list_item(line) {
            if col >= item.end {
                if a == b && line[item.end..].trim().is_empty() {
                    // Empty item: drop the marker.
                    let l = a.0;
                    let len = line.len() as u32;
                    return doc.group(Kind::Other, time, s, |tx| Sel::caret(tx.edit((l, 0), (l, len), "")));
                }
                let text = format!("\n{}", item.next);
                return insert(doc, s, &text, Kind::Other, time);
            }
        }
    }

    let unit = tabs.unit();
    let pair = before.chars().next_back().zip(after.chars().next()).is_some_and(|(o, c)| closer_of(o) == Some(c));
    if pair {
        let text = format!("\n{ind}{unit}\n{ind}");
        let mid: Pos = (a.0 + 1, (ind.len() + unit.len()) as u32);
        return doc.group(Kind::Other, time, s, |tx| {
            tx.edit(a, b, &text);
            Sel::caret(mid)
        });
    }
    let mut text = format!("\n{ind}");
    if opens_block(lang, before) {
        text.push_str(&unit);
    }
    insert(doc, s, &text, Kind::Other, time)
}

/// `ch` typed with a caret where only whitespace precedes it on the line: re-indent the line.
/// `None`: not applicable (insert normally).
pub fn closer(doc: &mut Document, s: Sel, ch: char, tabs: &Tabs, time: f64) -> Option<Option<Sel>> {
    let opener = opener_of(ch)?;
    if s.anchor != s.head {
        return None;
    }
    let (l, c) = s.head;
    let line = doc.line(l);
    let col = floor_boundary(line, c as usize);
    let lead = &line[..col];
    if !lead.bytes().all(|b| b == b' ' || b == b'\t') {
        return None;
    }
    let target = match opener_indent(doc, l, opener, ch) {
        Some(t) => t,
        None if doc.lang().is_prose() || lead.is_empty() => return None,
        None => {
            let n = if lead.ends_with('\t') { 1 } else { lead.bytes().rev().take(tabs.width.clamp(1, 16) as usize).take_while(|&b| b == b' ').count() };
            lead[..lead.len() - n].to_owned()
        }
    };
    if target == lead {
        return None;
    }
    let text = format!("{target}{ch}");
    let end: Pos = (l, text.len() as u32);
    Some(doc.group(Kind::Other, time, s, |tx| {
        tx.edit((l, 0), (l, col as u32), &text);
        Sel::caret(end)
    }))
}

/// Prose list item / quote on `line`: where the marker ends and the marker for the next line.
pub struct Item {
    pub end: usize,
    pub next: String,
}

pub fn list_item(line: &str) -> Option<Item> {
    let ws = indent_len(line);
    let b = line.as_bytes();
    let mut i = ws;
    // Quotes: `>` with an optional space, repeated.
    while b.get(i) == Some(&b'>') {
        i += 1;
        if b.get(i) == Some(&b' ') {
            i += 1;
        }
    }
    let quote_end = i;
    let mut next = line[..quote_end].to_owned();
    let marker_start = i;
    let marker: Option<String> = match b.get(i) {
        Some(b'-' | b'*' | b'+') => {
            i += 1;
            Some(line[marker_start..i].to_owned())
        }
        Some(c) if c.is_ascii_digit() => {
            let digits = b[i..].iter().take(10).take_while(|c| c.is_ascii_digit()).count();
            let delim = b.get(i + digits).copied();
            if digits <= 9 && matches!(delim, Some(b'.' | b')')) {
                let n: u64 = line[i..i + digits].parse().unwrap_or(0);
                i += digits + 1;
                Some(format!("{}{}", n.saturating_add(1), delim.map_or('.', char::from)))
            } else {
                None
            }
        }
        _ => None,
    };
    match marker {
        Some(m) => {
            let gap = b[i..].iter().take(8).take_while(|&&c| c == b' ' || c == b'\t').count();
            if gap == 0 {
                return finish_quote(line, quote_end, ws, next);
            }
            next.push_str(&m);
            next.push_str(&line[i..i + gap]);
            i += gap;
            // Task box: `[ ]`, `[x]`, `[X]` + space.
            let rest = &line[i..];
            if rest.len() >= 4 && (rest.starts_with("[ ]") || rest.starts_with("[x]") || rest.starts_with("[X]")) && matches!(b.get(i + 3), Some(b' ')) {
                next.push_str("[ ] ");
                i += 4;
            }
            Some(Item { end: i, next })
        }
        None => finish_quote(line, quote_end, ws, next),
    }
}

fn finish_quote(_line: &str, quote_end: usize, ws: usize, next: String) -> Option<Item> {
    (quote_end > ws).then_some(Item { end: quote_end, next })
}

fn closer_of(c: char) -> Option<char> {
    match c {
        '{' => Some('}'),
        '[' => Some(']'),
        '(' => Some(')'),
        _ => None,
    }
}

fn opener_of(c: char) -> Option<char> {
    match c {
        '}' => Some('{'),
        ']' => Some('['),
        ')' => Some('('),
        _ => None,
    }
}

/// Leading whitespace of the line holding the unmatched `open` above line `l` (bounded scan).
fn opener_indent(doc: &Document, l: u32, open: char, close: char) -> Option<String> {
    let mut depth = 0u32;
    let mut bytes = 0usize;
    let lo = l.saturating_sub(SCAN_LINES);
    for k in (lo..l).rev() {
        let line = doc.line(k);
        bytes += line.len() + 1;
        if bytes > SCAN_BYTES {
            return None;
        }
        for ch in line.chars().rev() {
            if ch == close {
                depth += 1;
            } else if ch == open {
                if depth == 0 {
                    return Some(line[..indent_len(line)].to_owned());
                }
                depth -= 1;
            }
        }
    }
    None
}

/// `s` with a trailing line comment removed (quotes respected, cheaply).
fn strip_comment(lang: Lang, s: &str) -> &str {
    let Some(p) = lang.line_comment() else { return s };
    if s.len() > COMMENT_SCAN || !s.contains(p) {
        return s;
    }
    let hash = p == "#";
    let single_quotes = lang != Lang::Rust; // lifetimes
    let mut quote: Option<char> = None;
    let mut esc = false;
    let mut prev = ' ';
    for (i, c) in s.char_indices() {
        if let Some(q) = quote {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == q {
                quote = None;
            }
        } else if c == '"' || c == '`' || (c == '\'' && single_quotes) {
            quote = Some(c);
        } else if s[i..].starts_with(p) && (!hash || prev.is_whitespace()) {
            return &s[..i];
        }
        prev = c;
    }
    s
}

/// Trailing identifier of `t` (`""` when it ends with something else).
fn last_word(t: &str) -> &str {
    let start = t.char_indices().rev().take_while(|&(_, c)| is_ident(c)).last().map_or(t.len(), |(i, _)| i);
    &t[start..]
}

fn first_word(t: &str) -> &str {
    let t = t.trim_start();
    let end = t.char_indices().find(|&(_, c)| !is_ident(c)).map_or(t.len(), |(i, _)| i);
    &t[..end]
}

/// `w` as a whole word in `t`: byte offset of the last occurrence.
fn rfind_word(t: &str, w: &str) -> Option<usize> {
    let mut end = t.len();
    while let Some(i) = t[..end].rfind(w) {
        let before = t[..i].chars().next_back().is_none_or(|c| !is_ident(c));
        let after = t[i + w.len()..].chars().next().is_none_or(|c| !is_ident(c));
        if before && after {
            return Some(i);
        }
        end = i;
    }
    None
}

pub fn opens_block(lang: Lang, before: &str) -> bool {
    let t = strip_comment(lang, before).trim_end();
    if t.ends_with(['{', '[', '(']) {
        return true;
    }
    let last = last_word(t);
    match lang {
        Lang::Python | Lang::Yaml => t.ends_with(':'),
        Lang::Lua => matches!(last, "then" | "do" | "else" | "repeat") || (t.ends_with(')') && rfind_word(t, "function").is_some_and(|i| rfind_word(&t[i..], "end").is_none())),
        Lang::Ruby => {
            let block_arg = t.ends_with('|') && (t.contains(" do |") || t.contains(" do|"));
            let lead = matches!(first_word(t), "def" | "class" | "module" | "if" | "unless" | "while" | "until" | "case" | "for" | "begin" | "elsif" | "else" | "when" | "rescue" | "ensure");
            matches!(last, "do" | "then" | "else" | "begin") || block_arg || (lead && rfind_word(t, "end").is_none())
        }
        Lang::Shell => matches!(last, "then" | "do" | "else"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOFT: Tabs = Tabs { width: 4, soft: true };
    const HARD: Tabs = Tabs { width: 4, soft: false };

    /// Enter at the end of the last line of `text`; returns the new text and cursor.
    fn enter(text: &str, lang: Lang, tabs: &Tabs) -> (String, Pos) {
        let mut d = Document::new(text, lang);
        let end = d.buf.end_pos();
        let s = newline(&mut d, Sel::caret(end), tabs, true, 0.0).unwrap();
        (d.text(), s.head)
    }

    fn opens(lang: Lang, t: &str) -> bool {
        opens_block(lang, t)
    }

    #[test]
    fn keeps_indent_verbatim() {
        assert_eq!(enter("\t  x", Lang::Rust, &SOFT).0, "\t  x\n\t  ");
        assert_eq!(enter("  x", Lang::Plain, &SOFT).0, "  x\n  ");
        // Cursor inside the leading whitespace: only what is before it.
        let mut d = Document::new("    ab", Lang::Rust);
        newline(&mut d, Sel::caret((0, 2)), &SOFT, true, 0.0);
        assert_eq!(d.text(), "  \n    ab");
        // Off: bare newline.
        let mut d = Document::new("    x {", Lang::Rust);
        let s = newline(&mut d, Sel::caret((0, 7)), &SOFT, false, 0.0).unwrap();
        assert_eq!((d.text().as_str(), s.head), ("    x {\n", (1, 0)));
    }

    #[test]
    fn c_like() {
        assert_eq!(enter("    fn x() {", Lang::Rust, &SOFT), ("    fn x() {\n        ".into(), (1, 8)));
        assert_eq!(enter("\tif (x) {", Lang::C, &HARD).0, "\tif (x) {\n\t\t");
        assert_eq!(enter("let a = [", Lang::JavaScript, &SOFT).0, "let a = [\n    ");
        assert_eq!(enter("call(", Lang::Go, &SOFT).0, "call(\n    ");
        assert!(opens(Lang::Rust, "if x {   // note"));
        assert!(opens(Lang::Rust, "fn f<'a>(x: &'a str) { // c"));
        assert!(!opens(Lang::Rust, r#"let u = "http://x{"; // c"#));
        assert!(opens(Lang::Rust, r#"let u = "//"; f(x, {"#));
        assert!(!opens(Lang::Rust, "x = 1; // {"));
        assert!(!opens(Lang::JavaScript, "f(x) => x"));
        assert!(!opens(Lang::Kotlin, "val f = { x -> x }"));
        assert!(opens(Lang::JavaScript, "const f = (x) => {"));
        assert!(!opens(Lang::Kotlin, "list.map { x ->"), "ends with ->, no brace: no level");
        assert!(!opens(Lang::Rust, "x:"), ": only in Python / YAML");
    }

    #[test]
    fn python_yaml() {
        assert_eq!(enter("    if x:", Lang::Python, &SOFT).0, "    if x:\n        ");
        assert!(opens(Lang::Python, "def f(a):  # comment"));
        assert!(opens(Lang::Python, "d = {"));
        assert!(!opens(Lang::Python, "x = 'a:'  # ok"));
        assert!(opens(Lang::Yaml, "key:"));
        assert!(opens(Lang::Yaml, "- name: # c"));
        assert!(!opens(Lang::Yaml, "key: value"));
        assert!(!opens(Lang::Markdown, "Steps:"));
    }

    #[test]
    fn lua_ruby_shell() {
        for t in ["if x then", "for i = 1, 3 do", "else", "repeat", "local function f(a, b)", "x = function()", "while true do -- loop"] {
            assert!(opens(Lang::Lua, t), "lua {t:?}");
        }
        for t in ["f(function() end)", "x = 1", "until done", "print(x)"] {
            assert!(!opens(Lang::Lua, t), "lua {t:?}");
        }
        for t in ["def f(x)", "class A < B", "module M", "items.each do |x|", "items.each do", "begin", "if x", "case y", "x.map {"] {
            assert!(opens(Lang::Ruby, t), "ruby {t:?}");
        }
        for t in ["def f; end", "x = 1 if y", "puts 'do'", "if x then y end"] {
            assert!(!opens(Lang::Ruby, t), "ruby {t:?}");
        }
        for t in ["if [ -f x ]; then", "for f in *; do", "else", "f() {"] {
            assert!(opens(Lang::Shell, t), "sh {t:?}");
        }
        for t in ["echo done", "fi", "echo ${#x}"] {
            assert!(!opens(Lang::Shell, t), "sh {t:?}");
        }
        assert_eq!(enter("  if x then", Lang::Lua, &HARD).0, "  if x then\n  \t");
    }

    #[test]
    fn bracket_pair_split() {
        for (open, close) in [('{', '}'), ('[', ']'), ('(', ')')] {
            let t = format!("  f{open}{close}");
            let mut d = Document::new(&t, Lang::Rust);
            let s = newline(&mut d, Sel::caret((0, 4)), &SOFT, true, 0.0).unwrap();
            assert_eq!(d.text(), format!("  f{open}\n      \n  {close}"));
            assert_eq!(s.head, (1, 6));
            // One undo step.
            assert_eq!(d.undo_group().map(|s| s.head), Some((0, 4)));
            assert_eq!(d.text(), t);
        }
        // Mismatched pair: no split.
        let mut d = Document::new("(]", Lang::Rust);
        newline(&mut d, Sel::caret((0, 1)), &SOFT, true, 0.0);
        assert_eq!(d.text(), "(\n    ]");
    }

    #[test]
    fn closer_dedents() {
        let run = |text: &str, at: Pos, ch: char, lang: Lang| {
            let mut d = Document::new(text, lang);
            let r = closer(&mut d, Sel::caret(at), ch, &SOFT, 0.0);
            (d.text(), r.map(|s| s.map(|s| s.head)))
        };
        // Matches the opener's line.
        let (t, r) = run("fn a() {\n    if x {\n    }\n    ", (3, 4), '}', Lang::Rust);
        assert_eq!((t.as_str(), r), ("fn a() {\n    if x {\n    }\n}", Some(Some((3, 1)))));
        let (t, _) = run("\tlet v = [\n\t\t1,\n\t\t", (2, 2), ']', Lang::Rust);
        assert_eq!(t, "\tlet v = [\n\t\t1,\n\t]");
        // No opener: one level less (code); prose: untouched.
        let (t, _) = run("x\n        ", (1, 8), ')', Lang::C);
        assert_eq!(t, "x\n    )");
        assert_eq!(run("x\n        ", (1, 8), ')', Lang::Markdown).1, None);
        // Already right, text before the cursor, selection, other chars: not ours.
        assert_eq!(run("{\n", (1, 0), '}', Lang::Rust).1, None);
        assert_eq!(run("{\n    x", (1, 5), '}', Lang::Rust).1, None);
        assert_eq!(run("{\n    ", (1, 4), 'x', Lang::Rust).1, None);
        let mut d = Document::new("{\n    ", Lang::Rust);
        assert!(closer(&mut d, Sel { anchor: (1, 0), head: (1, 4) }, '}', &SOFT, 0.0).is_none());
        // Read-only: applicable but no edit.
        let mut d = Document::new("{\n    ", Lang::Rust);
        d.set_read_only(true);
        assert_eq!(closer(&mut d, Sel::caret((1, 4)), '}', &SOFT, 0.0), Some(None));
        assert_eq!(d.text(), "{\n    ");
    }

    #[test]
    fn markdown_lists() {
        assert_eq!(enter("- item", Lang::Markdown, &SOFT), ("- item\n- ".into(), (1, 2)));
        assert_eq!(enter("  * a", Lang::Markdown, &SOFT).0, "  * a\n  * ");
        assert_eq!(enter("+ a", Lang::Plain, &SOFT).0, "+ a\n+ ");
        assert_eq!(enter("9. nine", Lang::Markdown, &SOFT).0, "9. nine\n10. ");
        assert_eq!(enter("1) one", Lang::Markdown, &SOFT).0, "1) one\n2) ");
        assert_eq!(enter("- [x] done", Lang::Markdown, &SOFT).0, "- [x] done\n- [ ] ");
        assert_eq!(enter("> quoted", Lang::Markdown, &SOFT).0, "> quoted\n> ");
        assert_eq!(enter("> - in quote", Lang::Markdown, &SOFT).0, "> - in quote\n> - ");
        // Empty item: marker removed, no new line.
        for t in ["- ", "  - ", "3. ", "- [ ] ", "> "] {
            let mut d = Document::new(&format!("- a\n{t}"), Lang::Markdown);
            let end = d.buf.end_pos();
            let s = newline(&mut d, Sel::caret(end), &SOFT, true, 0.0).unwrap();
            assert_eq!((d.text().as_str(), s.head), ("- a\n", (1, 0)), "{t:?}");
        }
        // Not items.
        for t in ["-x", "**bold**", "1.5 apples", "---x", "text"] {
            assert!(list_item(t).is_none(), "{t:?}");
        }
        // Enter inside an item splits it under a new marker.
        let mut d = Document::new("- ab", Lang::Markdown);
        newline(&mut d, Sel::caret((0, 3)), &SOFT, true, 0.0);
        assert_eq!(d.text(), "- a\n- b");
        // Cursor inside the marker: plain indent rules.
        let mut d = Document::new("  - ab", Lang::Markdown);
        newline(&mut d, Sel::caret((0, 1)), &SOFT, true, 0.0);
        assert_eq!(d.text(), " \n  - ab");
        // Huge numbers do not overflow.
        assert_eq!(list_item("999999999. x").map(|i| i.next), Some("1000000000. ".to_owned()));
        assert!(list_item("9999999999. x").is_none());
    }

    #[test]
    fn hostile_lines_do_not_panic() {
        for t in ["é{", "\u{202E}(", "-", ">", ">>>>", "1", "[ ]", "- [", "#{", "'", "\"//", "世界:"] {
            for lang in [Lang::Rust, Lang::Python, Lang::Markdown, Lang::Ruby, Lang::Lua, Lang::Shell] {
                let mut d = Document::new(t, lang);
                for c in 0..=t.len() as u32 {
                    let _ = newline(&mut d, Sel::caret((0, c)), &SOFT, true, 0.0);
                    let _ = closer(&mut d, Sel::caret((0, c)), '}', &SOFT, 0.0);
                }
            }
        }
    }
}
