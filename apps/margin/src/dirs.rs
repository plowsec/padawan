//! Where things live. All outside any opened folder.
//!
//! | What | macOS | Other unix | Windows |
//! |---|---|---|---|
//! | home | `$HOME` | `$HOME` | `%USERPROFILE%` |
//! | config (`settings.conf`, `state.conf`) | `~/.config/margin` | `$XDG_CONFIG_HOME/margin`, `~/.config/margin` | `%APPDATA%\margin` |
//! | cache (index per folder) | `~/Library/Caches/margin` | `$XDG_CACHE_HOME/margin`, `~/.cache/margin` | `%LOCALAPPDATA%\margin` |

use std::path::{Path, PathBuf};

pub fn home() -> Option<PathBuf> {
    abs_env(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
}

fn abs_env(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os(name)?);
    p.is_absolute().then_some(p)
}

pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return Some(abs_env("APPDATA")?.join("margin"));
    }
    abs_env("XDG_CONFIG_HOME").or_else(|| Some(home()?.join(".config"))).map(|p| p.join("margin"))
}

pub fn cache_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return Some(home()?.join("Library/Caches/margin"));
    }
    if cfg!(windows) {
        return Some(abs_env("LOCALAPPDATA")?.join("margin"));
    }
    abs_env("XDG_CACHE_HOME").or_else(|| Some(home()?.join(".cache"))).map(|p| p.join("margin"))
}

/// Cache directory of one folder: `<cache>/roots/<fnv64 of the canonical path>`.
pub fn root_cache_dir(cache: &Path, canonical_root: &Path) -> PathBuf {
    cache.join("roots").join(format!("{:016x}", fnv64(canonical_root.as_os_str().as_encoded_bytes())))
}

/// FNV-1a. Names a directory; not a security boundary (the engine checks the root stored in the cache).
fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Create `dir` (and parents) private to the user. Only ever called on the config and cache paths above.
pub fn ensure_private(dir: &Path) -> std::io::Result<()> {
    if !dir.is_absolute() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "absolute path required"));
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// macOS per-user temp/cache root (`/private/var/folders/xx/yyyy`): the parent of `$TMPDIR`.
/// Metal writes its shader cache there. `None` when `$TMPDIR` does not look like it.
pub fn darwin_user_dir() -> Option<PathBuf> {
    let tmp = std::fs::canonicalize(abs_env("TMPDIR")?).ok()?;
    let parent = tmp.parent()?;
    (parent.starts_with("/private/var/folders") && parent.components().count() == 6).then(|| parent.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_cache_is_stable_and_distinct() {
        let c = Path::new("/c");
        let a = root_cache_dir(c, Path::new("/x/a"));
        assert_eq!(a, root_cache_dir(c, Path::new("/x/a")));
        assert_ne!(a, root_cache_dir(c, Path::new("/x/b")));
        assert!(a.starts_with("/c/roots"));
        assert_eq!(a.file_name().unwrap().len(), 16);
    }

    #[test]
    fn ensure_private_rejects_relative() {
        assert!(ensure_private(Path::new("")).is_err());
        assert!(ensure_private(Path::new("x")).is_err());
    }
}
