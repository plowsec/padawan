//! OS sandbox for the two child roles, and the rule for which folders may be opened.
//!
//! macOS: Seatbelt through `/usr/bin/sandbox-exec` (no unsafe, no FFI). Paths go in as `-D` parameters,
//! never spliced into the profile text. A sandboxed process cannot sandbox its children again
//! (`sandbox_apply: Operation not permitted`), so the launcher, which starts both children, stays outside.
//!
//! | Child | Network | Exec / fork | Read | Write |
//! |---|---|---|---|---|
//! | gui | no | no | system files, own binary, config dir. Nothing under the home directory or `/Volumes` | config dir, per-user temp (Metal shader cache) |
//! | worker | no | no | system libraries, own binary, the folder, its cache dir | the folder, its cache dir |
//!
//! Other platforms: no sandbox yet (`Sandbox::None`); the status bar says so.

use std::path::{Path, PathBuf};
use std::process::Command;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// GUI: AppKit and Metal need too much for deny-default; allow, then take away what matters.
const GUI_PROFILE: &str = r#"(version 1)
(allow default)
(deny network*)
(deny process-exec*)
(deny process-fork)
(deny lsopen)
(deny appleevent-send)
(deny job-creation)
(deny file-write*)
(allow file-write* (subpath (param "CONFIG")) (subpath (param "DARWIN_USER")) (literal "/dev/null") (literal "/dev/dtracehelper"))
(deny file-read* (subpath "/Users") (subpath (param "HOME")) (subpath "/Volumes") (subpath "/private/etc/ssh"))
(allow file-read-metadata (subpath "/Users") (subpath (param "HOME")))
(allow file-read* (literal (param "EXE")) (subpath (param "CONFIG")))
(allow process-exec* (literal (param "EXE")))
"#;

/// Worker: deny-default. It reads hostile files; it gets the folder and nothing else.
const WORKER_PROFILE: &str = r#"(version 1)
(deny default)
(allow process-exec* (literal (param "EXE")))
(allow file-read* (literal (param "EXE")))
(allow file-read-metadata)
(allow file-read*
  (literal "/")
  (subpath "/usr/lib") (subpath "/usr/share") (subpath "/System") (subpath "/Library/Apple")
  (subpath "/private/var/db/dyld") (subpath "/private/var/db/timezone")
  (literal "/dev/null") (literal "/dev/urandom") (literal "/dev/random"))
(allow file-read* file-write* (subpath (param "ROOT")) (subpath (param "CACHE")))
(allow file-write-data (literal "/dev/null"))
(allow sysctl-read)
(allow signal (target self))
(allow process-info* (target self))
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sandbox {
    Seatbelt,
    None,
}

impl Sandbox {
    pub fn name(self) -> &'static str {
        match self {
            Sandbox::Seatbelt => "seatbelt",
            Sandbox::None => "none",
        }
    }

    /// What this machine offers. macOS without `sandbox-exec` is an error: refuse to run open rather than silently unconfined.
    pub fn detect() -> Result<Sandbox, String> {
        if cfg!(target_os = "macos") {
            if Path::new(SANDBOX_EXEC).is_file() {
                Ok(Sandbox::Seatbelt)
            } else {
                Err(format!("{SANDBOX_EXEC} is missing; run with --no-sandbox to start without the OS sandbox"))
            }
        } else {
            Ok(Sandbox::None)
        }
    }
}

/// All paths canonical and absolute (Seatbelt matches real paths: `/private/var`, not `/var`).
pub struct GuiPaths<'a> {
    pub exe: &'a Path,
    pub config: &'a Path,
    pub home: &'a Path,
    /// `dirs::darwin_user_dir()`; falls back to the config dir (then Metal just cannot cache shaders).
    pub darwin_user: &'a Path,
}

pub struct WorkerPaths<'a> {
    pub exe: &'a Path,
    pub root: &'a Path,
    pub cache: &'a Path,
}

fn param(cmd: &mut Command, key: &str, value: &Path) {
    let mut kv = std::ffi::OsString::from(format!("{key}="));
    kv.push(value);
    cmd.arg("-D").arg(kv);
}

/// Command that runs `exe` as the GUI inside its sandbox. The caller adds the role arguments and stdio.
pub fn gui_command(sb: Sandbox, p: &GuiPaths<'_>) -> Command {
    match sb {
        Sandbox::Seatbelt => {
            let mut c = Command::new(SANDBOX_EXEC);
            c.arg("-p").arg(GUI_PROFILE);
            param(&mut c, "EXE", p.exe);
            param(&mut c, "CONFIG", p.config);
            param(&mut c, "HOME", p.home);
            param(&mut c, "DARWIN_USER", p.darwin_user);
            c.arg(p.exe);
            c
        }
        Sandbox::None => Command::new(p.exe),
    }
}

