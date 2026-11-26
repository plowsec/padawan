//! Saving a buffer that belongs to no folder: the launcher writes that one file.
//!
//! The window process cannot write user files and there is no worker without a folder. The bytes are opaque
//! here: written, never parsed. The request comes from the GUI, which may be subverted, so the path is checked:
//!
//! | Rule | Why |
//! |---|---|
//! | absolute path, or `~/…`; the parent folder exists | no guessing, no directory creation |
//! | not under `~/<SENSITIVE>`, not under Margin's own dirs | credentials, start-up items |
//! | no dot-file directly in the home directory | shell and tool start-up files |
//! | new files only; replace only a file this launcher wrote | nothing that exists is overwritten |
//! | created without the execute bit, never through a symlink | no hook, no redirect |

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use margin_proto::limits::{MAX_NAME, MAX_PATH};
use margin_proto::SavedFile;

use crate::sandbox::SENSITIVE;

/// Files this launcher may replace. A subverted GUI cannot grow the set without creating files.
const MAX_GRANTED: usize = 1024;
static TMP_SEQ: AtomicU32 = AtomicU32::new(0);

pub struct Saver {
    home: Option<PathBuf>,
    own: Vec<PathBuf>,
    /// Canonical paths written here.
    granted: HashSet<PathBuf>,
}

impl Saver {
    /// `own`: Margin's config and cache dirs.
    pub fn new(home: Option<PathBuf>, own: Vec<PathBuf>) -> Self {
        Self { home, own, granted: HashSet::new() }
    }

    /// Write `data` to `path`. `replace`: the caller saved this file before. Returns the canonical path.
    pub fn save(&mut self, path: &str, data: &[u8], replace: bool) -> Result<PathBuf, String> {
        let own: Vec<&Path> = self.own.iter().map(PathBuf::as_path).collect();
        let target = check_target(path, self.home.as_deref(), &own)?;
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if replace && self.granted.contains(&target) {
            write_over(&target, &name, data).map_err(|e| format!("{name}: {e}"))?;
            return Ok(target);
        }
        // `create_new`: fails on anything that exists, a dangling symlink included.
        let mut f = OpenOptions::new().write(true).create_new(true).open(&target).map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => format!("{name} already exists"),
            _ => format!("{name}: {e}"),
        })?;
        f.write_all(data).and_then(|()| f.sync_all()).map_err(|e| format!("{name}: {e}"))?;
        if self.granted.len() < MAX_GRANTED {
            self.granted.insert(target.clone());
        }
        Ok(target)
    }
}

/// One request → its answer (launcher and `--no-sandbox` opener).
pub fn answer(saver: &std::sync::Mutex<Saver>, id: u32, path: &str, data: &[u8], replace: bool) -> SavedFile {
    let res = match saver.lock() {
        Ok(mut s) => s.save(path, data, replace),
        Err(_) => Err("save failed earlier; restart Margin".into()),
    };
    match res {
        Ok(p) => SavedFile { id, path: p.to_string_lossy().into_owned(), error: String::new() },
        Err(error) => SavedFile { id, path: String::new(), error },
    }
}

