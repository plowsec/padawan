//! Read-only view of the app for tests and screenshot scripts. Not used by the app itself.

use margin_theme::Look;

use crate::app::{App, Modal, SideTab};
use crate::tabs::{Banner, View};

#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub folder: Option<String>,
    pub fatal: Option<String>,
    pub open_error: Option<String>,
    /// Tree rows as `(path, depth)`.
    pub tree: Vec<(String, u16)>,
    pub tabs: Vec<String>,
    pub active: Option<String>,
    pub active_text: Option<String>,
    pub active_dirty: bool,
    pub active_read_only: bool,
    pub active_view: &'static str,
    pub cursor: Option<(u32, u32)>,
    pub banner: Option<&'static str>,
    pub bad_utf8: bool,
    pub lenses: Vec<(u32, Vec<String>)>,
    pub palette_open: bool,
    pub palette_query: String,
    pub palette_files: usize,
    pub palette_selected: Option<String>,
    pub picker_open: bool,
    pub picker_error: Option<String>,
    pub modal: Option<String>,
    pub results_title: Option<String>,
    pub results: usize,
    pub search_hits: usize,
    pub search_files: usize,
    pub search_error: Option<String>,
    pub side_search: bool,
    pub sidebar_visible: bool,
    pub toasts: Vec<String>,
    pub look: Look,
    pub zoom: f32,
    pub back: usize,
    pub recent: Vec<String>,
    pub editor_id: Option<egui::Id>,
}

impl App {
    #[doc(hidden)]
    pub fn probe(&self) -> Probe {
        let f = self.tabs.active_file();
        let doc = f.and_then(|f| f.doc());
        Probe {
            folder: self.ws.as_ref().map(|w| w.display_name()),
            fatal: self.ws.as_ref().and_then(|w| w.fatal.clone()),
            open_error: self.open_error.clone(),
            tree: self.tree.rows.iter().map(|r| (r.path.to_string(), r.depth)).collect(),
            tabs: self.tabs.list.iter().map(|t| t.title()).collect(),
            active: self.tabs.active_tab().map(|t| t.title()),
            active_text: doc.map(|d| d.text()),
            active_dirty: doc.is_some_and(|d| d.is_dirty()),
            active_read_only: doc.is_some_and(|d| d.read_only()),
            active_view: match f.map(|f| &f.view) {
                None => "none",
                Some(View::Loading) => "loading",
                Some(View::Text(_)) => "text",
                Some(View::Binary(_)) => "binary",
                Some(View::Message(_)) => "message",
            },
            cursor: f.map(|f| f.st.cursor()),
            banner: f.and_then(|f| f.banner).map(|b| match b {
                Banner::Conflict => "conflict",
                Banner::ChangedDirty => "changed",
            }),
            bad_utf8: f.is_some_and(|f| f.bad_utf8),
            lenses: f.map(|f| f.hints.lenses.iter().map(|l| (l.line, l.segs.iter().map(|s| s.text.clone()).collect())).collect()).unwrap_or_default(),
            palette_open: self.palette.is_some(),
            palette_query: self.palette.as_ref().map(|p| p.query.clone()).unwrap_or_default(),
            palette_files: self.palette.as_ref().map_or(0, |p| p.files.len()),
            palette_selected: self.palette.as_ref().and_then(|p| p.selected_desc()),
            picker_open: self.picker.is_some(),
            picker_error: self.picker.as_ref().and_then(|p| p.error.clone()),
            modal: self.modal.as_ref().map(|m| match m {
                Modal::Unsaved { .. } => "unsaved".to_owned(),
                Modal::Delete { path, .. } => format!("delete {path}"),
            }),
            results_title: self.results.as_ref().map(|r| r.header()),
            results: self.results.as_ref().map_or(0, |r| r.sites.len()),
            search_hits: self.search.hits.len(),
            search_files: self.search.files(),
            search_error: self.search.error.clone(),
            side_search: self.side == SideTab::Search,
            sidebar_visible: self.state.sidebar_visible,
            toasts: self.toasts.list.iter().map(|t| t.text.clone()).collect(),
            look: self.theme.look,
            zoom: self.settings.zoom,
            back: self.back.len(),
            recent: self.state.recent.clone(),
            editor_id: self.editor_id,
        }
    }
}
