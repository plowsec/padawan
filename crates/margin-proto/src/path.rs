//! `RelPath`: a path inside the open folder. `/`-separated UTF-8, no `.`/`..`, no empty part, no NUL,
//! no backslash, no leading `/`. `""` is the folder itself. The only path type that crosses the wire.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::limits::{MAX_NAME, MAX_PATH};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelPath(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadPath;

impl fmt::Display for BadPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid relative path")
    }
}

impl std::error::Error for BadPath {}

/// One component: non-empty, not `.`/`..`, no separator, no NUL, no control character.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name != "."
        && name != ".."
        && !name.chars().any(|c| c == '/' || c == '\\' || c == '\0' || c.is_control())
}

impl RelPath {
    /// The open folder itself.
    pub fn root() -> Self {
        Self(String::new())
    }

    pub fn new(s: &str) -> Result<Self, BadPath> {
        if s.is_empty() {
            return Ok(Self::root());
        }
        if s.len() > MAX_PATH || !s.split('/').all(valid_name) {
            return Err(BadPath);
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|c| !c.is_empty())
    }

    /// Last component; `""` for the root.
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or("")
    }

    /// Text after the last `.` of the name, lowercased; `""` if none.
    pub fn ext(&self) -> String {
        let n = self.name();
        match n.rfind('.') {
            Some(i) if i > 0 => n[i + 1..].to_ascii_lowercase(),
            _ => String::new(),
        }
    }

    pub fn parent(&self) -> Option<RelPath> {
        if self.is_root() {
            return None;
        }
        Some(match self.0.rfind('/') {
            Some(i) => Self(self.0[..i].to_owned()),
            None => Self::root(),
        })
    }

    pub fn join(&self, name: &str) -> Result<RelPath, BadPath> {
        if !valid_name(name) {
            return Err(BadPath);
        }
        let s = if self.is_root() { name.to_owned() } else { format!("{}/{name}", self.0) };
        if s.len() > MAX_PATH {
            return Err(BadPath);
        }
        Ok(Self(s))
    }

    pub fn starts_with(&self, dir: &RelPath) -> bool {
        dir.is_root() || self.0 == dir.0 || (self.0.starts_with(&dir.0) && self.0.as_bytes().get(dir.0.len()) == Some(&b'/'))
    }

    /// Lexical join under `root`. Does not touch the disk and does not resolve symlinks: the engine's jail does.
    pub fn under(&self, root: &Path) -> PathBuf {
        let mut p = root.to_path_buf();
        for c in self.components() {
            p.push(c);
        }
        p
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_rejects() {
        for ok in ["", "a", "a/b.rs", "dir with space/é.md", ".hidden/.x"] {
            assert!(RelPath::new(ok).is_ok(), "{ok:?}");
        }
        for bad in ["/", "/a", "a/", "a//b", ".", "..", "a/../b", "a/./b", "a\\b", "a\0b", "a\nb", "../x"] {
            assert!(RelPath::new(bad).is_err(), "{bad:?}");
        }
        assert!(RelPath::new(&"a/".repeat(3000)).is_err());
        assert!(RelPath::new(&"x".repeat(300)).is_err());
    }

    #[test]
    fn parts() {
        let p = RelPath::new("src/app/Main.RS").unwrap();
        assert_eq!(p.name(), "Main.RS");
        assert_eq!(p.ext(), "rs");
        assert_eq!(p.parent().unwrap().as_str(), "src/app");
        assert_eq!(RelPath::new("a").unwrap().parent().unwrap(), RelPath::root());
        assert_eq!(RelPath::root().parent(), None);
        assert_eq!(RelPath::root().join("x").unwrap().as_str(), "x");
        assert!(p.join("..").is_err());
        assert!(p.starts_with(&RelPath::new("src").unwrap()));
        assert!(!RelPath::new("srcx/a").unwrap().starts_with(&RelPath::new("src").unwrap()));
        assert_eq!(RelPath::new(".bashrc").unwrap().ext(), "");
        assert_eq!(p.under(Path::new("/r")), PathBuf::from("/r/src/app/Main.RS"));
    }
}
