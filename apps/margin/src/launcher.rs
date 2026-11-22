//! The launcher: the only process outside a sandbox, so the only one that can start the others.
//! It never reads file content and never decodes engine messages: it checks frame sizes and relays.
//!
//! ```text
//!  gui (sandboxed) ──ToLauncher──▶ launcher ──frames──▶ worker (sandboxed, one per folder)
//!                  ◀─FromLauncher─          ◀─frames──
//! ```
//!
//! | From the GUI | Launcher does |
//! |---|---|
//! | `Spawn { gen, root }` | checks the folder (`sandbox::check_root`), stops the old worker, starts a new one |
//! | `Engine { gen, payload }` | writes `payload` to the worker of `gen`; stale generations are dropped |
//! | `Browse { path }` | lists sub-directory names (folder picker) |
//! | `Quit` / end of stream / bad frame | stops the worker, exits |

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use margin_proto::frame::{read_frame, write_frame};
use margin_proto::limits::MAX_FRAME;
use margin_proto::mux::{FromLauncher, ToLauncher, MAX_MUX_FRAME};
use margin_proto::IndexOpts;

use crate::args::Args;
use crate::dirs;
use crate::sandbox::{self, GuiPaths, Sandbox, WorkerPaths};

/// How long a worker gets to save its cache after its input closes.
const GRACE: Duration = Duration::from_secs(3);

pub fn run(args: &Args) -> Result<(), String> {
    let config = dirs::config_dir();
    let cache = dirs::cache_dir();
    for d in config.iter().chain(cache.iter()) {
        dirs::ensure_private(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let (folder, file) = split_path(args.path.as_deref())?;

    if args.no_sandbox {
        eprintln!("margin: --no-sandbox: one process, no OS sandbox");
        return margin_gui::run(margin_gui::RunOpts {
            opener: Box::new(crate::local::LocalOpener::new(cache, config.clone())),
            folder,
            file,
            config_dir: config,
            gui_sandbox: "none".into(),
        });
    }

    let sb = Sandbox::detect()?;
    if sb == Sandbox::None {
        eprintln!("margin: no OS sandbox on this platform yet; children run unconfined");
    }
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).map_err(|e| format!("cannot find my own binary: {e}"))?;
    let config = config.ok_or("no home directory: cannot place the config dir")?;
    let cache = cache.ok_or("no home directory: cannot place the cache dir")?;
    let config = std::fs::canonicalize(&config).map_err(|e| format!("{}: {e}", config.display()))?;
    let cache = std::fs::canonicalize(&cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let home = dirs::home().and_then(|h| std::fs::canonicalize(h).ok()).ok_or("no home directory")?;
    let darwin_user = dirs::darwin_user_dir().unwrap_or_else(|| config.clone());

    let mut cmd = sandbox::gui_command(sb, &GuiPaths { exe: &exe, config: &config, home: &home, darwin_user: &darwin_user });
    cmd.args(["--role", "gui", "--sandbox", sb.name()]).arg("--config").arg(&config);
    if let Some(f) = &folder {
        cmd.arg("--folder").arg(f);
    }
    if let Some(f) = &file {
        cmd.arg("--file").arg(f);
    }
    let mut gui = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().map_err(|e| format!("cannot start the window process: {e}"))?;
    let mut from_gui = gui.stdout.take().ok_or("no pipe from the window process")?;
    let to_gui = Arc::new(Mutex::new(gui.stdin.take().ok_or("no pipe to the window process")?));

    let mut relay = Relay { sb, exe, cache, config, home, to_gui, worker: None };
    // Ends when the window closes (end of stream) or its pipe breaks.
    while let Ok(Some(buf)) = read_frame(&mut from_gui, MAX_MUX_FRAME) {
        match ToLauncher::decode(&buf) {
            Ok(ToLauncher::Spawn { gen, root, opts }) => relay.spawn(gen, &root, &opts),
            Ok(ToLauncher::Engine { gen, payload }) => relay.forward(gen, &payload),
            Ok(ToLauncher::Browse { path }) => {
                // Directory listing can be slow (network mounts): never on the relay thread.
                let to_gui = relay.to_gui.clone();
                let _ = std::thread::Builder::new().name("browse".into()).spawn(move || {
                    let listing = crate::browse::list(Some(Path::new(&path)).filter(|p| !p.as_os_str().is_empty()));
                    send(&to_gui, &FromLauncher::Dirs(listing));
                });
            }
            Ok(ToLauncher::Quit) => break,
            Err(e) => {
                // The GUI is the same binary: a frame we cannot read means it is broken or subverted.
                eprintln!("margin: bad frame from the window process ({e}); stopping");
                let _ = gui.kill();
                break;
            }
        }
    }
    relay.stop_worker();
    drop(relay);
    match gui.wait() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("the window process ended with {s}")),
        Err(e) => Err(format!("the window process: {e}")),
    }
}

