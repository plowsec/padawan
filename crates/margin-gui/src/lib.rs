//! The window: sidebar (files, search), tabs, editor, palette, settings, status bar.
//!
//! STUB. PUBLIC API IS FROZEN: `RunOpts`, `run`.

use std::path::PathBuf;

use margin_proto::Opener;

pub struct RunOpts {
    /// Starts engines for folders (in-process, or through the launcher into a sandboxed worker).
    pub opener: Box<dyn Opener>,
    /// Folder to open at start; `None`: welcome screen.
    pub folder: Option<PathBuf>,
    /// File inside `folder` to open at start (relative path).
    pub file: Option<String>,
    /// `settings.conf`, `state.conf` live here. `None`: nothing is persisted.
    pub config_dir: Option<PathBuf>,
    /// What confines this GUI process, for the status bar: `"seatbelt"`, `"none"`.
    pub gui_sandbox: String,
}

/// Open the window and run until it closes.
pub fn run(opts: RunOpts) -> Result<(), String> {
    let _ = opts;
    Err("not built yet".into())
}
