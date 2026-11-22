//! Outline fixtures per language: names, positions, extents, calls, sinks.

mod common;
use common::fixtures;
use common::*;
use margin_code::{FnDef, Lang, SinkKind};

/// (line, col) of the `nth` occurrence of `needle` (0-based) in `src`, plus `shift` bytes.
fn at(src: &str, needle: &str, nth: usize) -> (u32, u32) {
    let off = src.match_indices(needle).nth(nth).unwrap_or_else(|| panic!("{needle:?} #{nth} not in fixture")).0;
    let line = src[..off].matches('\n').count() as u32;
    let col = (off - src[..off].rfind('\n').map_or(0, |p| p + 1)) as u32;
    (line, col)
}

fn line_of(src: &str, needle: &str) -> u32 {
    at(src, needle, 0).0
}

#[track_caller]
fn def_at(f: &FnDef, src: &str, needle: &str, nth: usize) {
    let (l, c) = at(src, needle, nth);
    assert_eq!((f.line, f.col), (l, c), "position of {}", f.name);
}

#[track_caller]
fn sink(f: &FnDef, name: &str) -> Option<SinkKind> {
    f.calls.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("{} has no call {name}: {:?}", f.name, calls(f))).sink
}

#[track_caller]
fn qual<'a>(f: &'a FnDef, name: &str) -> &'a str {
    &f.calls.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("{} has no call {name}", f.name)).qualified
}

#[test]
fn c_outline() {
    let s = fixtures::C;
    let o = fns(Lang::C, s);
    assert_eq!(names(&o), ["parse_header", "run_handler", "spawn_shell", "spawn_shell", "main"], "{}", dump(&o));
    let p = &o[0];
    def_at(p, s, "parse_header(const char *in, size_t n)\n{", 0);
    assert_eq!(p.start, line_of(s, "static int\nparse_header"), "start includes the return type line");
    assert_eq!(p.end, line_of(s, "    return helper(buf);") + 1);
    // Strings, comments, `sizeof`, `if`, `for` are not calls; macros are.
    assert_eq!(calls(p), ["memcpy", "SQUARE", "helper"]);
    assert_eq!(sink(p, "memcpy"), Some(SinkKind::Memory));
    let (l, c) = at(s, "memcpy(buf", 0);
    assert_eq!((p.calls[0].line, p.calls[0].col), (l, c));
    // Function pointer call `(*h)(code)` has no name.
    assert!(o[1].calls.is_empty());
    // `#ifdef` branches: both heads seen, the body belongs to the `#else` one.
    assert_eq!(calls(&o[3]), ["system"]);
    assert_eq!(sink(&o[3], "system"), Some(SinkKind::Exec));
    let m = &o[4];
    assert_eq!(calls(m), ["printf", "parse_header", "strlen", "spawn_shell"]);
    assert_eq!(sink(m, "printf"), Some(SinkKind::Format));
    assert_eq!(m.start, line_of(s, "int main("));
    assert_eq!(m.end, line_of(s, "    return spawn_shell(argv[0]);") + 1);
}

#[test]
fn c_prototypes_macros_and_initialisers_are_not_definitions() {
    let s = "int f(int);\nextern void g(void);\nMODULE_INIT(setup);\nint table[] = { h(1), h(2) };\nstatic int (*fp)(int) = 0;\nvoid (*signal_like(int s))(int);\n";
    assert!(fns(Lang::C, s).is_empty(), "{}", dump(&fns(Lang::C, s)));
}

#[test]
fn c_macro_before_definition_is_replaced() {
    let s = "EXPORT_API(int)\nadd(int a, int b)\n{\n  return sum(a, b);\n}\n__attribute__((noreturn)) void die(const char *m) { abort(); }";
    let o = fns(Lang::C, s);
    assert_eq!(names(&o), ["add", "die"], "{}", dump(&o));
    assert_eq!(calls(&o[0]), ["sum"]);
    assert_eq!(o[0].start, 0);
}

