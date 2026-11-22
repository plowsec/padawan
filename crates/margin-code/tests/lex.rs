//! Lexer fixtures per language (invented code). `lex_text` also checks the token contract and that relexing
//! a line with the same state gives the same result.

mod common;
use common::*;
use margin_code::TokenKind::*;
use margin_code::{lex_line, Lang, LineState};

#[test]
fn c_tokens() {
    let src = "#include <stdio.h>\n#define MAX_LEN 16\nstatic int add(int a, size_t b) { /* sum */\n  char c = '\\n'; return a + b + 0x1Fu; // done\n}\nconst char *s = L\"wide\\t\";";
    let sp = spans(Lang::C, src);
    has(&sp, "#include", Attribute);
    has(&sp, "<stdio.h>", String);
    has(&sp, "#define", Attribute);
    has(&sp, "MAX_LEN", Constant);
    has(&sp, "static", Keyword);
    has(&sp, "int", Type);
    has(&sp, "size_t", Type);
    has(&sp, "add", Function);
    has(&sp, "/* sum */", Comment);
    has(&sp, "\\n", Escape);
    has(&sp, "0x1Fu", Number);
    has(&sp, "// done", Comment);
    has(&sp, "L\"wide", String);
    has(&sp, "\\t", Escape);
}

#[test]
fn c_block_comment_spans_lines() {
    let src = "int a; /* start\n still comment foo(\n end */ int b;";
    let lines = lex_text(Lang::C, src);
    assert_ne!(lines[1].1, LineState::default(), "line 2 starts inside the comment");
    let sp = spans(Lang::C, src);
    has(&sp, " still comment foo(", Comment);
    has(&sp, " end */", Comment);
    not_kind(&sp, "foo", Function);
    has(&sp, "int", Type);
}

#[test]
fn cpp_tokens() {
    let src = "namespace n { template <typename T> class Box { public: T get() const noexcept; }; }\nauto s = R\"xy(raw \" text\n)\" still raw)xy\"; int v = 1'000;\nstd::string t = u8\"x\";";
    let lines = lex_text(Lang::Cpp, src);
    assert_ne!(lines[2].1, LineState::default(), "inside the raw string");
    let sp = spans(Lang::Cpp, src);
    has(&sp, "namespace", Keyword);
    has(&sp, "template", Keyword);
    has(&sp, "typename", Keyword);
    has(&sp, "get", Function);
    has(&sp, "noexcept", Keyword);
    has(&sp, ")\" still raw)xy\"", String);
    has(&sp, "1'000", Number);
    has(&sp, "u8\"x\"", String);
}

#[test]
fn rust_tokens() {
    let src = "#[derive(Debug)]\npub struct P<'a> { s: &'a str }\nfn f<'b>(c: char) -> Option<u8> { let x = 'x'; let e = '\\u{1F600}'; println!(\"{}\", x); None }\nlet r = r#\"raw \"quoted\" \"#; let b = b\"bytes\"; let n = 1_000u32;\n/* outer /* inner */ still */ let after = true;";
    let sp = spans(Lang::Rust, src);
    has(&sp, "#[derive(Debug)]", Attribute);
    has(&sp, "'a", Type);
    has(&sp, "'b", Type);
    has(&sp, "'x'", String);
    has(&sp, "\\u{1F600}", Escape);
    has(&sp, "println!", Attribute);
    has(&sp, "Option", Type);
    has(&sp, "u8", Type);
    has(&sp, "None", Constant);
    has(&sp, "r#\"raw \"quoted\" \"#", String);
    has(&sp, "b\"bytes\"", String);
    has(&sp, "1_000u32", Number);
    has(&sp, "/* outer /* inner */ still */", Comment);
    has(&sp, "true", Constant);
    not_kind(&sp, "f", Function); // `f<'b>(`: not directly followed by `(`
}

#[test]
fn rust_multiline_raw_string_and_nested_comment() {
    let src = "let s = r##\"line one \"# not end\nline two\"##; /* a /* b\n*/ still comment\n*/ fn g() {}";
    let lines = lex_text(Lang::Rust, src);
    assert_ne!(lines[1].1, LineState::default());
    assert_ne!(lines[2].1, LineState::default());
    assert_ne!(lines[3].1, LineState::default(), "depth 1 remains after one */");
    let sp = spans(Lang::Rust, src);
    has(&sp, "line two\"##", String);
    has(&sp, "*/ still comment", Comment);
    has(&sp, "fn", Keyword);
    has(&sp, "g", Function);
}