/// The policy above. Returns `<canonical parent>/<name>`; the file itself is not touched.
pub fn check_target(path: &str, home: Option<&Path>, own: &[&Path]) -> Result<PathBuf, String> {
    if path.is_empty() || path.len() > MAX_PATH || path.chars().any(char::is_control) {
        return Err("not a usable path".into());
    }
    let p = match path.strip_prefix("~/") {
        Some(rest) => home.ok_or("no home directory")?.join(rest),
        None => PathBuf::from(path),
    };
    if !p.is_absolute() {
        return Err("type an absolute path, or ~/…".into());
    }
    let name = match p.components().next_back() {
        Some(Component::Normal(n)) if !path.ends_with(['/', std::path::MAIN_SEPARATOR]) => n.to_str().filter(|n| n.len() <= MAX_NAME).ok_or("not a usable file name")?.to_owned(),
        _ => return Err("that is a folder: add a file name".into()),
    };
    let parent = p.parent().ok_or("that is a folder: add a file name")?;
    let dir = fs::canonicalize(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    if !dir.is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    if let Some(home) = home.and_then(|h| fs::canonicalize(h).ok()) {
        if dir == home && name.starts_with('.') {
            return Err(format!("refusing to write {name}: dot-files in the home directory are start-up files"));
        }
        for s in SENSITIVE {
            if dir.starts_with(home.join(s)) {
                return Err(format!("refusing to write in {}: it holds credentials or start-up items (~/{s})", dir.display()));
            }
        }
    }
    for own in own {
        if fs::canonicalize(own).is_ok_and(|own| dir.starts_with(own)) {
            return Err(format!("refusing to write in {}: Margin keeps its own files there", dir.display()));
        }
    }
    Ok(dir.join(name))
}

/// Replace a regular file: temp file in the same folder, same permissions, `rename` over it.
fn write_over(target: &Path, name: &str, data: &[u8]) -> std::io::Result<()> {
    let old = fs::symlink_metadata(target)?;
    if !old.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file any more"));
    }
    let dir = target.parent().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent"))?;
    let mut cut = name.len().min(160);
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    let tmp = dir.join(format!(".{}.margin-tmp-{}-{}", &name[..cut], std::process::id(), TMP_SEQ.fetch_add(1, Ordering::Relaxed)));
    let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let res = f.write_all(data).and_then(|()| f.set_permissions(old.permissions())).and_then(|()| f.sync_all()).and_then(|()| fs::rename(&tmp, target));
    if res.is_err() {
        // Only the temp file created above.
        let _ = fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use margin_testutil::TempDir;

    fn home() -> TempDir {
        let home = TempDir::new("save-home");
        for d in ["notes", ".ssh", ".config/margin", ".config/autostart", "Library/LaunchAgents", ".cache-margin"] {
            home.mkdir(d);
        }
        home
    }

    #[test]
    fn policy() {
        let home = home();
        let h = fs::canonicalize(home.path()).unwrap();
        let own = h.join(".cache-margin");
        let own = [own.as_path()];
        let abs = |rel: &str| h.join(rel).to_string_lossy().into_owned();
        let check = |p: &str| check_target(p, Some(&h), &own);

        assert_eq!(check(&abs("notes/a.md")).unwrap(), h.join("notes/a.md"));
        assert_eq!(check("~/notes/a.md").unwrap(), h.join("notes/a.md"));
        assert_eq!(check("~/todo.txt").unwrap(), h.join("todo.txt"), "the home directory itself is fine");
        assert_eq!(check(&abs("notes/../notes/.hidden")).unwrap(), h.join("notes/.hidden"), "dot-files elsewhere are fine");

        for bad in ["~/.zshrc", "~/.profile", "~/.ssh/authorized_keys", "~/.config/autostart/x.desktop", "~/.config/margin/settings.conf", "~/Library/LaunchAgents/x.plist", "~/.cache-margin/x", "~/notes/../.bashrc"] {
            assert!(check(bad).is_err(), "{bad} must be refused");
        }
        for bad in ["", "a.md", "notes/a.md", "~", "~/notes/", "~/missing/a.md", "/", "~/notes/..", "~/notes/a\nb"] {
            assert!(check(bad).is_err(), "{bad:?} is not a file path");
        }
        assert!(check(&format!("~/notes/{}", "x".repeat(300))).is_err());
        assert!(check_target("~/a.md", None, &[]).is_err(), "no home, no ~");
    }

    #[cfg(unix)]
    #[test]
    fn policy_sees_through_symlinks() {
        let home = home();
        let h = fs::canonicalize(home.path()).unwrap();
        std::os::unix::fs::symlink(h.join(".ssh"), h.join("notes/innocent")).unwrap();
        assert!(check_target("~/notes/innocent/authorized_keys", Some(&h), &[]).is_err());
        // A link as the file itself is never written through.
        home.write("notes/real.txt", "keep");
        std::os::unix::fs::symlink(h.join("notes/real.txt"), h.join("notes/link.txt")).unwrap();
        std::os::unix::fs::symlink(h.join("notes/nowhere"), h.join("notes/dangling.txt")).unwrap();
        let mut s = Saver::new(Some(h.clone()), vec![]);
        assert!(s.save("~/notes/link.txt", b"x", false).is_err());
        assert!(s.save("~/notes/dangling.txt", b"x", true).is_err());
        assert_eq!(fs::read_to_string(h.join("notes/real.txt")).unwrap(), "keep");
        assert!(!h.join("notes/nowhere").exists());
    }

    #[test]
    fn new_files_only_then_replace_its_own() {
        let home = home();
        let h = fs::canonicalize(home.path()).unwrap();
        home.write("notes/theirs.md", "theirs");
        let mut s = Saver::new(Some(h.clone()), vec![]);

        assert!(s.save("~/notes/theirs.md", b"mine", false).unwrap_err().contains("already exists"));
        assert!(s.save("~/notes/theirs.md", b"mine", true).unwrap_err().contains("already exists"), "replace needs a file written here");
        assert_eq!(fs::read_to_string(h.join("notes/theirs.md")).unwrap(), "theirs");

        let p = s.save("~/notes/a.md", b"one", false).unwrap();
        assert_eq!(p, h.join("notes/a.md"));
        assert_eq!(fs::read(&p).unwrap(), b"one");
        assert!(s.save("~/notes/a.md", b"two", false).is_err(), "Save As onto it again is refused");
        assert_eq!(s.save(&p.to_string_lossy(), b"two, longer", true).unwrap(), p);
        assert_eq!(fs::read(&p).unwrap(), b"two, longer");
        s.save("~/notes/a.md", b"3", true).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"3");
        let left: Vec<String> = fs::read_dir(h.join("notes")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.contains("margin-tmp")).collect();
        assert!(left.is_empty(), "{left:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o111, 0, "not executable");
        }
        // Another launcher (a new session) has no grant.
        assert!(Saver::new(Some(h), vec![]).save("~/notes/a.md", b"x", true).is_err());
    }
}
