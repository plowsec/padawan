//! Command line. Hand-parsed: a handful of flags, no dependency.

use std::ffi::OsString;
use std::path::PathBuf;

use margin_proto::IndexOpts;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Launcher,
    Gui,
    Worker,
    Help,
    Version,
}

#[derive(Clone, Debug)]
pub struct Args {
    pub role: Role,
    pub no_sandbox: bool,
    /// Launcher: folder or file to open.
    pub path: Option<PathBuf>,
    /// Gui: folder to open at start. Worker: the folder (`--root`).
    pub folder: Option<PathBuf>,
    /// Gui: file inside the folder, relative.
    pub file: Option<String>,
    /// Gui: config dir. Worker: cache dir.
    pub dir: Option<PathBuf>,
    /// Gui: name of the sandbox it runs in, for the status bar.
    pub sandbox: String,
    /// Worker: index options.
    pub opts: IndexOpts,
}

impl Args {
    pub fn parse(it: impl Iterator<Item = OsString>) -> Result<Args, String> {
        let mut a = Args {
            role: Role::Launcher,
            no_sandbox: false,
            path: None,
            folder: None,
            file: None,
            dir: None,
            sandbox: "none".into(),
            opts: IndexOpts::default(),
        };
        let mut it = it.peekable();
        let mut positional_only = false;
        while let Some(arg) = it.next() {
            let s = arg.to_string_lossy().into_owned();
            if positional_only || !s.starts_with("--") {
                if a.path.is_some() {
                    return Err("one path at most".into());
                }
                a.path = Some(PathBuf::from(arg));
                continue;
            }
            let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
            match s.as_str() {
                "--" => positional_only = true,
                "--help" => a.role = Role::Help,
                "--version" => a.role = Role::Version,
                "--no-sandbox" => a.no_sandbox = true,
                "--role" => {
                    a.role = match value("--role")?.to_string_lossy().as_ref() {
                        "gui" => Role::Gui,
                        "worker" => Role::Worker,
                        other => return Err(format!("unknown role {other:?}")),
                    }
                }
                "--folder" | "--root" => a.folder = Some(PathBuf::from(value(&s)?)),
                "--file" => a.file = Some(value("--file")?.to_string_lossy().into_owned()),
                "--config" | "--cache" => a.dir = Some(PathBuf::from(value(&s)?)),
                "--sandbox" => a.sandbox = value("--sandbox")?.to_string_lossy().into_owned(),
                "--gitignore" => a.opts.respect_gitignore = flag(&value("--gitignore")?)?,
                "--hidden" => a.opts.index_hidden = flag(&value("--hidden")?)?,
                "--max-file" => {
                    a.opts.max_file_bytes = value("--max-file")?.to_string_lossy().parse().map_err(|_| "--max-file needs a number".to_owned())?;
                }
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(a)
    }
}

fn flag(v: &OsString) -> Result<bool, String> {
    match v.to_string_lossy().as_ref() {
        "1" => Ok(true),
        "0" => Ok(false),
        other => Err(format!("expected 0 or 1, got {other:?}")),
    }
}

/// Worker arguments for these options (the inverse of the parser above).
pub fn opts_args(opts: &IndexOpts) -> Vec<String> {
    vec![
        "--gitignore".into(),
        u8::from(opts.respect_gitignore).to_string(),
        "--hidden".into(),
        u8::from(opts.index_hidden).to_string(),
        "--max-file".into(),
        opts.max_file_bytes.to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: &[&str]) -> Result<Args, String> {
        Args::parse(v.iter().map(OsString::from))
    }

    #[test]
    fn launcher_forms() {
        let a = parse(&[]).unwrap();
        assert_eq!((a.role, a.no_sandbox, a.path.is_none()), (Role::Launcher, false, true));
        let a = parse(&["--no-sandbox", "/tmp/x"]).unwrap();
        assert!(a.no_sandbox);
        assert_eq!(a.path.unwrap(), PathBuf::from("/tmp/x"));
        assert_eq!(parse(&["--", "--weird"]).unwrap().path.unwrap(), PathBuf::from("--weird"));
        assert!(parse(&["a", "b"]).is_err());
        assert!(parse(&["--nope"]).is_err());
        assert!(parse(&["--role"]).is_err());
        assert_eq!(parse(&["--help"]).unwrap().role, Role::Help);
    }

    #[test]
    fn worker_options_round_trip() {
        let opts = IndexOpts { respect_gitignore: false, index_hidden: true, max_file_bytes: 12345 };
        let mut v = vec!["--role".to_owned(), "worker".into(), "--root".into(), "/r".into(), "--cache".into(), "/c".into()];
        v.extend(opts_args(&opts));
        let a = Args::parse(v.into_iter().map(OsString::from)).unwrap();
        assert_eq!(a.role, Role::Worker);
        assert_eq!(a.opts, opts);
        assert_eq!(a.folder.unwrap(), PathBuf::from("/r"));
        assert_eq!(a.dir.unwrap(), PathBuf::from("/c"));
    }
}
