//! Language of a file, from its name.
//!
//! | Source | Rule |
//! |---|---|
//! | well-known file name | exact match (`Makefile`, `Dockerfile`, `.bashrc`, `Gemfile`…), then `Dockerfile.*` |
//! | extension | after the last `.` of the file name, ASCII case-insensitive |
//! | anything else | `Plain` |

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

/// Exact file names (case-sensitive).
const NAMES: &[(&str, Lang)] = &[
    ("Makefile", Lang::Makefile),
    ("makefile", Lang::Makefile),
    ("GNUmakefile", Lang::Makefile),
    ("Kbuild", Lang::Makefile),
    ("Dockerfile", Lang::Shell),
    ("dockerfile", Lang::Shell),
    ("Containerfile", Lang::Shell),
    ("PKGBUILD", Lang::Shell),
    ("APKBUILD", Lang::Shell),
    (".bashrc", Lang::Shell),
    (".bash_profile", Lang::Shell),
    (".bash_login", Lang::Shell),
    (".bash_logout", Lang::Shell),
    (".bash_aliases", Lang::Shell),
    (".profile", Lang::Shell),
    (".zshrc", Lang::Shell),
    (".zshenv", Lang::Shell),
    (".zprofile", Lang::Shell),
    (".zlogin", Lang::Shell),
    (".zlogout", Lang::Shell),
    (".kshrc", Lang::Shell),
    (".envrc", Lang::Shell),
    (".env", Lang::Shell),
    ("Rakefile", Lang::Ruby),
    ("Gemfile", Lang::Ruby),
    ("Guardfile", Lang::Ruby),
    ("Vagrantfile", Lang::Ruby),
    ("Podfile", Lang::Ruby),
    ("Fastfile", Lang::Ruby),
    ("Brewfile", Lang::Ruby),
    ("Cargo.lock", Lang::Toml),
    ("Pipfile", Lang::Toml),
    ("poetry.lock", Lang::Toml),
    ("uv.lock", Lang::Toml),
    ("SConstruct", Lang::Python),
    ("SConscript", Lang::Python),
    ("BUILD", Lang::Python),
    ("WORKSPACE", Lang::Python),
    (".babelrc", Lang::Json),
    (".eslintrc", Lang::Json),
    (".prettierrc", Lang::Json),
    (".clang-format", Lang::Yaml),
    (".clang-tidy", Lang::Yaml),
];

/// Extensions, lowercase.
const EXTS: &[(&str, Lang)] = &[
    ("md", Lang::Markdown),
    ("markdown", Lang::Markdown),
    ("mdown", Lang::Markdown),
    ("mkd", Lang::Markdown),
    ("mdx", Lang::Markdown),
    ("c", Lang::C),
    ("h", Lang::C),
    ("cc", Lang::Cpp),
    ("cpp", Lang::Cpp),
    ("cxx", Lang::Cpp),
    ("c++", Lang::Cpp),
    ("hpp", Lang::Cpp),
    ("hh", Lang::Cpp),
    ("hxx", Lang::Cpp),
    ("h++", Lang::Cpp),
    ("ipp", Lang::Cpp),
    ("tpp", Lang::Cpp),
    ("inl", Lang::Cpp),
    ("ino", Lang::Cpp),
    ("cu", Lang::Cpp),
    ("cuh", Lang::Cpp),
    ("rs", Lang::Rust),
    ("go", Lang::Go),
    ("java", Lang::Java),
    ("js", Lang::JavaScript),
    ("mjs", Lang::JavaScript),
    ("cjs", Lang::JavaScript),
    ("jsx", Lang::JavaScript),
    ("ts", Lang::TypeScript),
    ("mts", Lang::TypeScript),
    ("cts", Lang::TypeScript),
    ("tsx", Lang::TypeScript),
    ("py", Lang::Python),
    ("pyw", Lang::Python),
    ("pyi", Lang::Python),
    ("bzl", Lang::Python),
    ("cs", Lang::CSharp),
    ("csx", Lang::CSharp),
    ("php", Lang::Php),
    ("phtml", Lang::Php),
    ("php3", Lang::Php),
    ("php4", Lang::Php),
    ("php5", Lang::Php),
    ("php7", Lang::Php),
    ("php8", Lang::Php),
    ("phps", Lang::Php),
    ("rb", Lang::Ruby),
    ("rake", Lang::Ruby),
    ("gemspec", Lang::Ruby),
    ("ru", Lang::Ruby),
    ("erb", Lang::Ruby),
    ("kt", Lang::Kotlin),
    ("kts", Lang::Kotlin),
    ("swift", Lang::Swift),
    ("sh", Lang::Shell),
    ("bash", Lang::Shell),
    ("zsh", Lang::Shell),
    ("ksh", Lang::Shell),
    ("ash", Lang::Shell),
    ("dash", Lang::Shell),
    ("fish", Lang::Shell),
    ("command", Lang::Shell),
    ("dockerfile", Lang::Shell),
    ("ebuild", Lang::Shell),
    ("json", Lang::Json),
    ("jsonc", Lang::Json),
    ("json5", Lang::Json),
    ("jsonl", Lang::Json),
    ("ndjson", Lang::Json),
    ("geojson", Lang::Json),
    ("webmanifest", Lang::Json),
    ("ipynb", Lang::Json),
    ("har", Lang::Json),
    ("toml", Lang::Toml),
    ("yml", Lang::Yaml),
    ("yaml", Lang::Yaml),
    ("html", Lang::Html),
    ("htm", Lang::Html),
    ("xhtml", Lang::Html),
    ("xml", Lang::Html),
    ("svg", Lang::Html),
    ("xsl", Lang::Html),
    ("xslt", Lang::Html),
    ("plist", Lang::Html),
    ("vue", Lang::Html),
    ("svelte", Lang::Html),
    ("csproj", Lang::Html),
    ("vcxproj", Lang::Html),
    ("props", Lang::Html),
    ("xaml", Lang::Html),
    ("css", Lang::Css),
    ("scss", Lang::Css),
    ("less", Lang::Css),
    ("sql", Lang::Sql),
    ("lua", Lang::Lua),
    ("mk", Lang::Makefile),
    ("mak", Lang::Makefile),
    ("make", Lang::Makefile),
    ("diff", Lang::Diff),
    ("patch", Lang::Diff),
];

