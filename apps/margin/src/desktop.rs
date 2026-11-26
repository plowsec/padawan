//! `--install-desktop` / `--uninstall-desktop`: menu entry and icons for the current user (Linux, BSD).
//!
//! Wayland shows no icon a window sets itself: the compositor looks up `<app id>.desktop` and its `Icon=`.
//!
//! | File, under `$XDG_DATA_HOME` (else `~/.local/share`) | What |
//! |---|---|
//! | `applications/margin.desktop` | entry, `Exec=` the binary that ran the install |
//! | `icons/hicolor/<n>x<n>/apps/margin.png` | icon, `n` in [`SIZES`] |
//!
//! Uninstall unlinks these files only, and only when the entry carries [`MARK`].

use std::path::{Path, PathBuf};

use margin_gui::APP_ID;

pub const SIZES: [u32; 5] = [32, 48, 64, 128, 256];
/// Line of an entry written here.
pub const MARK: &str = "X-Margin-Installed=1";

/// `$XDG_DATA_HOME`, else `~/.local/share`.
pub fn data_dir() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute());
    xdg.or_else(|| Some(crate::dirs::home()?.join(".local/share")))
}

pub fn entry_path(data: &Path) -> PathBuf {
    data.join("applications").join(format!("{APP_ID}.desktop"))
}

pub fn icon_path(data: &Path, size: u32) -> PathBuf {
    data.join("icons/hicolor").join(format!("{size}x{size}")).join("apps").join(format!("{APP_ID}.png"))
}

/// Text of the entry. `exe`: absolute, UTF-8, no control character.
pub fn entry(exe: &Path) -> Result<String, String> {
    let s = exe.to_str().filter(|s| exe.is_absolute() && !s.chars().any(char::is_control)).ok_or_else(|| format!("cannot write {exe:?} into a desktop entry"))?;
    // Quoted argument: `"`, backtick, `$`, `\` take a backslash, and a backslash is `\\` in a desktop string. `%` is a field code.
    let mut quoted = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=Margin\nGenericName=Text Editor\nComment=Notes, files and reading code\nExec=\"{quoted}\" %f\nIcon={APP_ID}\nTerminal=false\nCategories=Utility;TextEditor;Development;\nStartupWMClass={APP_ID}\n{MARK}\n"
    ))
}

fn check(data: &Path) -> Result<(), String> {
    if data.is_absolute() {
        Ok(())
    } else {
        Err("no home directory: cannot place the desktop entry".into())
    }
}

fn io(path: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", path.display())
}

/// Writes the entry and the icons under `data`. Returns the files written.
pub fn install(data: &Path, exe: &Path) -> Result<Vec<PathBuf>, String> {
    check(data)?;
    let text = entry(exe)?;
    let path = entry_path(data);
    if std::fs::read_to_string(&path).is_ok_and(|old| !old.lines().any(|l| l == MARK)) {
        return Err(format!("{}: exists and was not written by margin", path.display()));
    }
    let mut files = vec![(path, text.into_bytes())];
    for size in SIZES {
        let png = margin_shot::png::encode(size, size, &margin_theme::icon::rgba(size, 0.03)).map_err(|e| e.to_string())?;
        files.push((icon_path(data, size), png));
    }
    let mut done = Vec::new();
    for (path, bytes) in files {
        let dir = path.parent().ok_or("no parent")?;
        std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
        std::fs::write(&path, bytes).map_err(|e| io(&path, e))?;
        done.push(path);
    }
    touch(&data.join("icons/hicolor"));
    Ok(done)
}

/// Unlinks what `install` wrote. Returns the files removed. No entry with [`MARK`]: nothing is touched.
pub fn uninstall(data: &Path) -> Result<Vec<PathBuf>, String> {
    check(data)?;
    let path = entry_path(data);
    match std::fs::read_to_string(&path) {
        Ok(text) if text.lines().any(|l| l == MARK) => {}
        Ok(_) => return Err(format!("{}: not written by margin, left alone", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(&path, e)),
    }
    let mut done = Vec::new();
    for p in std::iter::once(path).chain(SIZES.into_iter().map(|n| icon_path(data, n))) {
        match std::fs::remove_file(&p) {
            Ok(()) => done.push(p),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(&p, e)),
        }
    }
    touch(&data.join("icons/hicolor"));
    Ok(done)
}

