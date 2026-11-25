//! The window: menu bar, sidebar (search box + files), editor groups (split right / down; each with its tabs: files,
//! untitled, search, settings) showing editor | Markdown preview, bottom panel (results, call paths), palette,
//! context menus, status bar.
//!
//! PUBLIC API IS FROZEN: `RunOpts`, `run`. Added: `App` (`App::new`, `App::ui`, `App::probe` for tests).
//!
//! | Module | What |
//! |---|---|
//! | `app` / `ops` / `features` | state + frame / commands, replies, file ops, overlays / untitled + hot exit, Save As, replace, context menus, preview wiring |
//! | `workspace` | engine client, requests in flight, index status |
//! | `side`, `tree`, `tabs`, `search_panel`, `results_panel`, `paths_view`, `md_preview`, `menubar`, `palette`, `picker` | the parts |
//! | `commands` | command registry + keymap |
//! | `settings` / `settings_pane` / `state` | model + file format / Settings tab / `state.conf` + writer thread |
//! | `hints` | outline thread + hint lines |
//! | `status`, `welcome`, `toast` | chrome |
//! | `sanitize` | every engine string shown outside the editor goes through it |
//! | `widgets`, `fuzzy` | shared painters, small scorer |
//!
//! Security: the GUI opens no URL, starts no process, touches no file outside `config_dir`,
//! and sends the engine only `RelPath`s it built itself.

use std::path::PathBuf;

use margin_proto::Opener;

mod app;
pub mod commands;
mod features;
pub mod fuzzy;
pub mod hints;
pub mod md_preview;
pub mod menubar;
mod ops;
pub mod palette;
pub mod paths_view;
pub mod picker;
mod probe;
pub mod results_panel;
pub mod sanitize;
pub mod search_panel;
pub mod settings;
pub mod side;
mod settings_pane;
pub mod state;
pub mod status;
pub mod tabs;
pub mod toast;
pub mod tree;
mod welcome;
pub mod widgets;
pub mod workspace;

pub use app::{group_divider_id, preview_divider_id, preview_toggle_id, split_button_id, App};
pub use features::ctx_menu_id;
pub use ops::save_as_id;
pub use probe::Probe;

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
    /// A secondary window ("New Window"): do not restore untitled buffers or the last session.
    pub fresh: bool,
}

struct Shell(App);

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }

    fn on_exit(&mut self) {
        self.0.on_exit();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.0.theme().ui.panel_bg.to_normalized_gamma_f32()
    }
}

/// Open the window and run until it closes.
///
/// `MARGIN_SMOKE` set: after the start folder (and file) loaded and the index is ready, print
/// `smoke: ok …` to stderr and close; `smoke: fatal <msg>` / `smoke: timeout` (20 s) → `Err`.
pub fn run(opts: RunOpts) -> Result<(), String> {
    let smoke = std::env::var_os("MARGIN_SMOKE").is_some();
    let verdict = std::sync::Arc::new(std::sync::Mutex::new(None::<Result<String, String>>));
    let v2 = verdict.clone();
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Margin").with_app_id("margin").with_inner_size([1280.0, 820.0]).with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    let mut opts = Some(opts);
    eframe::run_native(
        "Margin",
        native,
        Box::new(move |cc| {
            margin_theme::fonts::install(&cc.egui_ctx);
            let opts = opts.take().ok_or("started twice")?;
            let mut app = App::new(&cc.egui_ctx, opts);
            if smoke {
                app.enable_smoke(v2);
            }
            Ok(Box::new(Shell(app)))
        }),
    )
    .map_err(|e| e.to_string())?;
    if smoke {
        let v = verdict.lock().map(|g| g.clone()).unwrap_or(None);
        return match v {
            Some(Ok(line)) => {
                eprintln!("{line}");
                Ok(())
            }
            Some(Err(line)) => {
                eprintln!("{line}");
                Err(line)
            }
            None => {
                eprintln!("smoke: fatal window closed before the check");
                Err("smoke: window closed before the check".into())
            }
        };
    }
    Ok(())
}
