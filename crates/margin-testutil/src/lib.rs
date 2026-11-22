//! Test helpers. `TempDir`: the only place tests may create or delete files.
//!
//! Delete rules (a recursive delete on an empty path wiped a project once):
//! the directory is created by us, lives under the system temp dir, has our name prefix,
//! and is removed entry by entry without following symlinks. Anything else: refuse.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

const PREFIX: &str = "margin-test-";
static SEQ: AtomicU32 = AtomicU32::new(0);

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// New empty directory `<tmp>/margin-test-<tag>-<pid>-<nanos>-<n>`.
    pub fn new(tag: &str) -> Self {
        let tag: String = tag.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        let base = fs::canonicalize(std::env::temp_dir()).expect("temp dir");
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!("{PREFIX}{tag}-{}-{nanos}-{n}", std::process::id()));
        fs::create_dir(&path).expect("create temp dir");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write `rel` (parents created). `rel` must be relative without `..`.
    pub fn write(&self, rel: &str, data: impl AsRef<[u8]>) -> PathBuf {
        let p = self.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("create parents");
        }
        fs::write(&p, data).expect("write file");
        p
    }

    pub fn mkdir(&self, rel: &str) -> PathBuf {
        let p = self.join(rel);
        fs::create_dir_all(&p).expect("create dir");
        p
    }

    pub fn join(&self, rel: &str) -> PathBuf {
        let r = Path::new(rel);
        assert!(r.is_relative() && !rel.is_empty(), "relative path expected: {rel:?}");
        assert!(r.components().all(|c| matches!(c, std::path::Component::Normal(_))), "no `..` in {rel:?}");
        self.path.join(r)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = remove_tree(&self.path);
    }
}

/// Remove a directory this crate created. Refuses any path that does not look like one.
fn remove_tree(dir: &Path) -> std::io::Result<()> {
    let refuse = |why: &str| Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("refusing to delete {}: {why}", dir.display())));
    if !dir.is_absolute() {
        return refuse("not absolute");
    }
    let base = fs::canonicalize(std::env::temp_dir())?;
    if dir.parent() != Some(base.as_path()) {
        return refuse("not directly under the temp dir");
    }
    match dir.file_name().and_then(|n| n.to_str()) {
        Some(n) if n.starts_with(PREFIX) && n.len() > PREFIX.len() => {}
        _ => return refuse("name prefix"),
    }
    if !fs::symlink_metadata(dir)?.file_type().is_dir() {
        return refuse("not a directory");
    }
    remove_inside(dir, 0)?;
    fs::remove_dir(dir)
}

fn remove_inside(dir: &Path, depth: u32) -> std::io::Result<()> {
    if depth > 64 {
        return Err(std::io::Error::other("tree too deep"));
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        // `file_type` does not follow symlinks: a link is unlinked, never entered.
        if entry.file_type()?.is_dir() {
            // Tests may have made it unreadable.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o700));
            }
            remove_inside(&p, depth + 1)?;
            fs::remove_dir(&p)?;
        } else {
            fs::remove_file(&p)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_removes() {
        let t = TempDir::new("self");
        let root = t.path().to_path_buf();
        t.write("a/b/c.txt", "x");
        t.mkdir("d/e");
        assert!(root.join("a/b/c.txt").is_file());
        drop(t);
        assert!(!root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks() {
        let outside = TempDir::new("outside");
        let keep = outside.write("keep.txt", "keep");
        let t = TempDir::new("links");
        std::os::unix::fs::symlink(outside.path(), t.path().join("link")).unwrap();
        drop(t);
        assert!(keep.is_file());
    }

    #[test]
    fn refuses_foreign_paths() {
        assert!(remove_tree(Path::new("")).is_err());
        assert!(remove_tree(Path::new("relative")).is_err());
        assert!(remove_tree(&std::env::temp_dir()).is_err());
        assert!(remove_tree(Path::new("/")).is_err());
    }

    #[test]
    #[should_panic]
    fn join_rejects_parent() {
        TempDir::new("join").join("../x");
    }
}