/// GTK rescans an icon theme when the mtime of its top directory changes. Best effort.
fn touch(dir: &Path) {
    if let Ok(f) = std::fs::File::open(dir) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// The two command-line roles.
pub fn run(remove: bool) -> Result<(), String> {
    use std::io::Write as _;
    if cfg!(any(target_os = "macos", windows)) {
        return Err("desktop entries are for Linux and BSD".into());
    }
    let data = data_dir().ok_or("no home directory: cannot place the desktop entry")?;
    let files = if remove {
        uninstall(&data)?
    } else {
        let exe = std::env::current_exe().and_then(std::fs::canonicalize).map_err(|e| format!("cannot find my own binary: {e}"))?;
        install(&data, &exe)?
    };
    let mut out = std::io::stdout();
    for f in &files {
        let _ = writeln!(out, "{} {}", if remove { "removed" } else { "wrote" }, f.display());
    }
    if files.is_empty() {
        let _ = writeln!(out, "nothing to remove");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use margin_testutil::TempDir;

    #[test]
    fn entry_quotes_the_binary_path() {
        let e = entry(Path::new("/opt/my tools/margin")).unwrap();
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("\nExec=\"/opt/my tools/margin\" %f\n"));
        assert!(e.contains("\nIcon=margin\n") && e.contains("\nStartupWMClass=margin\n"));
        assert!(e.lines().any(|l| l == MARK));
        let e = entry(Path::new("/a/100%/$x/q\"/b`/c\\d")).unwrap();
        assert!(e.contains("\nExec=\"/a/100%%/\\\\$x/q\\\\\"/b\\\\`/c\\\\\\\\d\" %f\n"), "{e}");
        assert!(entry(Path::new("margin")).is_err());
        assert!(entry(Path::new("")).is_err());
        assert!(entry(Path::new("/a\nExec=evil")).is_err());
    }

    #[test]
    fn install_then_uninstall() {
        let tmp = TempDir::new("desktop");
        let data = tmp.path().join("share");
        let other = data.join("icons/hicolor/48x48/apps/other.png");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, b"x").unwrap();

        let files = install(&data, Path::new("/usr/local/bin/margin")).unwrap();
        assert_eq!(files.len(), 1 + SIZES.len());
        assert!(std::fs::read_to_string(entry_path(&data)).unwrap().contains("Exec=\"/usr/local/bin/margin\" %f"));
        for size in SIZES {
            let png = std::fs::read(icon_path(&data, size)).unwrap();
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), size);
        }
        // Again: same files, new binary path.
        install(&data, Path::new("/home/u/bin/margin")).unwrap();
        assert!(std::fs::read_to_string(entry_path(&data)).unwrap().contains("Exec=\"/home/u/bin/margin\" %f"));

        let removed = uninstall(&data).unwrap();
        assert_eq!(removed.len(), 1 + SIZES.len());
        assert!(!entry_path(&data).exists() && !icon_path(&data, 48).exists());
        assert!(other.exists(), "files of other apps stay");
        assert!(data.join("icons/hicolor/48x48/apps").is_dir(), "directories stay");
        assert!(uninstall(&data).unwrap().is_empty());
    }

    #[test]
    fn a_foreign_entry_is_left_alone() {
        let tmp = TempDir::new("desktop-foreign");
        let data = tmp.path().join("share");
        let path = entry_path(&data);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[Desktop Entry]\nName=Someone else\n").unwrap();
        let icon = icon_path(&data, 48);
        std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
        std::fs::write(&icon, b"theirs").unwrap();

        assert!(install(&data, Path::new("/usr/bin/margin")).is_err());
        assert!(uninstall(&data).is_err());
        assert_eq!(std::fs::read(&icon).unwrap(), b"theirs");
        assert!(std::fs::read_to_string(&path).unwrap().contains("Someone else"));
        assert!(install(Path::new(""), Path::new("/usr/bin/margin")).is_err());
        assert!(uninstall(Path::new("rel")).is_err());
    }
}