#[test]
fn rust_multiline_plain_string() {
    let src = "let s = \"first\nsecond\"; let t = 1;";
    let sp = spans(Lang::Rust, src);
    has(&sp, "second\"", String);
    has(&sp, "let", Keyword);
}

#[test]
fn go_tokens() {
    let src = "package main\nfunc (s *Srv) Run(ctx context.Context) error {\n\tq := `raw\nmulti`\n\treturn nil\n}";
    let lines = lex_text(Lang::Go, src);
    assert_ne!(lines[3].1, LineState::default());
    let sp = spans(Lang::Go, src);
    has(&sp, "func", Keyword);
    has(&sp, "Srv", Type);
    has(&sp, "Run", Function);
    has(&sp, "error", Type);
    has(&sp, "`raw", String);
    has(&sp, "multi`", String);
    has(&sp, "nil", Constant);
}

#[test]
fn java_tokens() {
    let src = "@Override\npublic List<String> names(int n) throws IOException {\n  String t = \"\"\"\n    text block\n    \"\"\";\n  return new ArrayList<>(n);\n}";
    let sp = spans(Lang::Java, src);
    has(&sp, "@Override", Attribute);
    has(&sp, "public", Keyword);
    has(&sp, "List", Type);
    has(&sp, "names", Function);
    has(&sp, "throws", Keyword);
    has(&sp, "IOException", Type);
    has(&sp, "    text block", String);
    has(&sp, "new", Keyword);
}

#[test]
fn js_tokens() {
    let src = "const re = /a\\/b[/]c/gi; const n = a / b / c;\nlet t = `hi ${name + `in`} and ${\n  obj.call(1)\n} done`;\nconst s = 'q' + \"d\"; // end\nexport default async function* gen() { yield null; }";
    let lines = lex_text(Lang::JavaScript, src);
    assert_ne!(lines[2].1, LineState::default(), "inside ${{ … }}");
    assert_ne!(lines[3].1, LineState::default());
    let sp = spans(Lang::JavaScript, src);
    has(&sp, "/a\\/b[/]c/gi", String);
    has(&sp, "/", Operator);
    has(&sp, "`hi ", String);
    has(&sp, "${", Punct);
    has(&sp, "`in`", String);
    has(&sp, "call", Function);
    has(&sp, " done`", String);
    has(&sp, "'q'", String);
    has(&sp, "\"d\"", String);
    has(&sp, "// end", Comment);
    has(&sp, "function", Keyword);
    has(&sp, "null", Constant);
}

#[test]
fn ts_tokens() {
    let src = "interface Shape { area(): number }\n@Component({})\nclass Circle implements Shape { constructor(private r: number) {} area(): number { return Math.PI * this.r ** 2; } }";
    let sp = spans(Lang::TypeScript, src);
    has(&sp, "interface", Keyword);
    has(&sp, "Shape", Type);
    has(&sp, "number", Type);
    has(&sp, "@Component", Attribute);
    has(&sp, "implements", Keyword);
    has(&sp, "private", Keyword);
    has(&sp, "PI", Constant);
    has(&sp, "area", Function);
}

#[test]
fn csharp_tokens() {
    let src = "#region Io\nusing System;\nvar p = @\"C:\\dir\n\"\"quoted\"\" end\";\nvar i = $\"v={x}\";\npublic async Task<int> Run() => await Go();";
    let lines = lex_text(Lang::CSharp, src);
    assert_ne!(lines[3].1, LineState::default(), "verbatim string continues");
    let sp = spans(Lang::CSharp, src);
    has(&sp, "#region", Attribute);
    has(&sp, "using", Keyword);
    has(&sp, "@\"C:\\dir", String);
    has(&sp, "\"\"quoted\"\" end\"", String);
    has(&sp, "$\"v={x}\"", String);
    has(&sp, "Task", Type);
    has(&sp, "await", Keyword);
    has(&sp, "Run", Function);
}

#[test]
fn php_tokens() {
    let src = "<?php\n# hash comment\n$name = 'multi\nline'; #[Attr]\nfunction greet($who) { echo \"hi $who\"; return null; } ?>";
    let lines = lex_text(Lang::Php, src);
    assert_ne!(lines[3].1, LineState::default());
    let sp = spans(Lang::Php, src);
    has(&sp, "<?php", Attribute);
    has(&sp, "# hash comment", Comment);
    has(&sp, "line'", String);
    has(&sp, "#[Attr]", Attribute);
    has(&sp, "function", Keyword);
    has(&sp, "greet", Function);
    has(&sp, "echo", Keyword);
    has(&sp, "?>", Attribute);
}