#[test]
fn cpp_outline() {
    let s = fixtures::CPP;
    let o = fns(Lang::Cpp, s);
    assert_eq!(names(&o), ["clamp", "Shape", "id", "~Shape", "area", "c_entry"], "{}", dump(&o));
    let clamp = get(&o, "clamp");
    assert_eq!(clamp.start, line_of(s, "template <typename T>"), "template line starts the definition");
    assert_eq!(qual(clamp, "max"), "std::max");
    let dtor = get(&o, "~Shape");
    def_at(dtor, s, "Shape() {", 0);
    assert_eq!(calls(dtor), ["log_destroy"]);
    let area = get(&o, "area");
    def_at(area, s, "area() const {", 0);
    // Lambda body calls count for the enclosing function.
    assert_eq!(calls(area), ["sq"]);
    let c = get(&o, "c_entry");
    assert_eq!(sink(c, "strcpy"), Some(SinkKind::Memory));
    assert_eq!(qual(c, "system"), "std::system");
    assert_eq!(sink(c, "system"), Some(SinkKind::Exec));
}

#[test]
fn rust_outline() {
    let s = fixtures::RUST;
    let o = fns(Lang::Rust, s);
    assert_eq!(names(&o), ["new", "run", "twice", "outer", "inner"], "{}", dump(&o));
    let run = get(&o, "run");
    def_at(run, s, "run(&self", 0);
    assert_eq!(run.start, line_of(s, "#[inline]"));
    assert_eq!(run.end, line_of(s, "        Ok(())") + 1);
    // Closure body calls belong to `run`; `Ok`, macros and strings are not calls.
    assert_eq!(calls(run), ["compute", "new", "arg", "status", "add"]);
    assert_eq!(qual(run, "new"), "Command::new");
    assert_eq!(sink(run, "new"), Some(SinkKind::Exec));
    let outer = get(&o, "outer");
    assert_eq!(calls(outer), ["wrapping_add", "with_capacity", "transmute", "inner", "len"]);
    assert_eq!(sink(outer, "transmute"), Some(SinkKind::Memory));
    let inner = get(&o, "inner");
    assert_eq!((inner.start, inner.end), (inner.line, inner.line));
    // Trait method without a body is not a definition.
    assert!(!names(&o).contains(&"visit"));
}

#[test]
fn go_outline() {
    let s = fixtures::GO;
    let o = fns(Lang::Go, s);
    assert_eq!(names(&o), ["Lookup", "run", "Map"], "{}", dump(&o));
    let l = &o[0];
    def_at(l, s, "Lookup(", 0);
    assert_eq!(sink(l, "Query"), Some(SinkKind::Sql));
    assert_eq!(qual(l, "Query"), "s.db.Query");
    // Goroutine closure calls count.
    assert!(calls(l).contains(&"notify"));
    assert_eq!(l.end, line_of(s, "\treturn nil\n}") + 1);
    assert_eq!(sink(&o[1], "Command"), Some(SinkKind::Exec));
    assert_eq!(calls(&o[2]), ["make", "len", "append", "f"]);
}

#[test]
fn java_outline() {
    let s = fixtures::JAVA;
    let o = fns(Lang::Java, s);
    assert_eq!(names(&o), ["UserStore", "find", "listen", "run"], "{}", dump(&o));
    let find = get(&o, "find");
    assert_eq!(find.start, line_of(s, "    @Override\n    public <R>"));
    assert_eq!(sink(find, "executeQuery"), Some(SinkKind::Sql));
    assert_eq!(qual(find, "exec"), "Runtime.getRuntime().exec");
    assert_eq!(sink(find, "exec"), Some(SinkKind::Exec));
    assert_eq!(qual(find, "ArrayList"), "new ArrayList");
    // Anonymous class method: its own definition, and its calls count for `listen` too.
    let run = get(&o, "run");
    assert_eq!(sink(run, "readObject"), Some(SinkKind::Deserialize));
    let listen = get(&o, "listen");
    assert!(calls(listen).contains(&"readObject"));
    assert!(!calls(listen).iter().any(|c| *c == "run" && listen.calls.iter().any(|k| k.qualified.is_empty() && k.name == "run")));
    assert!(!names(&o).contains(&"hook"), "abstract method without body");
}