impl Lang {
    /// Every variant, `Plain` first. Added for tests and pickers.
    pub const ALL: [Lang; 25] = [
        Lang::Plain,
        Lang::Markdown,
        Lang::C,
        Lang::Cpp,
        Lang::Rust,
        Lang::Go,
        Lang::Java,
        Lang::JavaScript,
        Lang::TypeScript,
        Lang::Python,
        Lang::CSharp,
        Lang::Php,
        Lang::Ruby,
        Lang::Kotlin,
        Lang::Swift,
        Lang::Shell,
        Lang::Json,
        Lang::Toml,
        Lang::Yaml,
        Lang::Html,
        Lang::Css,
        Lang::Sql,
        Lang::Lua,
        Lang::Makefile,
        Lang::Diff,
    ];

    /// From a file name or path (extension, else well-known names like `Makefile`, `Dockerfile`, `.bashrc`).
    /// Case-insensitive on the extension. Unknown → `Plain`.
    pub fn from_path(path: &str) -> Lang {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        if let Some(&(_, l)) = NAMES.iter().find(|(n, _)| *n == name) {
            return l;
        }
        if name.starts_with("Dockerfile.") || name.starts_with("Containerfile.") {
            return Lang::Shell;
        }
        if name.starts_with("Makefile.") {
            return Lang::Makefile;
        }
        let Some(dot) = name.rfind('.') else {
            return Lang::Plain;
        };
        if dot == 0 {
            return Lang::Plain;
        }
        let ext = name.get(dot + 1..).unwrap_or("");
        EXTS.iter()
            .find(|(e, _)| e.eq_ignore_ascii_case(ext))
            .map_or(Lang::Plain, |&(_, l)| l)
    }

    /// Display name: `"Rust"`, `"C++"`, `"Plain text"`.
    pub fn name(self) -> &'static str {
        match self {
            Lang::Plain => "Plain text",
            Lang::Markdown => "Markdown",
            Lang::C => "C",
            Lang::Cpp => "C++",
            Lang::Rust => "Rust",
            Lang::Go => "Go",
            Lang::Java => "Java",
            Lang::JavaScript => "JavaScript",
            Lang::TypeScript => "TypeScript",
            Lang::Python => "Python",
            Lang::CSharp => "C#",
            Lang::Php => "PHP",
            Lang::Ruby => "Ruby",
            Lang::Kotlin => "Kotlin",
            Lang::Swift => "Swift",
            Lang::Shell => "Shell",
            Lang::Json => "JSON",
            Lang::Toml => "TOML",
            Lang::Yaml => "YAML",
            Lang::Html => "HTML",
            Lang::Css => "CSS",
            Lang::Sql => "SQL",
            Lang::Lua => "Lua",
            Lang::Makefile => "Makefile",
            Lang::Diff => "Diff",
        }
    }

    /// `outline` finds functions for this language.
    pub fn has_outline(self) -> bool {
        matches!(
            self,
            Lang::C
                | Lang::Cpp
                | Lang::Rust
                | Lang::Go
                | Lang::Java
                | Lang::JavaScript
                | Lang::TypeScript
                | Lang::Python
                | Lang::CSharp
                | Lang::Php
                | Lang::Ruby
                | Lang::Kotlin
                | Lang::Swift
                | Lang::Lua
                | Lang::Shell
        )
    }

    /// Line-comment prefix (`//`, `#`, `--`), for the toggle-comment action.
    pub fn line_comment(self) -> Option<&'static str> {
        match self {
            Lang::C
            | Lang::Cpp
            | Lang::Rust
            | Lang::Go
            | Lang::Java
            | Lang::JavaScript
            | Lang::TypeScript
            | Lang::CSharp
            | Lang::Php
            | Lang::Kotlin
            | Lang::Swift => Some("//"),
            Lang::Python | Lang::Ruby | Lang::Shell | Lang::Toml | Lang::Yaml | Lang::Makefile => Some("#"),
            Lang::Sql | Lang::Lua => Some("--"),
            Lang::Plain | Lang::Markdown | Lang::Json | Lang::Html | Lang::Css | Lang::Diff => None,
        }
    }

    /// Prose: the editor soft-wraps it by default (Markdown, plain text).
    pub fn is_prose(self) -> bool {
        matches!(self, Lang::Plain | Lang::Markdown)
    }
}