#[test]
fn kotlin_tokens() {
    let src = "@JvmStatic\nfun <T> List<T>.second(): T = this[1]\nval raw = \"\"\"no \\n escape\n\"\"\"\n/* a /* b */ c */ val x = null";
    let sp = spans(Lang::Kotlin, src);
    has(&sp, "@JvmStatic", Attribute);
    has(&sp, "fun", Keyword);
    has(&sp, "second", Function);
    has(&sp, "\"\"\"no \\n escape", String);
    has(&sp, "/* a /* b */ c */", Comment);
    has(&sp, "null", Constant);
}

#[test]
fn swift_tokens() {
    let src = "#if DEBUG\n@objc func tap(_ s: Any) { let m = \"\"\"\nmulti\n\"\"\"; guard let x = y else { return nil } }\n#endif";
    let sp = spans(Lang::Swift, src);
    has(&sp, "#if", Attribute);
    has(&sp, "@objc", Attribute);
    has(&sp, "func", Keyword);
    has(&sp, "tap", Function);
    has(&sp, "multi", String);
    has(&sp, "guard", Keyword);
    has(&sp, "nil", Constant);
}

#[test]
fn python_tokens() {
    let src = "@decorator.name\nasync def fetch(url: str) -> bytes:\n    \"\"\"Doc\n    still doc( \"\"\"\n    data = rb'\\x00' + f\"{url}\\n\" # trailing\n    return None if MAX_TRIES else True";
    let lines = lex_text(Lang::Python, src);
    assert_ne!(lines[3].1, LineState::default());
    let sp = spans(Lang::Python, src);
    has(&sp, "@decorator.name", Attribute);
    has(&sp, "async", Keyword);
    has(&sp, "def", Keyword);
    has(&sp, "fetch", Function);
    has(&sp, "str", Type);
    has(&sp, "    still doc( \"\"\"", String);
    not_kind(&sp, "doc", Function);
    has(&sp, "rb'\\x00'", String);
    has(&sp, "f\"{url}", String);
    has(&sp, "\\n", Escape);
    has(&sp, "# trailing", Comment);
    has(&sp, "None", Constant);
    has(&sp, "MAX_TRIES", Constant);
    has(&sp, "True", Constant);
}

#[test]
fn ruby_tokens() {
    let src = "=begin\nblock comment\n=end\nclass Foo < Base\n  def run!(x) = x.even? ? :yes : :no\n  TEXT = <<~EOS\n    body #{x}\n  EOS\n  w = %w[a b]; r = /re(x)/i\nend";
    let lines = lex_text(Lang::Ruby, src);
    assert_ne!(lines[1].1, LineState::default());
    assert_ne!(lines[6].1, LineState::default(), "heredoc body");
    assert_eq!(lines[8].1, LineState::default(), "after heredoc terminator");
    let sp = spans(Lang::Ruby, src);
    has(&sp, "block comment", Comment);
    has(&sp, "class", Keyword);
    has(&sp, "Foo", Type);
    has(&sp, "run!", Function);
    has(&sp, ":yes", Constant);
    has(&sp, "    body #{x}", String);
    has(&sp, "%w[a b]", String);
    has(&sp, "/re(x)/i", String);
    has(&sp, "end", Keyword);
}

#[test]
fn lua_tokens() {
    let src = "--[==[ long\ncomment ]] still\n]==] local function f(a) return a .. [[long\nstr]] end -- tail";
    let lines = lex_text(Lang::Lua, src);
    assert_ne!(lines[1].1, LineState::default());
    let sp = spans(Lang::Lua, src);
    has(&sp, "comment ]] still", Comment);
    has(&sp, "local", Keyword);
    has(&sp, "f", Function);
    has(&sp, "[[long", String);
    has(&sp, "str]]", String);
    has(&sp, "-- tail", Comment);
}

#[test]
fn shell_tokens() {
    let src = "#!/bin/sh\nNAME=world # set\ngreet() {\n  echo \"hi $NAME ${X:-y}\" | grep -q h && ls $(pwd)\n  cat <<'EOF'\nraw $text\nEOF\n}\nif [ -n \"$1\" ]; then greet; fi";
    let lines = lex_text(Lang::Shell, src);
    assert_ne!(lines[5].1, LineState::default(), "heredoc body");
    assert_eq!(lines[7].1, LineState::default());
    let sp = spans(Lang::Shell, src);
    has(&sp, "#!/bin/sh", Comment);
    has(&sp, "NAME", Key);
    has(&sp, "# set", Comment);
    has(&sp, "greet", Function);
    has(&sp, "echo", Function);
    has(&sp, "$NAME", Constant);
    has(&sp, "${X:-y}", Constant);
    has(&sp, "grep", Function);
    has(&sp, "ls", Function);
    has(&sp, "pwd", Function);
    has(&sp, "raw $text", String);
    has(&sp, "if", Keyword);
    has(&sp, "then", Keyword);
    has(&sp, "fi", Keyword);
}

