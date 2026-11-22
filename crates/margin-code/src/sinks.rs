//! Dangerous functions per language, for the hint line (`sinks: memcpy, system`).
//!
//! Match on `name` first; `qualified` (as written, `->` already `.`) disambiguates names that are noisy bare.
//! "bare" below = `qualified` empty or equal to `name`.
//!
//! | Lang | Exec | Eval | Deserialize | Sql | File | Network | Memory / Format / Html |
//! |---|---|---|---|---|---|---|---|
//! | C, C++ | `system popen exec* posix_spawn* CreateProcess* ShellExecute* WinExec dlopen LoadLibrary*` | | | `sqlite3_exec` | `fopen freopen open openat creat unlink remove rename chmod chown` | `curl_easy_perform` | Memory `strcpy strcat sprintf vsprintf gets scanf-family memcpy memmove strncpy strncat alloca wcscpy wcscat stpcpy bcopy`; Format `printf fprintf snprintf vprintf vfprintf vsnprintf dprintf syslog vsyslog` |
//! | Python | `os.system os.popen os.spawn* os.exec* subprocess.* pty.spawn create_subprocess_*`, bare `system popen Popen` | `eval exec compile __import__ execfile import_module render_template_string` | `pickle/cPickle/dill/joblib/marshal/torch .load(s)`, `shelve.open`, `yaml.load/unsafe_load`, `jsonpickle.decode` | `*.execute/executemany/executescript`, `objects.raw` | bare `open`, `io.open codecs.open os.remove os.unlink shutil.rmtree send_file send_from_directory` | `requests.* httpx.* urlopen` | Html `mark_safe Markup` |
//! | JS, TS | `child_process.*`/`cp.*` exec family, bare `exec execSync execFile execFileSync spawn spawnSync`, `shell.exec` | `eval`, `Function`/`new Function`, `vm.run*`/`vm.Script`/`compileFunction` | `unserialize deserialize` | `*.query *.execute *.raw`, Prisma `$queryRaw*`/`$executeRaw*` | `fs.*` file ops, `sendFile download` | `fetch axios axios.* http(s).get/request got $.ajax/get/post` | Html `document.write/writeln insertAdjacentHTML createContextualFragment setHTMLUnsafe $(…).html` |
//! | Java | `exec`, `ProcessBuilder` | `eval parseExpression forName defineClass` | `readObject readUnshared XMLDecoder fromXML`, `*yaml*.load` | `executeQuery executeUpdate createQuery createNativeQuery createSQLQuery`, `execute` on stmt/statement/conn/jdbc/template, `jdbcTemplate.query*/update` | `File FileInputStream FileOutputStream FileReader FileWriter RandomAccessFile`, `Files.*` | `openConnection openStream`, `restTemplate.*`, JNDI `lookup` on `*ctx*`/`*context*`/`*jndi*` | Exec also `loadLibrary`, `System.load` |
//! | Go | `exec.Command exec.CommandContext syscall.Exec syscall.ForkExec os.StartProcess` | | | `Query QueryRow Exec *Context` on `db. tx. conn. stmt.` | `os.Open OpenFile Create Remove RemoveAll ReadFile WriteFile`, `ioutil.ReadFile/WriteFile`, `http.ServeFile` | `http.Get Post PostForm Head NewRequest NewRequestWithContext`, `net.Dial` | Memory `unsafe.Pointer/Slice/String`; Html `template.HTML/JS/URL/HTMLAttr` |
//! | Rust | `Command::new`, `libc::` exec family | | | | `File::open File::create`, `fs::read read_to_string write remove_file remove_dir remove_dir_all copy rename` | `reqwest::get TcpStream::connect` | Memory `transmute transmute_copy from_raw_parts(_mut) from_utf8_unchecked get_unchecked(_mut) set_len from_raw assume_init zeroed uninitialized`, `ptr::read/write/copy*`, `libc::` memory family |
//! | PHP | `system exec shell_exec passthru popen proc_open pcntl_exec` (bare) | `eval assert create_function call_user_func call_user_func_array extract` | `unserialize` | `mysql_query mysqli_query mysqli_multi_query pg_query sqlite_query`, `->query`, `->exec` | `include* require* file_get_contents file_put_contents fopen readfile unlink file move_uploaded_file` | `curl_exec fsockopen` | |
//! | Ruby | `system exec spawn popen Open3.*` | `eval instance_eval class_eval module_eval instance_exec send public_send __send__ constantize safe_constantize` | `Marshal.load/restore`, `YAML/Psych.load/unsafe_load`, `Oj.load` | `find_by_sql execute exec_query` (qualified) | `open File.open/read/write IO.read send_file` | `Net::HTTP.get/get_response URI.open HTTParty.* Faraday.*` | Html `html_safe raw` |
//! | C# | `Process.Start ProcessStartInfo` | `Assembly.Load/LoadFrom/LoadFile CSharpScript.*` | `Deserialize UnsafeDeserialize ReadObject` unless `Json` in `qualified` | `ExecuteReader ExecuteNonQuery ExecuteScalar ExecuteSqlRaw ExecuteSqlCommand FromSqlRaw SqlQuery SqlCommand` | `File.*` IO, `FileStream` | `*client*.Get/Post/Send/GetString*Async`, `WebClient` `Download*/Upload*`, `WebRequest.Create` | Html `Html.Raw HtmlString` |
//! | Kotlin | `exec ProcessBuilder` | | `readObject` | `rawQuery execSQL executeQuery` | `File` | | |
//! | Swift | `Process NSTask launch`, `run` on `*task*`/`*process*`, `system popen dlopen` | | `unarchiveObject unarchiveTopLevelObjectWithData` | `sqlite3_exec` | | | |
//! | Lua | `os.execute io.popen` | `loadstring load dofile loadfile` | | | `io.open os.remove` | | |
//! | Shell | | `eval` | | | | `curl wget` | |
//!
//! JNDI `lookup` is labelled Network (remote object fetch). `setTimeout`/`setInterval`, `preg_replace`,
//! `dangerouslySetInnerHTML` and `innerHTML` assignments are not calls and are skipped.

