//! Dangerous functions per language. STUB: knows none.

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

/// Is a call to `name` (last segment) written as `qualified` (may be empty) a sink in `lang`?
pub fn sink_kind(lang: Lang, name: &str, qualified: &str) -> Option<SinkKind> {
    let _ = (lang, name, qualified);
    None
}