#[test]
fn makefile_tokens() {
    let src = "CC := gcc # compiler\nall: main.o\n\t$(CC) -o app $^\nifeq ($(OS),x)\nendif";
    let sp = spans(Lang::Makefile, src);
    has(&sp, "CC", Key);
    has(&sp, "# compiler", Comment);
    has(&sp, "all", Function);
    has(&sp, "$(CC)", Constant);
    has(&sp, "$^", Constant);
    has(&sp, "ifeq", Keyword);
}

#[test]
fn sql_tokens() {
    let src = "SELECT id, COUNT(*) FROM users -- note\nWHERE name = 'o''brien' AND age > 3 /* multi\nline */ ORDER BY id;";
    let lines = lex_text(Lang::Sql, src);
    assert_ne!(lines[2].1, LineState::default());
    let sp = spans(Lang::Sql, src);
    has(&sp, "SELECT", Keyword);
    has(&sp, "COUNT", Function);
    has(&sp, "-- note", Comment);
    has(&sp, "'o''brien'", String);
    has(&sp, "line */", Comment);
    has(&sp, "ORDER", Keyword);
}

#[test]
fn markdown_tokens() {
    let src = "# Title\n\n> quote with `code` and **bold** and *em*\n- item [link](http://x.test/a) and _under_\n1. first\n```rust\nfn x() {}\n```\nafter <b>tag</b> \\* <!-- c -->";
    let lines = lex_text(Lang::Markdown, src);
    assert_ne!(lines[6].1, LineState::default(), "inside the fence");
    assert_eq!(lines[8].1, LineState::default(), "after the fence");
    let sp = spans(Lang::Markdown, src);
    has(&sp, "# Title", Heading);
    has(&sp, ">", Marker);
    has(&sp, "`code`", CodeSpan);
    has(&sp, "**bold**", Strong);
    has(&sp, "*em*", Emphasis);
    has(&sp, "-", Marker);
    has(&sp, "[link]", Link);
    has(&sp, "(http://x.test/a)", Link);
    has(&sp, "_under_", Emphasis);
    has(&sp, "1.", Marker);
    has(&sp, "```rust", CodeSpan);
    has(&sp, "fn x() {}", CodeSpan);
    has(&sp, "<b>", Tag);
    has(&sp, "\\*", Escape);
    has(&sp, "<!-- c -->", Comment);
}

#[test]
fn markdown_unclosed_delimiters_are_text() {
    let sp = spans(Lang::Markdown, "a * b ** c ` d [e f");
    assert!(sp.iter().all(|(_, k)| !matches!(k, Emphasis | Strong | CodeSpan | Link)), "{sp:?}");
}

#[test]
fn json_tokens() {
    let src = "{\"key\": \"val\\n\", \"n\": -1.5e3, \"ok\": true, \"z\": null} // c\n/* jsonc\n*/";
    let sp = spans(Lang::Json, src);
    has(&sp, "\"key\"", Key);
    has(&sp, "\"val", String);
    has(&sp, "\\n", Escape);
    has(&sp, "-1.5e3", Number);
    has(&sp, "true", Constant);
    has(&sp, "null", Constant);
    has(&sp, "// c", Comment);
    has(&sp, "*/", Comment);
}

#[test]
fn toml_tokens() {
    let src = "[package]\nname = \"margin\" # c\nver.minor = 3\n[[bin]]\ndoc = \"\"\"\nmulti\n\"\"\"\nflag = true\nwhen = 1979-05-27T07:32:00Z";
    let sp = spans(Lang::Toml, src);
    has(&sp, "[package]", Heading);
    has(&sp, "name", Key);
    has(&sp, "\"margin\"", String);
    has(&sp, "# c", Comment);
    has(&sp, "ver", Key);
    has(&sp, "[[bin]]", Heading);
    has(&sp, "multi", String);
    has(&sp, "true", Constant);
    has(&sp, "1979-05-27T07:32:00Z", Number);
}

