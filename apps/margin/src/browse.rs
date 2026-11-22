//! Folder picker backend: the sub-directories of one directory. Names only, no file content.
//! Runs in the launcher (the GUI cannot read user directories) or on a thread with `--no-sandbox`.

use std::path::{Path, PathBuf};

use margin_proto::DirListing;

const MAX_DIRS: usize = 5000;

/// `path` empty / `None`: the home directory.
pub fn list(path: Option<&Path>) -> DirListing {
    let start: PathBuf = match path {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => match crate::dirs::home() {
            Some(h) => h,
            None => PathBuf::from("/"),
        },
    };
    let mut out = DirListing { path: start.to_string_lossy().into_owned(), ..Default::default() };
    if !start.is_absolute() {
        out.error = "not an absolute path".into();
        return out;
    }
    let dir = match std::fs::canonicalize(&start) {
        Ok(d) => d,
        Err(e) => {
            out.error = e.to_string();
            return out;
        }
    };
    out.path = dir.to_string_lossy().into_owned();
    out.parent = dir.parent().map(|p| p.to_string_lossy().into_owned());
    let rd = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(e) => {
            out.error = e.to_string();
            return out;
        }
    };
    for entry in rd.flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        if !margin_proto::path::valid_name(&name) {
            continue;
        }
        // `metadata` follows symlinks: a link to a directory is a directory you can open.
        if std::fs::metadata(entry.path()).map(|m| m.is_dir()).unwrap_or(false) {
            out.dirs.push(name);
            if out.dirs.len() >= MAX_DIRS {
                break;
            }
        }
    }
    // Plain names first, dot-directories after; case-insensitive.
    out.dirs.sort_by_cached_key(|n| (n.starts_with('.'), n.to_lowercase()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use margin_testutil::TempDir;

    #[test]
    fn lists_directories_only_sorted() {
        let t = TempDir::new("browse");
        t.mkdir("beta");
        t.mkdir("Alpha");
        t.mkdir(".hidden");
        t.write("file.txt", "x");
        let l = list(Some(t.path()));
        assert_eq!(l.error, "");
        assert_eq!(l.dirs, vec!["Alpha", "beta", ".hidden"]);
        assert_eq!(l.parent.as_deref(), t.path().parent().and_then(|p| p.to_str()));
    }

    #[test]
    fn errors_are_reported_not_panics() {
        assert!(!list(Some(Path::new("relative"))).error.is_empty());
        assert!(!list(Some(Path::new("/definitely/not/here"))).error.is_empty());
        assert!(list(None).error.is_empty());
    }
}
