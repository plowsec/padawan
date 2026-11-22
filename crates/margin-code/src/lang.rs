//! Language of a file. STUB: the implementer fills the tables.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lang {
    #[default]
    Plain,
    Markdown,
    C,
    Cpp,
    Rust,
    Go,
    Java,
    JavaScript,
    TypeScript,
    Python,
    CSharp,
    Php,
    Ruby,
    Kotlin,
    Swift,
    Shell,
    Json,
    Toml,
    Yaml,
    Html,
    Css,
    Sql,
    Lua,
    Makefile,
    Diff,
}

impl Lang {
    /// From a file name or path (extension, else well-known names like `Makefile`, `Dockerfile`, `.bashrc`).
    /// Case-insensitive on the extension. Unknown → `Plain`.
    pub fn from_path(path: &str) -> Lang {
        let _ = path;
        Lang::Plain
    }

    /// Display name: `"Rust"`, `"C++"`, `"Plain text"`.
    pub fn name(self) -> &'static str {
        "Plain text"
    }

    /// `outline` finds functions for this language.
    pub fn has_outline(self) -> bool {
        false
    }

    /// Line-comment prefix (`//`, `#`, `--`), for the toggle-comment action.
    pub fn line_comment(self) -> Option<&'static str> {
        None
    }

    /// Prose: the editor soft-wraps it by default (Markdown, plain text).
    pub fn is_prose(self) -> bool {
        matches!(self, Lang::Plain | Lang::Markdown)
    }
}