use crate::Lang;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SinkKind {
    /// Runs a command or a program: `system`, `exec*`, `popen`, `Runtime.exec`, `subprocess.*`, `Command::new`.
    Exec,
    /// Unbounded or size-trusting memory writes: `strcpy`, `memcpy`, `sprintf`, `gets`, `alloca`.
    Memory,
    /// Evaluates code: `eval`, `exec` (Python), `Function`, `setTimeout` with a string, template rendering from strings.
    Eval,
    /// Turns bytes into objects: `pickle.loads`, `yaml.load`, `readObject`, `unserialize`, `BinaryFormatter.Deserialize`.
    Deserialize,
    /// Builds or runs SQL: `execute`, `query`, `rawQuery`, `mysql_query`.
    Sql,
    /// File paths from data: `fopen`, `open`, `unlink`, `include`/`require` (PHP), `readFile`, `sendFile`.
    File,
    /// Format-string functions: `printf` family, `syslog`.
    Format,
    /// Outbound requests / SSRF: `curl_exec`, `urlopen`, `requests.get`, `fetch`, `http.Get`.
    Network,
    /// HTML/DOM injection: `innerHTML`-style setters seen as calls, `document.write`, `dangerouslySetInnerHTML`, `mark_safe`.
    Html,
}

impl SinkKind {
    /// Short lowercase label for the UI: `"exec"`, `"memory"`…
    pub fn label(self) -> &'static str {
        match self {
            SinkKind::Exec => "exec",
            SinkKind::Memory => "memory",
            SinkKind::Eval => "eval",
            SinkKind::Deserialize => "deserialize",
            SinkKind::Sql => "sql",
            SinkKind::File => "file",
            SinkKind::Format => "format",
            SinkKind::Network => "network",
            SinkKind::Html => "html",
        }
    }
}

