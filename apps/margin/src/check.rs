//! `margin --check-sandbox`: start a probe under each sandbox profile and print what it could do.
//! Exit status is a failure if any escape worked. Uses a scratch directory inside Margin's cache dir.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::dirs;
use crate::sandbox::{self, GuiPaths, Sandbox, WorkerPaths};

pub fn run() -> Result<(), String> {
    let sb = Sandbox::detect()?;
    if sb == Sandbox::None {
        return Err("no OS sandbox on this platform yet".into());
    }
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).map_err(|e| format!("cannot find my own binary: {e}"))?;
    let home = dirs::home().and_then(|h| std::fs::canonicalize(h).ok()).ok_or("no home directory")?;
    let cache = dirs::cache_dir().ok_or("no home directory")?;
    dirs::ensure_private(&cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let cache = std::fs::canonicalize(&cache).map_err(|e| format!("{}: {e}", cache.display()))?;

    let scratch = Scratch::create(&cache)?;
    // A listener of our own: the probe must not be able to reach even this.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("listener: {e}"))?;
    let port = listener.local_addr().map_err(|e| format!("listener: {e}"))?.port();

    let mut out = std::io::stdout().lock();
    let mut failed = 0usize;
    let worker = sandbox::worker_command(sb, &WorkerPaths { exe: &exe, root: &scratch.allowed, cache: &scratch.allowed });
    let gui = sandbox::gui_command(sb, &GuiPaths { exe: &exe, config: &scratch.allowed, home: &home, darwin_user: &scratch.allowed });
    for (name, mut cmd) in [("worker", worker), ("gui", gui)] {
        let _ = writeln!(out, "{name} sandbox ({})", sb.name());
        cmd.args(["--role", "probe", "--sandbox", name, "--root"]).arg(&scratch.allowed).arg("--outside").arg(&scratch.outside).args(["--port", &port.to_string()]);
        match probe(cmd) {
            Ok(lines) if !lines.is_empty() => {
                for (check, pass, detail) in lines {
                    failed += usize::from(!pass);
                    let _ = writeln!(out, "  {}  {check:<36} {detail}", if pass { "ok  " } else { "FAIL" });
                }
            }
            Ok(_) => {
                failed += 1;
                let _ = writeln!(out, "  FAIL  the probe reported nothing");
            }
            Err(e) => {
                failed += 1;
                let _ = writeln!(out, "  FAIL  {e}");
            }
        }
    }
    drop(listener);
    scratch.remove();
    if failed == 0 {
        let _ = writeln!(out, "all checks passed");
        Ok(())
    } else {
        Err(format!("{failed} sandbox check(s) failed"))
    }
}

fn probe(mut cmd: Command) -> Result<Vec<(String, bool, String)>, String> {
    let outp = cmd.stdin(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| format!("cannot start the probe: {e}"))?;
    let text = String::from_utf8_lossy(&outp.stdout);
    let lines: Vec<_> = text
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            match (f.next(), f.next(), f.next(), f.next()) {
                (Some("check"), Some(name), Some(verdict), Some(detail)) => Some((name.to_owned(), verdict == "pass", detail.to_owned())),
                _ => None,
            }
        })
        .collect();
    if lines.is_empty() && !outp.status.success() {
        return Err(format!("the probe ended with {}: {}", outp.status, String::from_utf8_lossy(&outp.stderr).trim()));
    }
    Ok(lines)
}

/// `<cache>/selftest-<pid>/{allowed,outside/canary.txt}`. Removed entry by entry: only what was created here.
struct Scratch {
    base: PathBuf,
    allowed: PathBuf,
    outside: PathBuf,
}

impl Scratch {
    fn create(cache: &Path) -> Result<Self, String> {
        if !cache.is_absolute() {
            return Err("cache dir is not absolute".into());
        }
        let base = cache.join(format!("selftest-{}", std::process::id()));
        let s = Self { allowed: base.join("allowed"), outside: base.join("outside"), base };
        let io = |e: std::io::Error| format!("scratch dir: {e}");
        std::fs::create_dir(&s.base).map_err(io)?;
        std::fs::create_dir(&s.allowed).map_err(io)?;
        std::fs::create_dir(&s.outside).map_err(io)?;
        std::fs::write(s.outside.join("canary.txt"), b"canary").map_err(io)?;
        Ok(s)
    }

    fn remove(&self) {
        for f in [self.outside.join("canary.txt"), self.outside.join("escape.txt"), self.allowed.join("probe.txt")] {
            let _ = std::fs::remove_file(f);
        }
        for d in [&self.outside, &self.allowed, &self.base] {
            let _ = std::fs::remove_dir(d);
        }
    }
}