#[test]
fn js_outline() {
    let s = fixtures::JS;
    let o = fns(Lang::JavaScript, s);
    assert_eq!(
        names(&o),
        ["build", "handler", "square", "constructor", "render", "create", "onClick", "helper", "short", "default"],
        "{}",
        dump(&o)
    );
    let b = &o[0];
    assert_eq!(calls(b), ["exec", "log"], "strings and comments are masked; arrow callback calls count");
    assert_eq!(sink(b, "exec"), Some(SinkKind::Exec));
    let h = get(&o, "handler");
    assert_eq!(sink(h, "fetch"), Some(SinkKind::Network));
    assert_eq!(h.end, line_of(s, "  res.send(data);") + 1);
    let sq = get(&o, "square");
    assert_eq!((sq.start, sq.end), (sq.line, sq.line), "expression body ends on its line");
    let r = get(&o, "render");
    assert_eq!(sink(r, "insertAdjacentHTML"), Some(SinkKind::Html));
    assert_eq!(sink(r, "write"), Some(SinkKind::Html));
    assert_eq!(qual(get(&o, "create"), "Widget"), "new Widget");
    assert_eq!(sink(get(&o, "helper"), "eval"), Some(SinkKind::Eval));
    assert_eq!(calls(get(&o, "default")), ["build"]);
}

#[test]
fn js_child_process_via_require_chain() {
    let s = "function f(c) { require('child_process').execSync(c); }";
    let o = fns(Lang::JavaScript, s);
    assert_eq!(qual(&o[0], "execSync"), "require('child_process').execSync");
    assert_eq!(sink(&o[0], "execSync"), Some(SinkKind::Exec));
}

#[test]
fn ts_outline() {
    let s = fixtures::TS;
    let o = fns(Lang::TypeScript, s);
    assert_eq!(names(&o), ["constructor", "find", "parse", "Panel", "generic"], "{}", dump(&o));
    let f = get(&o, "find");
    assert_eq!(f.start, f.line);
    assert_eq!(sink(f, "query"), Some(SinkKind::Sql));
    assert_eq!(qual(f, "query"), "this.db.query");
    assert_eq!(calls(get(&o, "Panel")), ["render"]);
    assert_eq!(calls(get(&o, "generic")), ["identity"], "generic call `identity<T>(x)`");
}

#[test]
fn python_outline() {
    let s = fixtures::PYTHON;
    let o = fns(Lang::Python, s);
    assert_eq!(names(&o), ["plain", "run_cmd", "nested", "load", "fetch"], "{}", dump(&o));
    let p = &o[0];
    assert_eq!(calls(p), ["len"], "docstring and comment calls are masked");
    assert_eq!(p.end, line_of(s, "    return total"));
    let r = get(&o, "run_cmd");
    assert_eq!(r.start, line_of(s, "@app.route"), "decorators start the definition");
    def_at(r, s, "run_cmd", 0);
    assert_eq!(r.end, line_of(s, "    return nested(cmd)"));
    assert_eq!(calls(r), ["system", "eval", "run", "nested"], "nested def calls count for the outer def");
    assert_eq!(sink(r, "system"), Some(SinkKind::Exec));
    assert_eq!(sink(r, "run"), Some(SinkKind::Exec));
    let n = get(&o, "nested");
    assert_eq!((n.start, n.end), (line_of(s, "    def nested"), line_of(s, "        return eval(x)")));
    assert_eq!(sink(n, "eval"), Some(SinkKind::Eval));
    let l = get(&o, "load");
    assert_eq!(sink(l, "loads"), Some(SinkKind::Deserialize));
    assert_eq!(sink(l, "execute"), Some(SinkKind::Sql));
    let f = get(&o, "fetch");
    assert_eq!(sink(f, "get"), Some(SinkKind::Network));
    assert_eq!(f.end, line_of(s, "        return await requests.get(url)"));
    // `class Store(Base)` is not a call; the lambda at module level is outside every def.
    assert!(o.iter().all(|f| !calls(f).contains(&"Store") && !calls(f).contains(&"open")));
}