#[test]
fn yaml_tokens() {
    let src = "---\nname: app # c\nlist:\n  - one\n  - &anc two\nscript: |\n  echo a: b\n  # not a comment\nnext: *anc\nflag: yes\n\"quoted key\": 3";
    let lines = lex_text(Lang::Yaml, src);
    assert_ne!(lines[6].1, LineState::default(), "block scalar");
    assert_ne!(lines[7].1, LineState::default());
    let sp = spans(Lang::Yaml, src);
    has(&sp, "---", Heading);
    has(&sp, "name", Key);
    has(&sp, "# c", Comment);
    has(&sp, "-", Marker);
    has(&sp, "&anc", Attribute);
    has(&sp, "echo a: b", String);
    has(&sp, "# not a comment", String);
    has(&sp, "next", Key);
    has(&sp, "*anc", Attribute);
    has(&sp, "yes", Constant);
    has(&sp, "\"quoted key\"", Key);
    has(&sp, "3", Number);
}

#[test]
fn html_tokens() {
    let src = "<!DOCTYPE html>\n<a href=\"/x\" data-id=7\n   class='c'>T &amp; U</a><!-- note\n--><script>let s = \"</b>\"; f(1)\n</script><style>p { color: red; }</style>";
    let lines = lex_text(Lang::Html, src);
    assert_ne!(lines[2].1, LineState::default(), "inside a tag");
    assert_ne!(lines[4].1, LineState::default(), "inside script");
    let sp = spans(Lang::Html, src);
    has(&sp, "<!DOCTYPE html>", Keyword);
    has(&sp, "a", Tag);
    has(&sp, "href", Key);
    has(&sp, "\"/x\"", String);
    has(&sp, "7", String);
    has(&sp, "class", Key);
    has(&sp, "&amp;", Escape);
    has(&sp, "<!-- note", Comment);
    has(&sp, "-->", Comment);
    has(&sp, "let", Keyword);
    has(&sp, "\"</b>\"", String);
    has(&sp, "f", Function);
    has(&sp, "script", Tag);
    has(&sp, "color", Key);
}

#[test]
fn css_tokens() {
    let src = "@media (max-width: 600px) {\n  .card > a:hover, #main { color: #fff !important; margin: -2px .5em; }\n}\n/* multi\nline */ body { background: url(img/x.png); }";
    let lines = lex_text(Lang::Css, src);
    assert_ne!(lines[2].1, LineState::default(), "inside @media braces");
    let sp = spans(Lang::Css, src);
    has(&sp, "@media", Attribute);
    has(&sp, ".card", Type);
    has(&sp, "a", Tag);
    has(&sp, ":hover", Keyword);
    has(&sp, "#main", Type);
    has(&sp, "color", Key);
    has(&sp, "#fff", Number);
    has(&sp, "!important", Keyword);
    has(&sp, "-2px", Number);
    has(&sp, "line */", Comment);
    has(&sp, "body", Tag);
    has(&sp, "url", Function);
    has(&sp, "img/x.png", String);
}

#[test]
fn diff_tokens() {
    let src = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n ctx\n\\ No newline at end of file";
    let sp = spans(Lang::Diff, src);
    has(&sp, "diff --git a/x b/x", Keyword);
    has(&sp, "--- a/x", Heading);
    has(&sp, "+++ b/x", Heading);
    has(&sp, "@@ -1,2 +1,2 @@", Attribute);
    has(&sp, "-old", Deleted);
    has(&sp, "+new", Inserted);
    has(&sp, "\\ No newline at end of file", Comment);
}

#[test]
fn plain_has_no_tokens() {
    let sp = spans(Lang::Plain, "fn main() { \"x\" }");
    assert!(sp.is_empty());
}

#[test]
fn long_line_capped_but_state_tracked() {
    // A block comment opened after the cap and never closed: the next line must still start inside it.
    let mut line = "x ".repeat(margin_code::LEX_LINE_CAP);
    line.push_str("/* open");
    let mut v = Vec::new();
    let st = lex_line(Lang::C, LineState::default(), &line, &mut v);
    check_tokens(&line, &v);
    assert!(v.iter().all(|t| t.end as usize <= margin_code::LEX_LINE_CAP));
    let mut w = Vec::new();
    lex_line(Lang::C, st, "still */ int y;", &mut w);
    assert_eq!(w.first().map(|t| t.kind), Some(Comment));
}

#[test]
fn foreign_state_is_harmless() {
    // A state produced by one language fed to another must not panic.
    let mut states = vec![LineState::default(), LineState(u64::MAX), LineState(0x0101), LineState(0xFF_FF05)];
    for lang in Lang::ALL {
        let mut v = Vec::new();
        states.push(lex_line(lang, LineState::default(), "/* \"\"\" ``` ''' <!-- [[ =begin", &mut v));
    }
    for lang in Lang::ALL {
        for &s in &states {
            let mut v = Vec::new();
            let line = "a /* b */ \"c\" `d` ]] */ end";
            lex_line(lang, s, line, &mut v);
            check_tokens(line, &v);
        }
    }
}
