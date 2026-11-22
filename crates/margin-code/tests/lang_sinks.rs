//! `Lang` tables and `sink_kind` tables.

use margin_code::SinkKind::*;
use margin_code::{sink_kind, Lang, SinkKind};

#[test]
fn from_path() {
    let cases = [
        ("src/main.rs", Lang::Rust),
        ("a/b/x.H", Lang::C),
        ("x.c", Lang::C),
        ("x.hpp", Lang::Cpp),
        ("x.cc", Lang::Cpp),
        ("x.CXX", Lang::Cpp),
        ("x.hh", Lang::Cpp),
        ("m.go", Lang::Go),
        ("A.java", Lang::Java),
        ("a.mjs", Lang::JavaScript),
        ("a.cjs", Lang::JavaScript),
        ("a.jsx", Lang::JavaScript),
        ("a.tsx", Lang::TypeScript),
        ("a.py", Lang::Python),
        ("a.cs", Lang::CSharp),
        ("a.php", Lang::Php),
        ("a.rb", Lang::Ruby),
        ("Gemfile", Lang::Ruby),
        ("a.kt", Lang::Kotlin),
        ("build.gradle.kts", Lang::Kotlin),
        ("a.swift", Lang::Swift),
        ("run.sh", Lang::Shell),
        ("/home/u/.bashrc", Lang::Shell),
        (".zshrc", Lang::Shell),
        ("Dockerfile", Lang::Shell),
        ("Dockerfile.dev", Lang::Shell),
        ("x.json", Lang::Json),
        ("Cargo.toml", Lang::Toml),
        ("Cargo.lock", Lang::Toml),
        ("ci.yml", Lang::Yaml),
        ("x.YAML", Lang::Yaml),
        ("index.html", Lang::Html),
        ("a.svg", Lang::Html),
        ("s.css", Lang::Css),
        ("q.sql", Lang::Sql),
        ("a.lua", Lang::Lua),
        ("Makefile", Lang::Makefile),
        ("rules.mk", Lang::Makefile),
        ("fix.patch", Lang::Diff),
        ("x.diff", Lang::Diff),
        ("README.md", Lang::Markdown),
        ("CMakeLists.txt", Lang::Plain),
        ("noext", Lang::Plain),
        (".hidden", Lang::Plain),
        ("trailing.", Lang::Plain),
        ("", Lang::Plain),
        ("C:\\src\\win.cpp", Lang::Cpp),
    ];
    for (p, l) in cases {
        assert_eq!(Lang::from_path(p), l, "{p}");
    }
}

#[test]
fn names_and_comments() {
    assert_eq!(Lang::Rust.name(), "Rust");
    assert_eq!(Lang::Cpp.name(), "C++");
    assert_eq!(Lang::Plain.name(), "Plain text");
    assert_eq!(Lang::CSharp.name(), "C#");
    for l in Lang::ALL {
        assert!(!l.name().is_empty());
    }
    assert_eq!(Lang::Python.line_comment(), Some("#"));
    assert_eq!(Lang::Go.line_comment(), Some("//"));
    assert_eq!(Lang::Sql.line_comment(), Some("--"));
    assert_eq!(Lang::Lua.line_comment(), Some("--"));
    assert_eq!(Lang::Json.line_comment(), None);
    assert!(Lang::Shell.has_outline() && !Lang::Json.has_outline() && !Lang::Markdown.has_outline());
    let mut all = Lang::ALL.to_vec();
    all.dedup();
    assert_eq!(all.len(), 25);
}

fn s(lang: Lang, name: &str, q: &str) -> Option<SinkKind> {
    sink_kind(lang, name, q)
}

#[test]
fn c_sinks() {
    for n in ["strcpy", "strcat", "sprintf", "vsprintf", "gets", "scanf", "sscanf", "memcpy", "memmove", "strncpy", "alloca"] {
        assert_eq!(s(Lang::C, n, ""), Some(Memory), "{n}");
    }
    for n in ["system", "popen", "execl", "execvp", "execve", "posix_spawn"] {
        assert_eq!(s(Lang::C, n, ""), Some(Exec), "{n}");
    }
    assert_eq!(s(Lang::C, "printf", ""), Some(Format));
    assert_eq!(s(Lang::C, "syslog", ""), Some(Format));
    assert_eq!(s(Lang::C, "fopen", ""), Some(File));
    assert_eq!(s(Lang::Cpp, "system", "std::system"), Some(Exec));
    assert_eq!(s(Lang::Cpp, "memcpy", "buf.memcpy"), None, "member call is not libc");
    assert_eq!(s(Lang::C, "strlen", ""), None);
}

