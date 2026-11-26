//! Read-only view of the app for tests and screenshot scripts. Not used by the app itself.

use margin_theme::Look;

use margin_editor::EditAction;

use crate::app::{App, Modal};
use crate::side::SideMode;
use crate::tabs::{Banner, View};

#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub folder: Option<String>,
    pub fatal: Option<String>,
    pub open_error: Option<String>,
    /// Tree rows as `(path, depth)`.
    pub tree: Vec<(String, u16)>,
    /// Tab titles of the focused group, strip order.
    pub tabs: Vec<String>,
    /// Tab titles of every group, in layout order; the active tab's title; the group's rect last frame.
    pub groups: Vec<(Vec<String>, Option<String>, egui::Rect)>,
    /// Index into `groups` of the focused one.
    pub group: usize,
    /// Group ids in layout order (widget ids).
    pub group_ids: Vec<u32>,
    /// Open buffers (a file shown in two groups counts once).
    pub buffers: usize,
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
    /// Active search tab (else the last one).
    pub search_hits: usize,
    pub search_files: usize,
    pub search_error: Option<String>,
    pub search_summary: String,
    pub search_note: Option<String>,
    pub search_tabs: usize,
    /// Tab id of that search tab (widget ids).
    pub search_tab: Option<u64>,
    pub search_query: String,
    pub search_replace_shown: bool,
    /// Sidebar: `"names"` / `"content"`, query, list rows (paths), count line.
    pub side_mode: &'static str,
    pub side_query: String,
    pub side_list: Vec<String>,
    pub side_count: String,
    pub side_list_active: bool,
    pub menu_open: Option<usize>,
    pub menu_sel: Option<usize>,
    pub ctx_menu: Vec<String>,
    pub save_as_error: Option<String>,
    pub save_as_text: Option<String>,
    pub paths_title: Option<String>,
    pub paths: usize,
    pub paths_selected: Option<String>,
    pub preview_on: bool,
    pub preview_parsed: bool,
    /// Text runs of the preview drawn last frame, and its selection as text.
    pub preview_text: Vec<egui::Rect>,
    pub preview_selection: String,
    pub edit_log: Vec<EditAction>,
    pub active_untitled: Option<u32>,
    /// Absolute path of a buffer saved outside any folder.
    pub active_external: Option<String>,
    pub menu_bar: bool,
    /// Open modal: title, buttons (label, rect).
    pub modal_title: Option<String>,
    pub modal_buttons: Vec<(String, egui::Rect)>,
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
        let title = |id: u64| self.tabs.by_id(id).map(|i| self.tabs.list[i].title()).unwrap_or_default();
        let order = self.tabs.order();
        let search = self.tabs.active_tab().and_then(|t| t.search()).or_else(|| self.tabs.list.iter().rev().find_map(|t| t.search()));
        Probe {
            folder: self.ws.as_ref().map(|w| w.display_name()),
            fatal: self.ws.as_ref().and_then(|w| w.fatal.clone()),
            open_error: self.open_error.clone(),
            tree: self.tree.rows.iter().map(|r| (r.path.to_string(), r.depth)).collect(),
            tabs: self.tabs.groups[self.tabs.cur].tabs.iter().map(|id| title(*id)).collect(),
            groups: order
                .iter()
                .filter_map(|id| self.tabs.group_index(*id))
                .map(|g| {
                    let g = &self.tabs.groups[g];
                    let rect = self.group_rects.iter().find(|(id, _)| *id == g.id).map_or(egui::Rect::NOTHING, |x| x.1);
                    (g.tabs.iter().map(|id| title(*id)).collect(), g.active.map(title), rect)
                })
                .collect(),
            group: order.iter().position(|id| *id == self.tabs.groups[self.tabs.cur].id).unwrap_or(0),
            group_ids: order.clone(),
            buffers: self.tabs.list.len(),
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
                Modal::Replace { hits, files, with, only, .. } => format!("replace {hits} in {files} with {with}{}", only.as_ref().map(|p| format!(" only {p}")).unwrap_or_default()),
                Modal::SaveAs(_) => "save-as".to_owned(),
                Modal::About => "about".to_owned(),
            }),
            results_title: self.results.as_ref().map(|r| r.header()),
            results: self.results.as_ref().map_or(0, |r| r.sites.len()),
            search_hits: search.map_or(0, |s| s.hits.len()),
            search_files: search.map_or(0, |s| s.files()),
            search_error: search.and_then(|s| s.error.clone()),
            search_summary: search.map(|s| s.summary()).unwrap_or_default(),
            search_note: search.and_then(|s| s.note.clone()),
            search_tab: search.map(|s| s.tab),
            search_query: search.map(|s| s.query.clone()).unwrap_or_default(),
            search_replace_shown: search.is_some_and(|s| s.show_replace),
            search_tabs: self.tabs.list.iter().filter(|t| t.search().is_some()).count(),
            side_mode: match self.side.mode {
                SideMode::Names => "names",
                SideMode::Content => "content",
            },
            side_query: self.side.query.clone(),
            side_list: self.side.items.iter().map(|m| m.path.to_string()).collect(),
            side_count: self.side.count_line(),
            side_list_active: self.side.list_active(),
            menu_open: self.menubar.open,
            menu_sel: self.menubar.sel,
            ctx_menu: self.ctx_menu.as_ref().map(|m| m.items.iter().filter(|(e, _)| !e.sep).map(|(e, _)| if e.enabled { e.label.clone() } else { format!("({})", e.label) }).collect()).unwrap_or_default(),
            save_as_error: match &self.modal {
                Some(Modal::SaveAs(s)) => s.error.clone(),
                _ => None,
            },
            save_as_text: match &self.modal {
                Some(Modal::SaveAs(s)) => Some(s.text.clone()),
                _ => None,
            },
            paths_title: self.paths.as_ref().map(|p| p.header()),
            paths: self.paths.as_ref().map_or(0, |p| p.paths.len()),
            paths_selected: self.paths.as_ref().and_then(|p| p.selected_loc()),
            preview_on: f.and_then(|f| f.preview.as_ref()).is_some_and(|p| p.on),
            preview_parsed: f.and_then(|f| f.preview.as_ref()).is_some_and(|p| p.doc.is_some()),
            preview_text: f.and_then(|f| f.preview.as_ref()).filter(|p| p.on).map(|p| p.state.text_rects()).unwrap_or_default(),
            preview_selection: f.and_then(|f| f.preview.as_ref()).and_then(|p| p.doc.as_ref().map(|d| p.state.selected_text(d))).unwrap_or_default(),
            edit_log: self.edit_log.clone(),
            active_untitled: f.and_then(|f| f.untitled),
            active_external: f.and_then(|f| f.external.clone()),
            menu_bar: self.settings.menu_bar,
            modal_title: self.modal.as_ref().map(|_| self.modal_seen.0.clone()),
            modal_buttons: if self.modal.is_some() { self.modal_seen.1.clone() } else { Vec::new() },
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