use SinkKind::{Deserialize, Eval, Exec, File, Format, Html, Memory, Network, Sql};

/// Is a call to `name` (last segment) written as `qualified` (may be empty) a sink in `lang`?
pub fn sink_kind(lang: Lang, name: &str, qualified: &str) -> Option<SinkKind> {
    let q = if qualified.is_empty() { name } else { qualified };
    let bare = q == name;
    match lang {
        Lang::C | Lang::Cpp => {
            let std = bare || q.starts_with("std::") || q.starts_with("::");
            if std {
                c_sink(name)
            } else {
                None
            }
        }
        Lang::Python => python(name, q, bare),
        Lang::JavaScript | Lang::TypeScript => js(name, q, bare),
        Lang::Java => java(name, q),
        Lang::Go => go(name, q),
        Lang::Rust => rust(name, q),
        Lang::Php => php(name, q, bare),
        Lang::Ruby => ruby(name, q, bare),
        Lang::CSharp => csharp(name, q),
        Lang::Kotlin => match name {
            "exec" | "ProcessBuilder" => Some(Exec),
            "readObject" | "readUnshared" => Some(Deserialize),
            "rawQuery" | "execSQL" | "executeQuery" | "executeUpdate" => Some(Sql),
            "File" | "FileInputStream" | "FileOutputStream" => Some(File),
            _ => None,
        },
        Lang::Swift => match name {
            "Process" | "NSTask" | "launch" | "system" | "popen" | "dlopen" => Some(Exec),
            "run" if contains_ci(q, "task") || contains_ci(q, "process") => Some(Exec),
            "unarchiveObject" | "unarchiveTopLevelObjectWithData" => Some(Deserialize),
            "sqlite3_exec" => Some(Sql),
            _ => None,
        },
        Lang::Lua => match q {
            "os.execute" | "io.popen" => Some(Exec),
            "loadstring" | "load" | "dofile" | "loadfile" => Some(Eval),
            "io.open" | "os.remove" => Some(File),
            _ => None,
        },
        Lang::Shell => match name {
            "eval" => Some(Eval),
            "curl" | "wget" => Some(Network),
            _ => None,
        },
        _ => None,
    }
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.as_bytes().windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

fn c_sink(name: &str) -> Option<SinkKind> {
    Some(match name {
        "strcpy" | "strcat" | "sprintf" | "vsprintf" | "gets" | "memcpy" | "memmove" | "strncpy" | "strncat"
        | "alloca" | "_alloca" | "wcscpy" | "wcscat" | "stpcpy" | "bcopy" | "lstrcpy" | "lstrcat" | "lstrcpyA"
        | "lstrcpyW" | "lstrcatA" | "lstrcatW" | "scanf" | "fscanf" | "sscanf" | "vscanf" | "vsscanf" | "vfscanf"
        | "wscanf" | "swscanf" | "swprintf" => Memory,
        "system" | "popen" | "_popen" | "_wsystem" | "_wpopen" | "execl" | "execlp" | "execle" | "execv" | "execvp"
        | "execvpe" | "execve" | "fexecve" | "posix_spawn" | "posix_spawnp" | "CreateProcess" | "CreateProcessA"
        | "CreateProcessW" | "CreateProcessAsUser" | "ShellExecute" | "ShellExecuteA" | "ShellExecuteW"
        | "ShellExecuteEx" | "ShellExecuteExA" | "ShellExecuteExW" | "WinExec" | "dlopen" | "LoadLibrary"
        | "LoadLibraryA" | "LoadLibraryW" | "LoadLibraryEx" | "LoadLibraryExA" | "LoadLibraryExW" => Exec,
        "printf" | "fprintf" | "snprintf" | "vprintf" | "vfprintf" | "vsnprintf" | "dprintf" | "syslog"
        | "vsyslog" => Format,
        "fopen" | "freopen" | "open" | "openat" | "creat" | "unlink" | "remove" | "rename" | "chmod" | "chown" => File,
        "sqlite3_exec" => Sql,
        "curl_easy_perform" => Network,
        _ => return None,
    })
}

fn python(name: &str, q: &str, bare: bool) -> Option<SinkKind> {
    let module = q.strip_suffix(name).and_then(|m| m.strip_suffix('.')).unwrap_or("");
    let last_mod = module.rsplit('.').next().unwrap_or(module);
    match name {
        "eval" | "exec" | "compile" | "execfile" if bare || module == "builtins" => return Some(Eval),
        "__import__" => return Some(Eval),
        "import_module" if last_mod == "importlib" || bare => return Some(Eval),
        "render_template_string" => return Some(Eval),
        "mark_safe" | "Markup" => return Some(Html),
        "system" | "popen" | "Popen" if bare => return Some(Exec),
        "open" if bare => return Some(File),
        "urlopen" => return Some(Network),
        "send_file" | "send_from_directory" => return Some(File),
        "execute" | "executemany" | "executescript" if !bare => return Some(Sql),
        "raw" if q.ends_with("objects.raw") => return Some(Sql),
        "create_subprocess_shell" | "create_subprocess_exec" => return Some(Exec),
        _ => {}
    }
    match last_mod {
        "os" if matches!(name, "system" | "popen" | "popen2" | "popen3" | "popen4" | "startfile")
            || name.starts_with("spawn")
            || name.starts_with("exec") =>
        {
            Some(Exec)
        }
        "os" if matches!(name, "remove" | "unlink" | "open") => Some(File),
        "subprocess"
            if matches!(name, "call" | "run" | "Popen" | "check_output" | "check_call" | "getoutput" | "getstatusoutput") =>
        {
            Some(Exec)
        }
        "commands" if name.starts_with("get") => Some(Exec),
        "pty" if name == "spawn" => Some(Exec),
        "pickle" | "cPickle" | "_pickle" | "dill" | "cloudpickle" | "marshal" if matches!(name, "load" | "loads") => {
            Some(Deserialize)
        }
        "joblib" | "torch" if name == "load" => Some(Deserialize),
        "shelve" if name == "open" => Some(Deserialize),
        "yaml" if matches!(name, "load" | "unsafe_load" | "load_all" | "unsafe_load_all") => Some(Deserialize),
        "jsonpickle" if name == "decode" => Some(Deserialize),
        "io" | "codecs" if name == "open" => Some(File),
        "shutil" if name == "rmtree" => Some(File),
        "requests" | "httpx"
            if matches!(name, "get" | "post" | "put" | "delete" | "patch" | "head" | "request" | "options") =>
        {
            Some(Network)
        }
        _ => None,
    }
}

fn js(name: &str, q: &str, bare: bool) -> Option<SinkKind> {
    let child = q.contains("child_process") || q.starts_with("cp.") || q.starts_with("childProcess.");
    match name {
        "eval" if bare || q == "window.eval" || q == "globalThis.eval" => Some(Eval),
        "Function" if bare || q == "new Function" => Some(Eval),
        "runInNewContext" | "runInThisContext" | "runInContext" | "compileFunction" if q.starts_with("vm.") || bare => {
            Some(Eval)
        }
        "Script" if q.ends_with("vm.Script") => Some(Eval),
        "exec" | "execSync" | "execFile" | "execFileSync" | "spawn" | "spawnSync" | "fork" if child => Some(Exec),
        "exec" | "execSync" | "execFile" | "execFileSync" | "spawn" | "spawnSync" if bare => Some(Exec),
        "exec" if q.starts_with("shell.") => Some(Exec),
        "write" | "writeln" if q.ends_with("document.write") || q.ends_with("document.writeln") => Some(Html),
        "insertAdjacentHTML" | "createContextualFragment" | "setHTMLUnsafe" => Some(Html),
        "html" if q.starts_with("$(") || q.starts_with("jQuery(") => Some(Html),
        "unserialize" | "deserialize" => Some(Deserialize),
        "query" | "execute" | "raw" if !bare => Some(Sql),
        "$queryRaw" | "$executeRaw" | "$queryRawUnsafe" | "$executeRawUnsafe" => Some(Sql),
        "sendFile" | "download" if !bare => Some(File),
        "fetch" if bare || q == "window.fetch" => Some(Network),
        "axios" | "got" if bare => Some(Network),
        "get" | "post" | "put" | "delete" | "patch" | "request" | "head"
            if q.starts_with("axios.") || q.starts_with("got.") || q.starts_with("http.") || q.starts_with("https.") =>
        {
            Some(Network)
        }
        "ajax" | "get" | "post" | "getJSON" if q.starts_with("$.") || q.starts_with("jQuery.") => Some(Network),
        _ if q.starts_with("fs.") || q.starts_with("fsp.") || q.starts_with("fs.promises.") || q.starts_with("fse.") => {
            matches!(
                name,
                "readFile"
                    | "readFileSync"
                    | "writeFile"
                    | "writeFileSync"
                    | "appendFile"
                    | "appendFileSync"
                    | "unlink"
                    | "unlinkSync"
                    | "rm"
                    | "rmSync"
                    | "rmdir"
                    | "rmdirSync"
                    | "createReadStream"
                    | "createWriteStream"
                    | "open"
                    | "openSync"
                    | "rename"
                    | "renameSync"
                    | "copyFile"
                    | "copyFileSync"
            )
            .then_some(File)
        }
        _ => None,
    }
}

fn java(name: &str, q: &str) -> Option<SinkKind> {
    let lq = q.to_ascii_lowercase();
    match name {
        "exec" | "ProcessBuilder" | "loadLibrary" => Some(Exec),
        "load" if q == "System.load" || q == "Runtime.getRuntime().load" => Some(Exec),
        "load" if lq.contains("yaml") => Some(Deserialize),
        "eval" | "parseExpression" | "forName" | "defineClass" => Some(Eval),
        "readObject" | "readUnshared" | "XMLDecoder" | "fromXML" => Some(Deserialize),
        "executeQuery" | "executeUpdate" | "executeLargeUpdate" | "createQuery" | "createNativeQuery" | "createSQLQuery"
        | "addBatch" => Some(Sql),
        "execute" if ["stmt", "statement", "conn", "jdbc", "template", "query", "sql"].iter().any(|k| lq.contains(k)) => {
            Some(Sql)
        }
        "query" | "queryForObject" | "queryForList" | "queryForMap" | "update" | "batchUpdate"
            if lq.contains("jdbctemplate") =>
        {
            Some(Sql)
        }
        "File" | "FileInputStream" | "FileOutputStream" | "FileReader" | "FileWriter" | "RandomAccessFile" => Some(File),
        _ if q.starts_with("Files.") => matches!(
            name,
            "readAllBytes"
                | "readAllLines"
                | "readString"
                | "write"
                | "writeString"
                | "newInputStream"
                | "newOutputStream"
                | "newBufferedReader"
                | "newBufferedWriter"
                | "delete"
                | "deleteIfExists"
                | "copy"
                | "move"
                | "lines"
        )
        .then_some(File),
        "openConnection" | "openStream" => Some(Network),
        "lookup" if ["ctx", "context", "jndi"].iter().any(|k| lq.contains(k)) => Some(Network),
        _ if lq.contains("resttemplate") => Some(Network),
        _ => None,
    }
}

fn go(name: &str, q: &str) -> Option<SinkKind> {
    let lq = q.to_ascii_lowercase();
    let db = ["db.", "tx.", "conn.", "stmt."].iter().any(|k| lq.starts_with(k) || lq.contains(&format!(".{k}")));
    match (q, name) {
        ("exec.Command" | "exec.CommandContext" | "syscall.Exec" | "syscall.ForkExec" | "os.StartProcess", _) => Some(Exec),
        (_, "Query" | "QueryRow" | "Exec" | "QueryContext" | "QueryRowContext" | "ExecContext") if db => Some(Sql),
        ("template.HTML" | "template.JS" | "template.URL" | "template.HTMLAttr", _) => Some(Html),
        (
            "os.Open" | "os.OpenFile" | "os.Create" | "os.Remove" | "os.RemoveAll" | "os.ReadFile" | "os.WriteFile"
            | "ioutil.ReadFile" | "ioutil.WriteFile" | "http.ServeFile",
            _,
        ) => Some(File),
        (
            "http.Get" | "http.Post" | "http.PostForm" | "http.Head" | "http.NewRequest" | "http.NewRequestWithContext"
            | "net.Dial" | "net.DialTimeout",
            _,
        ) => Some(Network),
        ("unsafe.Pointer" | "unsafe.Slice" | "unsafe.String" | "unsafe.StringData", _) => Some(Memory),
        _ => None,
    }
}

fn rust(name: &str, q: &str) -> Option<SinkKind> {
    if let Some(rest) = q.strip_prefix("libc::") {
        if rest == name {
            return c_sink(name);
        }
    }
    let ptr = q.starts_with("ptr::") || q.starts_with("std::ptr::") || q.starts_with("core::ptr::");
    match name {
        "new" if q.ends_with("Command::new") => Some(Exec),
        "transmute" | "transmute_copy" | "from_raw_parts" | "from_raw_parts_mut" | "from_utf8_unchecked"
        | "from_utf8_unchecked_mut" | "get_unchecked" | "get_unchecked_mut" | "set_len" | "assume_init" | "zeroed"
        | "uninitialized" | "from_raw" => Some(Memory),
        "read" | "write" | "copy" | "copy_nonoverlapping" | "read_unaligned" | "write_unaligned" | "read_volatile"
        | "write_volatile"
            if ptr =>
        {
            Some(Memory)
        }
        "open" | "create" if q.ends_with("File::open") || q.ends_with("File::create") => Some(File),
        "read" | "read_to_string" | "write" | "remove_file" | "remove_dir" | "remove_dir_all" | "copy" | "rename"
            if q.starts_with("fs::") || q.starts_with("std::fs::") || q.starts_with("tokio::fs::") =>
        {
            Some(File)
        }
        "get" if q == "reqwest::get" => Some(Network),
        "connect" if q.ends_with("TcpStream::connect") => Some(Network),
        _ => None,
    }
}

fn php(name: &str, q: &str, bare: bool) -> Option<SinkKind> {
    if !bare {
        let lq = q.to_ascii_lowercase();
        return match name.to_ascii_lowercase().as_str() {
            "query" | "exec" | "multi_query" | "real_query" if !lq.starts_with("new ") => Some(Sql),
            "unserialize" => Some(Deserialize),
            _ => None,
        };
    }
    let lname = name.to_ascii_lowercase();
    Some(match lname.as_str() {
        "system" | "exec" | "shell_exec" | "passthru" | "popen" | "proc_open" | "pcntl_exec" => Exec,
        "eval" | "assert" | "create_function" | "call_user_func" | "call_user_func_array" | "extract" => Eval,
        "unserialize" => Deserialize,
        "mysql_query" | "mysqli_query" | "mysqli_multi_query" | "mysqli_real_query" | "pg_query" | "sqlite_query" => Sql,
        "include" | "include_once" | "require" | "require_once" | "file_get_contents" | "file_put_contents" | "fopen"
        | "readfile" | "unlink" | "file" | "move_uploaded_file" => File,
        "curl_exec" | "fsockopen" => Network,
        _ => return None,
    })
}

fn ruby(name: &str, q: &str, bare: bool) -> Option<SinkKind> {
    let recv = q.strip_suffix(name).and_then(|m| m.strip_suffix('.').or_else(|| m.strip_suffix("::"))).unwrap_or("");
    match name {
        "eval" | "instance_eval" | "class_eval" | "module_eval" | "instance_exec" | "class_exec" => Some(Eval),
        "send" | "public_send" | "__send__" | "constantize" | "safe_constantize" => Some(Eval),
        "system" | "exec" | "spawn" if bare || recv == "Kernel" || recv == "Process" => Some(Exec),
        "popen" if bare || recv == "IO" => Some(Exec),
        _ if recv == "Open3" => Some(Exec),
        "load" | "restore" if recv == "Marshal" => Some(Deserialize),
        "load" | "unsafe_load" | "load_file" if recv == "YAML" || recv == "Psych" || recv == "Oj" => Some(Deserialize),
        "open" if bare || recv == "File" || recv == "Kernel" => Some(File),
        "read" | "write" | "readlines" if recv == "File" || recv == "IO" => Some(File),
        "send_file" => Some(File),
        "find_by_sql" => Some(Sql),
        "execute" | "exec_query" if !bare => Some(Sql),
        "html_safe" => Some(Html),
        "raw" if bare => Some(Html),
        "get" | "get_response" | "post" | "post_form" if recv.ends_with("HTTP") => Some(Network),
        "open" if recv == "URI" => Some(Network),
        _ if recv == "HTTParty" || recv == "Faraday" || recv == "RestClient" => Some(Network),
        _ => None,
    }
}

fn csharp(name: &str, q: &str) -> Option<SinkKind> {
    let lq = q.to_ascii_lowercase();
    match name {
        "Start" if q.contains("Process") => Some(Exec),
        "ProcessStartInfo" => Some(Exec),
        "Load" | "LoadFrom" | "LoadFile" | "UnsafeLoadFrom" if q.contains("Assembly") => Some(Eval),
        "EvaluateAsync" | "RunAsync" if q.contains("CSharpScript") => Some(Eval),
        "Deserialize" | "UnsafeDeserialize" | "ReadObject" if !lq.contains("json") => Some(Deserialize),
        "ExecuteReader" | "ExecuteNonQuery" | "ExecuteScalar" | "ExecuteSqlRaw" | "ExecuteSqlCommand" | "FromSqlRaw"
        | "SqlQuery" | "SqlCommand" | "ExecuteReaderAsync" | "ExecuteNonQueryAsync" | "ExecuteScalarAsync"
        | "ExecuteSqlRawAsync" => Some(Sql),
        "FileStream" => Some(File),
        _ if q.starts_with("File.") || q.starts_with("System.IO.File.") => matches!(
            name,
            "ReadAllText"
                | "ReadAllBytes"
                | "ReadAllLines"
                | "WriteAllText"
                | "WriteAllBytes"
                | "WriteAllLines"
                | "AppendAllText"
                | "Delete"
                | "Open"
                | "OpenRead"
                | "OpenWrite"
                | "Copy"
                | "Move"
        )
        .then_some(File),
        "GetAsync" | "PostAsync" | "PutAsync" | "SendAsync" | "GetStringAsync" | "GetByteArrayAsync" | "GetStreamAsync"
            if lq.contains("client") =>
        {
            Some(Network)
        }
        "DownloadString" | "DownloadFile" | "DownloadData" | "UploadString" | "UploadData" | "OpenRead" => Some(Network),
        "Create" if q.ends_with("WebRequest.Create") => Some(Network),
        "Raw" if q.ends_with("Html.Raw") => Some(Html),
        "HtmlString" => Some(Html),
        _ => None,
    }
}