#[test]
fn python_sinks() {
    assert_eq!(s(Lang::Python, "eval", ""), Some(Eval));
    assert_eq!(s(Lang::Python, "exec", ""), Some(Eval));
    assert_eq!(s(Lang::Python, "system", "os.system"), Some(Exec));
    assert_eq!(s(Lang::Python, "popen", "os.popen"), Some(Exec));
    assert_eq!(s(Lang::Python, "check_output", "subprocess.check_output"), Some(Exec));
    assert_eq!(s(Lang::Python, "Popen", "subprocess.Popen"), Some(Exec));
    assert_eq!(s(Lang::Python, "run", "subprocess.run"), Some(Exec));
    assert_eq!(s(Lang::Python, "run", "app.run"), None);
    assert_eq!(s(Lang::Python, "loads", "pickle.loads"), Some(Deserialize));
    assert_eq!(s(Lang::Python, "load", "yaml.load"), Some(Deserialize));
    assert_eq!(s(Lang::Python, "safe_load", "yaml.safe_load"), None);
    assert_eq!(s(Lang::Python, "loads", "marshal.loads"), Some(Deserialize));
    assert_eq!(s(Lang::Python, "open", "shelve.open"), Some(Deserialize));
    assert_eq!(s(Lang::Python, "__import__", ""), Some(Eval));
    assert_eq!(s(Lang::Python, "execute", "cursor.execute"), Some(Sql));
    assert_eq!(s(Lang::Python, "executemany", "self.cur.executemany"), Some(Sql));
    assert_eq!(s(Lang::Python, "get", "requests.get"), Some(Network));
    assert_eq!(s(Lang::Python, "urlopen", "urllib.request.urlopen"), Some(Network));
    assert_eq!(s(Lang::Python, "open", ""), Some(File));
    assert_eq!(s(Lang::Python, "open", "zipfile.open"), None);
    assert_eq!(s(Lang::Python, "mark_safe", ""), Some(Html));
    assert_eq!(s(Lang::Python, "render_template_string", "flask.render_template_string"), Some(Eval));
    assert_eq!(s(Lang::Python, "get", "session.get"), None);
}

#[test]
fn js_sinks() {
    assert_eq!(s(Lang::JavaScript, "eval", ""), Some(Eval));
    assert_eq!(s(Lang::JavaScript, "Function", "new Function"), Some(Eval));
    assert_eq!(s(Lang::JavaScript, "exec", "child_process.exec"), Some(Exec));
    assert_eq!(s(Lang::JavaScript, "execSync", ""), Some(Exec));
    assert_eq!(s(Lang::JavaScript, "exec", "re.exec"), None, "RegExp#exec");
    assert_eq!(s(Lang::TypeScript, "write", "document.write"), Some(Html));
    assert_eq!(s(Lang::TypeScript, "insertAdjacentHTML", "el.insertAdjacentHTML"), Some(Html));
    assert_eq!(s(Lang::JavaScript, "readFile", "fs.readFile"), Some(File));
    assert_eq!(s(Lang::JavaScript, "readFile", "reader.readFile"), None);
    assert_eq!(s(Lang::JavaScript, "query", "db.query"), Some(Sql));
    assert_eq!(s(Lang::JavaScript, "fetch", ""), Some(Network));
    assert_eq!(s(Lang::JavaScript, "get", "axios.get"), Some(Network));
    assert_eq!(s(Lang::JavaScript, "runInNewContext", "vm.runInNewContext"), Some(Eval));
    assert_eq!(s(Lang::JavaScript, "unserialize", "serialize.unserialize"), Some(Deserialize));
    assert_eq!(s(Lang::JavaScript, "setTimeout", ""), None);
}

#[test]
fn java_kotlin_swift_sinks() {
    assert_eq!(s(Lang::Java, "exec", "Runtime.getRuntime().exec"), Some(Exec));
    assert_eq!(s(Lang::Java, "ProcessBuilder", "new ProcessBuilder"), Some(Exec));
    assert_eq!(s(Lang::Java, "readObject", "ois.readObject"), Some(Deserialize));
    assert_eq!(s(Lang::Java, "XMLDecoder", "new XMLDecoder"), Some(Deserialize));
    assert_eq!(s(Lang::Java, "executeQuery", "stmt.executeQuery"), Some(Sql));
    assert_eq!(s(Lang::Java, "execute", "executor.execute"), None);
    assert_eq!(s(Lang::Java, "execute", "stmt.execute"), Some(Sql));
    assert_eq!(s(Lang::Java, "createNativeQuery", "em.createNativeQuery"), Some(Sql));
    assert_eq!(s(Lang::Java, "lookup", "ctx.lookup"), Some(Network));
    assert_eq!(s(Lang::Java, "lookup", "map.lookup"), None);
    assert_eq!(s(Lang::Java, "eval", "engine.eval"), Some(Eval));
    assert_eq!(s(Lang::Java, "getRuntime", "Runtime.getRuntime"), None);
    assert_eq!(s(Lang::Kotlin, "exec", "Runtime.getRuntime().exec"), Some(Exec));
    assert_eq!(s(Lang::Kotlin, "rawQuery", "db.rawQuery"), Some(Sql));
    assert_eq!(s(Lang::Swift, "unarchiveObject", "NSKeyedUnarchiver.unarchiveObject"), Some(Deserialize));
    assert_eq!(s(Lang::Swift, "launch", "task.launch"), Some(Exec));
    assert_eq!(s(Lang::Swift, "run", "runner.run"), None);
}

