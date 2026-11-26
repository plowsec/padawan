//! `margin [PATH]`: one binary, three roles.
//!
//! | Role | Process | Sandbox (macOS) | Touches |
//! |---|---|---|---|
//! | launcher (default) | parent | none: it parses no file content | starts the two others, relays frames, lists directories for the folder picker, writes a buffer saved without a folder (`save.rs`) |
//! | `--role gui` | child | no network, no exec, no user files, writes only the config dir | window, input, rendering |
//! | `--role worker` | child, one per open folder | deny-default: the folder + its cache dir, nothing else | every file of the folder |
//!
//! `--no-sandbox`: everything in one process, engine on threads. For platforms without a sandbox and for debugging.

pub mod args;
pub mod browse;
pub mod check;
pub mod desktop;
pub mod dirs;
pub mod launcher;
pub mod local;
pub mod mux_client;
pub mod probe;
pub mod sandbox;
pub mod save;
pub mod window;
pub mod worker;

use std::io::Write as _;
use std::process::ExitCode;

use args::{Args, Role};

const HELP: &str = "\
margin [OPTIONS] [PATH]

  PATH                 folder to open, or a file (its folder is opened)
  --no-sandbox         one process, no OS sandbox
  --check-sandbox      try to break out of both sandboxes, print what was refused
  --install-desktop    Linux: menu entry and icon for this user, pointing at this binary
  --uninstall-desktop  remove them
  --version
  --help

Internal: --role gui|worker|probe (started by the launcher).
";

/// Entry point of the `margin` binary.
pub fn main_entry() -> ExitCode {
    let args = match Args::parse(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("margin: {e}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    let result = match args.role {
        Role::Help => {
            let _ = std::io::stdout().write_all(HELP.as_bytes());
            Ok(())
        }
        Role::Version => {
            let _ = writeln!(std::io::stdout(), "margin {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Role::Launcher => launcher::run(&args),
        Role::Gui => mux_client::run_gui(&args),
        Role::Worker => worker::run(&args),
        Role::Check => check::run(),
        Role::InstallDesktop => desktop::run(false),
        Role::UninstallDesktop => desktop::run(true),
        Role::Probe => probe::run(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("margin: {e}");
            ExitCode::FAILURE
        }
    }
}
