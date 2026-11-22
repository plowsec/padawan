//! `margin [PATH]`: one binary, three roles.
//!
//! | Role | Process | Sandbox (macOS) | Touches |
//! |---|---|---|---|
//! | launcher (default) | parent | none: it handles no file content | starts the two others, relays frames, lists directories for the folder picker |
//! | `--role gui` | child | no network, no exec, no user files, writes only the config dir | window, input, rendering |
//! | `--role worker` | child, one per open folder | deny-default: the folder + its cache dir, nothing else | every file of the folder |
//!
//! `--no-sandbox`: everything in one process, engine on threads. For platforms without a sandbox and for debugging.

mod args;
mod browse;
mod dirs;
mod launcher;
mod local;
mod mux_client;
mod sandbox;
mod worker;

use std::io::Write as _;
use std::process::ExitCode;

use args::{Args, Role};

const HELP: &str = "\
margin [OPTIONS] [PATH]

  PATH            folder to open, or a file (its folder is opened)
  --no-sandbox    one process, no OS sandbox
  --version
  --help

Internal: --role gui|worker (started by the launcher).
";

fn main() -> ExitCode {
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
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("margin: {e}");
            ExitCode::FAILURE
        }
    }
}