/// Command that runs `exe` as a worker confined to `root` + `cache`.
/// Environment cleared, working directory `/` (never the hostile folder).
pub fn worker_command(sb: Sandbox, p: &WorkerPaths<'_>) -> Command {
    let mut c = match sb {
        Sandbox::Seatbelt => {
            let mut c = Command::new(SANDBOX_EXEC);
            c.arg("-p").arg(WORKER_PROFILE);
            param(&mut c, "EXE", p.exe);
            param(&mut c, "ROOT", p.root);
            param(&mut c, "CACHE", p.cache);
            c.arg(p.exe);
            c
        }
        Sandbox::None => Command::new(p.exe),
    };
    c.env_clear().current_dir("/");
    c
}

/// May this folder be opened? The worker gets read/write on it, so refuse the places where a write means
/// code execution or credential theft later. Returns the canonical path.
pub fn check_root(path: &Path, home: Option<&Path>, own_dirs: &[&Path]) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("not an absolute path".into());
    }
    let root = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    if root.parent().is_none() {
        return Err("refusing to open the filesystem root".into());
    }
    if let Some(home) = home.and_then(|h| std::fs::canonicalize(h).ok()) {
        if home.starts_with(&root) {
            return Err(format!("refusing to open {}: it is or contains your home directory. Open a sub-folder.", root.display()));
        }
        for sensitive in ["Library", ".ssh", ".gnupg", ".aws", ".kube", ".docker", ".config", ".local", ".cargo", ".rustup", ".claude"] {
            let s = home.join(sensitive);
            if root.starts_with(&s) || s.starts_with(&root) {
                return Err(format!("refusing to open {}: it holds credentials or start-up items (~/{sensitive})", root.display()));
            }
        }
    }
    for own in own_dirs {
        if let Ok(own) = std::fs::canonicalize(own) {
            if root.starts_with(&own) || own.starts_with(&root) {
                return Err(format!("refusing to open {}: Margin keeps its own files there", root.display()));
            }
        }
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use margin_testutil::TempDir;

    #[test]
    fn root_policy() {
        let home = TempDir::new("home");
        home.mkdir("Library/LaunchAgents");
        home.mkdir(".ssh");
        home.mkdir("notes/deep");
        home.mkdir(".config/margin");
        home.write("notes/file.txt", "x");
        let h = Some(home.path());
        let own = home.path().join(".config/margin");
        let own = [own.as_path()];
        assert!(check_root(&home.path().join("notes"), h, &own).is_ok());
        assert!(check_root(&home.path().join("notes/deep"), h, &own).is_ok());
        for bad in ["", "Library", "Library/LaunchAgents", ".ssh", ".config", ".config/margin", "notes/file.txt", "missing"] {
            let p = if bad.is_empty() { home.path().to_path_buf() } else { home.path().join(bad) };
            assert!(check_root(&p, h, &own).is_err(), "{bad:?} must be refused");
        }
        assert!(check_root(Path::new("/"), h, &own).is_err());
        assert!(check_root(home.path().parent().unwrap(), h, &own).is_err(), "ancestor of home");
        assert!(check_root(Path::new("relative"), h, &own).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn root_policy_sees_through_symlinks() {
        let home = TempDir::new("home-link");
        home.mkdir(".ssh");
        home.mkdir("work");
        std::os::unix::fs::symlink(home.path().join(".ssh"), home.path().join("work/innocent")).unwrap();
        assert!(check_root(&home.path().join("work/innocent"), Some(home.path()), &[]).is_err());
    }

    #[test]
    fn profiles_take_paths_as_parameters_only() {
        // A hostile folder name must never reach the profile text.
        for p in [GUI_PROFILE, WORKER_PROFILE] {
            assert!(!p.contains("/Users/"), "no user path baked into a profile");
            assert!(p.starts_with("(version 1)"));
        }
        assert!(WORKER_PROFILE.contains("(deny default)"));
        assert!(!WORKER_PROFILE.contains("network"));
        assert!(GUI_PROFILE.contains("(deny network*)"));
        let c = worker_command(Sandbox::Seatbelt, &WorkerPaths { exe: Path::new("/bin/x"), root: Path::new("/r/a\")(allow default)"), cache: Path::new("/c") });
        let args: Vec<_> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args[0], "-p");
        assert_eq!(args[1], WORKER_PROFILE);
        assert!(args.contains(&"ROOT=/r/a\")(allow default)".to_owned()));
    }
}