/// `PATH` argument → (folder, file inside it). A file opens its parent folder.
fn split_path(path: Option<&Path>) -> Result<(Option<PathBuf>, Option<String>), String> {
    let Some(p) = path else { return Ok((None, None)) };
    let abs = std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if abs.is_dir() {
        return Ok((Some(abs), None));
    }
    let parent = abs.parent().ok_or("a file needs a parent folder")?.to_path_buf();
    let name = abs.file_name().and_then(|n| n.to_str()).ok_or("file name is not UTF-8")?.to_owned();
    Ok((Some(parent), Some(name)))
}

fn send(to_gui: &Mutex<ChildStdin>, msg: &FromLauncher) {
    if let Ok(mut w) = to_gui.lock() {
        let _ = write_frame(&mut *w, &msg.encode());
    }
}

struct Worker {
    gen: u32,
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
}

struct Relay {
    sb: Sandbox,
    exe: PathBuf,
    cache: PathBuf,
    config: PathBuf,
    home: PathBuf,
    to_gui: Arc<Mutex<ChildStdin>>,
    worker: Option<Worker>,
}

impl Relay {
    fn spawn(&mut self, gen: u32, root: &str, opts: &IndexOpts) {
        self.stop_worker();
        if let Err(msg) = self.try_spawn(gen, root, opts) {
            send(&self.to_gui, &FromLauncher::Ended { gen, msg });
        }
    }

    fn try_spawn(&mut self, gen: u32, root: &str, opts: &IndexOpts) -> Result<(), String> {
        let root = sandbox::check_root(Path::new(root), Some(&self.home), &[&self.cache, &self.config])?;
        let cache = dirs::root_cache_dir(&self.cache, &root);
        dirs::ensure_private(&cache).map_err(|e| format!("cache dir: {e}"))?;
        let cache = std::fs::canonicalize(&cache).map_err(|e| format!("cache dir: {e}"))?;

        let mut cmd: Command = sandbox::worker_command(self.sb, &WorkerPaths { exe: &self.exe, root: &root, cache: &cache });
        cmd.args(["--role", "worker", "--root"]).arg(&root).arg("--cache").arg(&cache).args(crate::args::opts_args(opts));
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().map_err(|e| format!("cannot start the worker: {e}"))?;
        let stdin = child.stdin.take().ok_or("no pipe to the worker")?;
        let mut stdout = child.stdout.take().ok_or("no pipe from the worker")?;
        let child = Arc::new(Mutex::new(child));

        let (to_gui, reaper) = (self.to_gui.clone(), child.clone());
        std::thread::Builder::new()
            .name(format!("relay-{gen}"))
            .spawn(move || {
                let why = loop {
                    match read_frame(&mut stdout, MAX_FRAME) {
                        Ok(Some(payload)) => send(&to_gui, &FromLauncher::Engine { gen, payload }),
                        Ok(None) => break "the worker exited".to_owned(),
                        Err(e) => break format!("worker link: {e}"),
                    }
                };
                // A worker that sends garbage is not trusted to exit on its own.
                let status = reaper.lock().ok().and_then(|mut c| {
                    let _ = c.kill();
                    c.wait().ok()
                });
                let msg = match status {
                    Some(s) if !s.success() => format!("{why} ({s})"),
                    _ => why,
                };
                send(&to_gui, &FromLauncher::Ended { gen, msg });
            })
            .map_err(|e| format!("thread: {e}"))?;
        self.worker = Some(Worker { gen, child, stdin });
        Ok(())
    }

    fn forward(&mut self, gen: u32, payload: &[u8]) {
        let Some(w) = self.worker.as_mut().filter(|w| w.gen == gen) else { return };
        if write_frame(&mut w.stdin, payload).is_err() || w.stdin.flush().is_err() {
            // Worker gone: its relay thread reports `Ended`.
            self.worker = None;
        }
    }

    /// Close the worker's input (it saves its cache and exits), then make sure it is gone.
    fn stop_worker(&mut self) {
        let Some(Worker { child, stdin, .. }) = self.worker.take() else { return };
        drop(stdin);
        let _ = std::thread::Builder::new().name("reap".into()).spawn(move || {
            let deadline = Instant::now() + GRACE;
            loop {
                let Ok(mut c) = child.lock() else { return };
                match c.try_wait() {
                    Ok(Some(_)) | Err(_) => return,
                    Ok(None) if Instant::now() >= deadline => {
                        let _ = c.kill();
                        let _ = c.wait();
                        return;
                    }
                    Ok(None) => {}
                }
                drop(c);
                std::thread::sleep(Duration::from_millis(20));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use margin_testutil::TempDir;

    #[test]
    fn path_argument_splits_into_folder_and_file() {
        let t = TempDir::new("split");
        t.write("notes/a.md", "x");
        let root = std::fs::canonicalize(t.path()).unwrap();
        assert_eq!(split_path(None).unwrap(), (None, None));
        assert_eq!(split_path(Some(&t.path().join("notes"))).unwrap(), (Some(root.join("notes")), None));
        assert_eq!(split_path(Some(&t.path().join("notes/a.md"))).unwrap(), (Some(root.join("notes")), Some("a.md".into())));
        assert!(split_path(Some(&t.path().join("missing"))).is_err());
    }
}