#[test]
fn go_rust_sinks() {
    assert_eq!(s(Lang::Go, "Command", "exec.Command"), Some(Exec));
    assert_eq!(s(Lang::Go, "CommandContext", "exec.CommandContext"), Some(Exec));
    assert_eq!(s(Lang::Go, "Query", "db.Query"), Some(Sql));
    assert_eq!(s(Lang::Go, "Exec", "tx.Exec"), Some(Sql));
    assert_eq!(s(Lang::Go, "Exec", "tmpl.Exec"), None);
    assert_eq!(s(Lang::Go, "HTML", "template.HTML"), Some(Html));
    assert_eq!(s(Lang::Go, "Open", "os.Open"), Some(File));
    assert_eq!(s(Lang::Go, "Get", "http.Get"), Some(Network));
    assert_eq!(s(Lang::Go, "Pointer", "unsafe.Pointer"), Some(Memory));
    assert_eq!(s(Lang::Rust, "new", "Command::new"), Some(Exec));
    assert_eq!(s(Lang::Rust, "new", "std::process::Command::new"), Some(Exec));
    assert_eq!(s(Lang::Rust, "new", "Vec::new"), None);
    assert_eq!(s(Lang::Rust, "transmute", "std::mem::transmute"), Some(Memory));
    assert_eq!(s(Lang::Rust, "from_raw_parts", "slice::from_raw_parts"), Some(Memory));
    assert_eq!(s(Lang::Rust, "read", "ptr::read"), Some(Memory));
    assert_eq!(s(Lang::Rust, "read", "file.read"), None);
    assert_eq!(s(Lang::Rust, "system", "libc::system"), Some(Exec));
    assert_eq!(s(Lang::Rust, "open", "File::open"), Some(File));
}

#[test]
fn php_ruby_csharp_lua_shell_sinks() {
    for n in ["eval", "assert", "call_user_func", "extract"] {
        assert_eq!(s(Lang::Php, n, ""), Some(Eval), "{n}");
    }
    for n in ["system", "exec", "shell_exec", "passthru", "popen", "proc_open"] {
        assert_eq!(s(Lang::Php, n, ""), Some(Exec), "{n}");
    }
    assert_eq!(s(Lang::Php, "unserialize", ""), Some(Deserialize));
    assert_eq!(s(Lang::Php, "include", ""), Some(File));
    assert_eq!(s(Lang::Php, "require_once", ""), Some(File));
    assert_eq!(s(Lang::Php, "mysqli_query", ""), Some(Sql));
    assert_eq!(s(Lang::Php, "exec", "$pdo.exec"), Some(Sql), "PDO::exec");
    assert_eq!(s(Lang::Php, "curl_exec", ""), Some(Network));
    assert_eq!(s(Lang::Ruby, "eval", ""), Some(Eval));
    assert_eq!(s(Lang::Ruby, "send", "obj.send"), Some(Eval));
    assert_eq!(s(Lang::Ruby, "system", ""), Some(Exec));
    assert_eq!(s(Lang::Ruby, "load", "Marshal.load"), Some(Deserialize));
    assert_eq!(s(Lang::Ruby, "load", "YAML.load"), Some(Deserialize));
    assert_eq!(s(Lang::Ruby, "safe_load", "YAML.safe_load"), None);
    assert_eq!(s(Lang::Ruby, "constantize", "params[:k].constantize"), Some(Eval));
    assert_eq!(s(Lang::Ruby, "open", ""), Some(File));
    assert_eq!(s(Lang::CSharp, "Start", "Process.Start"), Some(Exec));
    assert_eq!(s(Lang::CSharp, "Deserialize", "formatter.Deserialize"), Some(Deserialize));
    assert_eq!(s(Lang::CSharp, "Deserialize", "JsonSerializer.Deserialize"), None);
    assert_eq!(s(Lang::CSharp, "ExecuteReader", "cmd.ExecuteReader"), Some(Sql));
    assert_eq!(s(Lang::CSharp, "Load", "Assembly.Load"), Some(Eval));
    assert_eq!(s(Lang::CSharp, "ReadAllText", "File.ReadAllText"), Some(File));
    assert_eq!(s(Lang::Lua, "execute", "os.execute"), Some(Exec));
    assert_eq!(s(Lang::Lua, "popen", "io.popen"), Some(Exec));
    assert_eq!(s(Lang::Lua, "loadstring", ""), Some(Eval));
    assert_eq!(s(Lang::Lua, "dofile", ""), Some(Eval));
    assert_eq!(s(Lang::Shell, "eval", ""), Some(Eval));
    assert_eq!(s(Lang::Plain, "system", ""), None);
    assert_eq!(s(Lang::Markdown, "eval", ""), None);
}

#[test]
fn labels() {
    let all = [Exec, Memory, Eval, Deserialize, Sql, File, Format, Network, Html];
    let labels: Vec<_> = all.iter().map(|k| k.label()).collect();
    assert_eq!(labels, ["exec", "memory", "eval", "deserialize", "sql", "file", "format", "network", "html"]);
}