#[test]
fn csharp_outline() {
    let s = fixtures::CSHARP;
    let o = fns(Lang::CSharp, s);
    assert_eq!(names(&o), ["Runner", "Start", "Twice", "QueryAsync"], "{}", dump(&o));
    let st = get(&o, "Start");
    assert_eq!(sink(st, "Start"), Some(SinkKind::Exec));
    assert_eq!(sink(st, "Deserialize"), Some(SinkKind::Deserialize));
    let t = get(&o, "Twice");
    assert_eq!(calls(t), ["Double"]);
    assert_eq!(t.end, t.line);
    assert_eq!(sink(get(&o, "QueryAsync"), "ExecuteScalarAsync"), Some(SinkKind::Sql));
}

#[test]
fn php_outline() {
    let s = fixtures::PHP;
    let o = fns(Lang::Php, s);
    assert_eq!(names(&o), ["render_page", "index", "helper"], "{}", dump(&o));
    let r = &o[0];
    assert_eq!(sink(r, "file_get_contents"), Some(SinkKind::File));
    assert_eq!(sink(r, "include"), Some(SinkKind::File));
    let i = get(&o, "index");
    assert_eq!(qual(i, "query"), "$this.db.query", "`->` normalised to `.`");
    assert_eq!(sink(i, "query"), Some(SinkKind::Sql));
    assert_eq!(sink(i, "system"), Some(SinkKind::Exec));
    assert_eq!(sink(i, "unserialize"), Some(SinkKind::Deserialize), "closure calls count");
    assert!(!calls(i).iter().any(|c| c.starts_with('$')), "`$fn(…)` is not a named call");
    assert_eq!(qual(get(&o, "helper"), "other"), "self::other");
}

#[test]
fn ruby_outline() {
    let s = fixtures::RUBY;
    let o = fns(Lang::Ruby, s);
    assert_eq!(names(&o), ["index", "build", "total", "update"], "{}", dump(&o));
    let i = &o[0];
    assert_eq!((i.start, i.end), (line_of(s, "    def index"), line_of(s, "      render json") + 1));
    assert_eq!(sink(i, "system"), Some(SinkKind::Exec), "paren-less call at statement start");
    let b = get(&o, "build");
    def_at(b, s, "build", 0);
    assert_eq!(sink(b, "load"), Some(SinkKind::Deserialize));
    assert_eq!(b.calls.iter().filter(|c| c.sink == Some(SinkKind::Deserialize)).count(), 2);
    let t = get(&o, "total");
    assert_eq!(t.end, t.line, "endless def");
    let u = get(&o, "update");
    assert_eq!(u.end, line_of(s, "      end\n    end\n  end\nend") + 1, "`do … end`, `while … do`, `unless` blocks balance");
}

#[test]
fn kotlin_outline() {
    let s = fixtures::KOTLIN;
    let o = fns(Lang::Kotlin, s);
    assert_eq!(names(&o), ["run", "second", "toString", "constructor", "main"], "{}", dump(&o));
    let r = &o[0];
    assert_eq!(sink(r, "exec"), Some(SinkKind::Exec));
    assert!(calls(r).contains(&"log"), "lambda calls count");
    assert_eq!(get(&o, "toString").end, get(&o, "toString").line);
    assert_eq!(calls(get(&o, "constructor")), ["check"]);
}

#[test]
fn swift_outline() {
    let s = fixtures::SWIFT;
    let o = fns(Lang::Swift, s);
    assert_eq!(names(&o), ["init", "run"], "{}", dump(&o));
    let r = &o[1];
    assert_eq!(sink(r, "Process"), Some(SinkKind::Exec));
    assert_eq!(sink(r, "launch"), Some(SinkKind::Exec));
    assert_eq!(sink(r, "system"), Some(SinkKind::Exec));
    assert_eq!(r.start, line_of(s, "    func run(_ cmd: String) -> Int32 {"));
}

#[test]
fn lua_outline() {
    let s = fixtures::LUA;
    let o = fns(Lang::Lua, s);
    assert_eq!(names(&o), ["helper", "run", "method", "cb"], "{}", dump(&o));
    let h = &o[0];
    assert_eq!(h.start, line_of(s, "local function helper"));
    let r = get(&o, "run");
    assert_eq!(r.end, line_of(s, "  return io.popen(cmd)") + 1, "`if`/`for … do` blocks balance");
    assert_eq!(sink(r, "execute"), Some(SinkKind::Exec));
    assert_eq!(sink(r, "popen"), Some(SinkKind::Exec));
    let m = get(&o, "method");
    assert_eq!(sink(m, "loadstring"), Some(SinkKind::Eval));
    assert_eq!(m.end, line_of(s, "  until true") + 1, "`repeat … until`");
    assert_eq!(calls(get(&o, "cb")), ["print"]);
}

