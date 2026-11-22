//! "New Window": a second launcher process, with its own window process and workers.
//! Asked for by the GUI (which cannot start programs); done by the launcher, or by the single
//! process under `--no-sandbox`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A subverted window process must not be able to flood the screen.
const MIN_GAP_MS: u64 = 400;
static LAST: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Start another Margin. `folder`: opened there (the new launcher applies the folder policy itself).
pub fn spawn(folder: Option<&Path>, no_sandbox: bool) -> Result<(), String> {
    let now = now_ms();
    let last = LAST.swap(now, Ordering::Relaxed);
    if now.saturating_sub(last) < MIN_GAP_MS {
        return Err("too many new windows at once".into());
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot find my own binary: {e}"))?;
    let mut cmd = Command::new(exe);
    if no_sandbox {
        cmd.arg("--no-sandbox");
    }
    if let Some(f) = folder.filter(|f| f.is_absolute()) {
        // After `--`: a folder name can never be read as a flag.
        cmd.arg("--").arg(f);
    }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().map_err(|e| format!("cannot start a new window: {e}"))?;
    // Reap it when it ends.
    std::thread::Builder::new()
        .name("window-wait".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}