#[test]
fn shell_outline() {
    let s = fixtures::SHELL;
    let o = fns(Lang::Shell, s);
    assert_eq!(names(&o), ["log", "deploy", "build"], "{}", dump(&o));
    let d = &o[1];
    assert_eq!(calls(d), ["log", "curl", "eval"]);
    assert_eq!(sink(d, "eval"), Some(SinkKind::Eval));
    assert_eq!(calls(&o[2]), ["make", "log"]);
}

#[test]
fn strings_and_comments_never_define_or_call() {
    let cases: &[(Lang, &str)] = &[
        (Lang::C, "/* int f(void) { g(); } */\nconst char *s = \"int h(void) { k(); }\";"),
        (Lang::Rust, "// fn a() { b(); }\nconst S: &str = r#\"fn c() { d(); }\"#;"),
        (Lang::Python, "s = '''\ndef a():\n    b()\n'''\n# def c(): d()"),
        (Lang::JavaScript, "const t = `function a() { b(); }`; // function c() {}"),
        (Lang::Go, "var s = `func a() { b() }`"),
        (Lang::Ruby, "=begin\ndef a\n  b()\nend\n=end\nx = \"def c; d(); end\""),
        (Lang::Lua, "--[[ function a() b() end ]]\nlocal s = [[function c() d() end]]"),
        (Lang::Shell, "# f() { g; }\necho 'h() { k; }'"),
        (Lang::Php, "<?php // function a() { b(); }\n$s = 'function c() { d(); }';"),
    ];
    for (lang, src) in cases {
        let o = fns(*lang, src);
        assert!(o.is_empty(), "{lang:?}: {}", dump(&o));
    }
}

#[test]
fn deep_nesting_is_bounded() {
    let mut s = String::from("void f(void) {\n");
    s.push_str(&"{ g(); ".repeat(5000));
    s.push_str(&"} ".repeat(5000));
    s.push_str("\n}\n");
    let o = fns(Lang::C, &s);
    assert_eq!(names(&o), ["f"]);
    assert_eq!(o[0].calls.len(), 2000, "MAX_CALLS per function");
    let mut js = String::new();
    for _ in 0..100 {
        js.push_str("function a() { ");
    }
    js.push_str("x();");
    js.push_str(&"}".repeat(100));
    let o = fns(Lang::JavaScript, &js);
    assert!(o.len() <= 16, "open-function depth is capped: {}", o.len());
    assert!(o.iter().all(|f| calls(f) == ["x"]));
}

#[test]
fn function_cap_per_file() {
    let s = "def f():\n    pass\n".repeat(25_000);
    assert_eq!(fns(Lang::Python, &s).len(), 20_000);
}

#[test]
fn long_names_are_skipped() {
    let long = "a".repeat(300);
    let s = format!("fn {long}() {{ {long}(); ok(); }}\nfn short() {{ {long}(); }}");
    let o = fns(Lang::Rust, &s);
    assert_eq!(names(&o), ["short"]);
    assert!(o[0].calls.is_empty());
}

#[test]
fn qualified_is_capped() {
    let chain = (0..60).map(|i| format!("seg{i}")).collect::<Vec<_>>().join(".");
    let s = format!("function f() {{ {chain}.last(); }}");
    let o = fns(Lang::JavaScript, &s);
    let c = &o[0].calls[0];
    assert_eq!(c.name, "last");
    assert!(c.qualified.len() <= 128 && c.qualified.ends_with(".last"), "{}", c.qualified);
}

#[test]
fn no_outline_languages_are_empty() {
    for lang in Lang::ALL {
        if !lang.has_outline() {
            assert!(fns(lang, "def f():\n  g()\nfunction h() { k(); }\nfn x() { y(); }").is_empty(), "{lang:?}");
        }
    }
}
