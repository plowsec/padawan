//! App state and the frame. `App::ui` draws everything and is callable headlessly (`margin_shot`).
//!
//! Frame order:
//! 1. theme, zoom; drain engine / picker / background channels
//! 2. window focus, dropped files, close requests
//! 3. shortcuts (consumed before anything else sees them)
//! 4. overlays (modal > picker > palette); if one is open, remaining keyboard input is dropped
//! 5. layout: status bar, sidebar, tab strip, editor, results panel; toasts
//! 6. timers: search / palette debounce, hints, settings + state saves (`request_repaint_after`)

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use egui::{pos2, vec2, CursorIcon, Id, Key, Rect, Sense, Ui};
use margin_editor::{Document, EditorOptions};
use margin_proto::{Meta, Opener, RelPath, Waker};
use margin_theme::Theme;

use crate::commands::{self, Cmd};
use crate::hints::OutlineDone;
use crate::palette::Palette;
use crate::picker::Picker;
use crate::results_panel::{ResultsAction, ResultsPanel};
use crate::sanitize::{clean, sanitize};
use crate::search_panel::SearchPanel;
use crate::settings::Settings;
use crate::state::{Persist, SavedState, SIDEBAR_W};
use crate::status::{self, StatusInfo};
use crate::tabs::{Banner, StripAction, TabKind, Tabs, View};
use crate::toast::Toasts;
use crate::tree::Tree;
use crate::welcome::{self, WelcomeOut};
use crate::workspace::Workspace;
use crate::{widgets, RunOpts};

pub const BIG_DOC: usize = 256 << 10;
pub const SETTINGS_DELAY: f64 = 0.5;
pub const REINDEX_EVERY: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideTab {
    Files,
    Search,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    CloseTab(u64),
    CloseFolder,
    Quit,
    Open(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Modal {
    Unsaved { tabs: Vec<u64>, then: Then },
    Delete { path: RelPath, dir: bool },
}

pub struct AfterSave {
    pub waiting: Vec<u64>,
    pub then: Then,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Loc {
    pub path: RelPath,
    pub line: u32,
    pub col: u32,
}

/// Document built off the UI thread.
pub struct DocDone {
    pub tab: u64,
    pub doc: Document,
    pub bad_utf8: bool,
    pub meta: Meta,
    pub reload: bool,
}

/// `MARGIN_SMOKE`: end-to-end check of the real pipeline (see `run`).
pub struct Smoke {
    pub start: std::time::Instant,
    pub frames_after: u32,
    pub verdict: Arc<Mutex<Option<Result<String, String>>>>,
    pub done: bool,
}

pub struct App {
    pub(crate) opener: Box<dyn Opener>,
    pub(crate) gui_sandbox: String,
    pub(crate) persist: Persist,
    pub settings: Settings,
    pub state: SavedState,
    pub theme: Theme,
    pub(crate) mac: bool,
    pub(crate) ctx: egui::Context,
    pub(crate) ws: Option<Workspace>,
    pub(crate) tabs: Tabs,
    pub(crate) tree: Tree,
    pub(crate) search: SearchPanel,
    pub(crate) results: Option<ResultsPanel>,
    pub(crate) results_h: f32,
    pub(crate) palette: Option<Palette>,
    pub(crate) picker: Option<Picker>,
    pub(crate) modal: Option<Modal>,
    pub(crate) after_save: Option<AfterSave>,
    pub(crate) toasts: Toasts,
    pub(crate) back: Vec<Loc>,
    pub(crate) forward: Vec<Loc>,
    pub(crate) side: SideTab,
    pub(crate) mru: Vec<Cmd>,
    pub(crate) settings_at: Option<f64>,
    pub(crate) state_at: Option<f64>,
    pub(crate) last_reindex: f64,
    pub(crate) was_focused: bool,
    pub(crate) doc_tx: Sender<DocDone>,
    pub(crate) doc_rx: Receiver<DocDone>,
    pub(crate) outline_tx: Sender<OutlineDone>,
    pub(crate) outline_rx: Receiver<OutlineDone>,
    /// Why the last open failed (welcome screen / picker).
    pub(crate) open_error: Option<String>,
    /// File to open once the folder says hello.
    pub(crate) start_file: Option<String>,
    /// Tabs of this folder were restored (state may be written from now on).
    pub(crate) restored: bool,
    pub(crate) tabs_changed: bool,
    pub(crate) quit_ok: bool,
    pub(crate) word_at_cursor: String,
    pub(crate) last_active: Option<u64>,
    pub(crate) now: f64,
    pub(crate) frame: u64,
    pub(crate) smoke: Option<Smoke>,
    pub(crate) focus_editor: bool,
    /// Id the editor widget used last frame (tests locate hint segments with it).
    pub(crate) editor_id: Option<Id>,
}

pub(crate) fn waker(ctx: &egui::Context) -> Waker {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

impl App {
    /// Reads `settings.conf` / `state.conf` (start-up, before the first frame) and opens `opts.folder`.
    pub fn new(ctx: &egui::Context, opts: RunOpts) -> App {
        let persist = Persist::new(opts.config_dir.as_deref());
        let settings = Settings::parse(&persist.load("settings.conf"));
        let state = SavedState::parse(&persist.load("state.conf"));
        let theme = Theme::new(settings.mode, settings.look);
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let (doc_tx, doc_rx) = channel();
        let (outline_tx, outline_rx) = channel();
        let mut app = App {
            opener: opts.opener,
            gui_sandbox: opts.gui_sandbox,
            persist,
            settings,
            state,
            theme,
            mac: ctx.os() == egui::os::OperatingSystem::Mac,
            ctx: ctx.clone(),
            ws: None,
            tabs: Tabs::default(),
            tree: Tree::default(),
            search: SearchPanel::default(),
            results: None,
            results_h: 220.0,
            palette: None,
            picker: None,
            modal: None,
            after_save: None,
            toasts: Toasts::default(),
            back: Vec::new(),
            forward: Vec::new(),
            side: SideTab::Files,
            mru: Vec::new(),
            settings_at: None,
            state_at: None,
            last_reindex: f64::NEG_INFINITY,
            was_focused: true,
            doc_tx,
            doc_rx,
            outline_tx,
            outline_rx,
            open_error: None,
            start_file: opts.file,
            restored: false,
            tabs_changed: false,
            quit_ok: false,
            word_at_cursor: String::new(),
            last_active: None,
            now: 0.0,
            frame: 0,
            smoke: None,
            focus_editor: false,
            editor_id: None,
        };
        if let Some(f) = opts.folder {
            app.open_folder_now(f);
        }
        app
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Quit: remember tabs, write settings and state now (bounded wait).
    pub fn on_exit(&mut self) {
        self.snapshot_tabs();
        self.persist.save_settings(self.settings.write());
        self.persist.save_state(self.state.write());
        self.persist.flush(std::time::Duration::from_secs(1));
    }

    pub(crate) fn enable_smoke(&mut self, verdict: Arc<Mutex<Option<Result<String, String>>>>) {
        self.smoke = Some(Smoke { start: std::time::Instant::now(), frames_after: 0, verdict, done: false });
    }

    /// Wait for queued config writes (tests).
    pub fn flush_config(&self) {
        self.persist.flush(std::time::Duration::from_secs(5));
    }

    pub(crate) fn any_overlay(&self) -> bool {
        self.modal.is_some() || self.picker.is_some() || self.palette.is_some()
    }

    /// One frame.
    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.now = ctx.input(|i| i.time);
        self.frame += 1;
        widgets::apply_theme(&ctx, &self.theme);
        if (ctx.zoom_factor() - self.settings.zoom).abs() > 1e-3 {
            ctx.set_zoom_factor(self.settings.zoom);
        }
        let t = self.theme;

        self.drain();
        self.window_events(&ctx);

        // Shortcuts.
        let only: Option<&[Cmd]> = if self.modal.is_some() {
            Some(&[])
        } else if self.any_overlay() {
            Some(&[Cmd::Palette, Cmd::GoToFile, Cmd::GoToLine, Cmd::GoToSymbol, Cmd::OpenFolder, Cmd::Quit])
        } else {
            None
        };
        for c in commands::take(&ctx, self.mac, only) {
            self.note_mru(c);
            self.run_cmd(c);
        }

        // Overlays first: they take their keys; everything below gets none.
        self.overlays(&ctx);
        if self.any_overlay() {
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. } | egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Copy | egui::Event::Cut | egui::Event::Ime(_))));
        }

        self.layout(ui, &t);
        let sb = status::height(&t);
        let full = ui.max_rect();
        self.toasts.show(&ctx, &t, full.right(), full.bottom() - sb, self.now);

        self.timers(&ctx);
        self.smoke_check(&ctx);
    }

    fn layout(&mut self, ui: &mut Ui, t: &Theme) {
        let full = ui.max_rect();
        let sb_h = status::height(t);
        let status_r = Rect::from_min_max(pos2(full.left(), full.bottom() - sb_h), full.max);
        let body = Rect::from_min_max(full.min, pos2(full.right(), status_r.top()));
        ui.painter().rect_filled(body, 0.0, t.ui.panel_bg);

        let show_side = self.ws.is_some() && self.state.sidebar_visible;
        let side_w = self.state.sidebar_width.clamp(*SIDEBAR_W.start(), (*SIDEBAR_W.end()).min(body.width() * 0.6).max(*SIDEBAR_W.start()));
        let main = if show_side {
            let sr = Rect::from_min_max(body.min, pos2(body.left() + side_w, body.bottom()));
            self.sidebar(ui, sr, t);
            ui.painter().vline(sr.right() - 0.5, sr.y_range(), t.border());
            // Resize handle on the hairline.
            let hr = Rect::from_min_max(pos2(sr.right() - 3.0, sr.top()), pos2(sr.right() + 3.0, sr.bottom()));
            let h = ui.interact(hr, Id::unique("sidebar-resize"), Sense::drag()).on_hover_cursor(CursorIcon::ResizeHorizontal);
            if h.dragged() {
                self.state.sidebar_width = (side_w + h.drag_delta().x).clamp(*SIDEBAR_W.start(), *SIDEBAR_W.end());
                self.state_at = Some(self.now);
            }
            Rect::from_min_max(pos2(sr.right(), body.top()), body.max)
        } else {
            body
        };
        self.main_area(ui, main, t);
        let info = self.status_info();
        status::show(ui, status_r, t, &info);
    }

    fn sidebar(&mut self, ui: &mut Ui, rect: Rect, t: &Theme) {
        let head_h = (t.row_h() + 12.0).round();
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), head_h));
        let name = self.ws.as_ref().map(Workspace::display_name).unwrap_or_default();
        let seg_w = 112.0;
        let seg = Rect::from_min_size(pos2(head.right() - 12.0 - seg_w, head.center().y - (t.row_h() - 2.0) / 2.0), vec2(seg_w, t.row_h() - 2.0));
        widgets::text_at(ui, head.with_max_x(seg.left() - 8.0), head.left() + 16.0, &name, t.font_ui(), t.ui.text_strong);
        let cur = usize::from(self.side == SideTab::Search);
        if let Some(i) = widgets::segmented(ui, seg, &["Files", "Search"], cur, t, Id::unique("side-switch")) {
            self.side = if i == 1 { SideTab::Search } else { SideTab::Files };
            if self.side == SideTab::Search {
                self.search.focus_query = true;
            }
        }
        let content = Rect::from_min_max(pos2(rect.left(), head.bottom()), pos2(rect.right() - 1.0, rect.bottom()));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content));
        child.set_clip_rect(content);
        match self.side {
            SideTab::Files => {
                let active = self.tabs.active_file().map(|f| f.path.clone());
                let acts = self.tree.show(&mut child, content, t, active.as_ref());
                for a in acts {
                    self.tree_action(a);
                }
            }
            SideTab::Search => {
                let (send, open) = self.search.show(&mut child, content, t, self.now);
                if let Some(q) = send {
                    self.start_search(q);
                }
                if let Some(h) = open {
                    self.open_file(h.path, Some((h.line, h.col, h.len)), true);
                    self.focus_editor = false;
                }
            }
        }
    }

    fn main_area(&mut self, ui: &mut Ui, rect: Rect, t: &Theme) {
        if self.ws.is_none() {
            let recent = self.state.recent.clone();
            match welcome::show(ui, rect, t, self.mac, &recent, self.open_error.as_deref()) {
                Some(WelcomeOut::OpenFolder) => self.run_cmd(Cmd::OpenFolder),
                Some(WelcomeOut::Recent(p)) => self.open_folder(PathBuf::from(p)),
                None => {}
            }
            return;
        }
        let strip_h = (t.row_h() + 8.0).round();
        let mut y = rect.top();
        if !self.tabs.list.is_empty() {
            let sr = Rect::from_min_size(rect.min, vec2(rect.width(), strip_h));
            match self.tabs.strip(ui, sr, t) {
                Some(StripAction::Activate(i)) => {
                    self.tabs.activate(i);
                    self.focus_editor = true;
                    self.tabs_changed = true;
                }
                Some(StripAction::Close(i)) => {
                    if let Some(id) = self.tabs.list.get(i).map(|x| x.id) {
                        self.close_tab(id);
                    }
                }
                None => {}
            }
            y = sr.bottom();
        }
        let mut area = Rect::from_min_max(pos2(rect.left(), y), rect.max);
        // Fatal banner.
        if let Some(msg) = self.ws.as_ref().and_then(|w| w.fatal.clone()) {
            let br = Rect::from_min_size(area.min, vec2(area.width(), (t.row_h() + 10.0).round()));
            if self.banner(ui, br, t, &format!("The folder's worker stopped: {}", sanitize(&msg, 200)), &["Reopen"], true) == Some(0) {
                let p = self.ws.as_ref().map(|w| w.path.clone());
                if let Some(p) = p {
                    self.reopen(p);
                }
            }
            area.min.y = br.bottom();
        }
        // Results panel.
        let mut editor_r = area;
        if self.results.is_some() {
            let h = self.results_h.clamp(80.0, (area.height() - 120.0).max(80.0));
            let rr = Rect::from_min_max(pos2(area.left(), area.bottom() - h), area.max);
            editor_r.max.y = rr.top();
            let act = self.results.as_mut().and_then(|r| r.show(ui, rr, t));
            if let Some(r) = &self.results {
                self.results_h = r.height;
            }
            match act {
                Some(ResultsAction::Jump(s)) => {
                    self.open_file(s.path, Some((s.line, s.col, s.len)), true);
                    self.focus_editor = false;
                }
                Some(ResultsAction::Close) => {
                    self.results = None;
                    self.focus_editor = true;
                }
                None => {}
            }
        }
        self.editor_area(ui, editor_r, t);
    }

    /// Thin banner with text buttons; returns the clicked button.
    pub(crate) fn banner(&self, ui: &mut Ui, r: Rect, t: &Theme, text: &str, buttons: &[&str], error: bool) -> Option<usize> {
        let p = ui.painter();
        let tint = if error { t.ui.error } else { t.ui.warning };
        p.rect_filled(r, 0.0, margin_theme::mix(t.code.bg, tint, 0.09));
        p.hline(r.x_range(), r.bottom() - 0.5, t.border());
        p.rect_filled(Rect::from_min_size(r.min, vec2(2.0, r.height())), 0.0, tint);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(12.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        let mut out = None;
        for (i, b) in buttons.iter().enumerate().rev() {
            if widgets::text_button(&mut child, b, t).clicked() {
                out = Some(i);
            }
            child.add_space(6.0);
        }
        let tr = Rect::from_min_max(pos2(r.left() + 16.0, r.top()), pos2(child.max_rect().left() + child.available_width(), r.bottom()));
        widgets::text_at(ui, tr, tr.left(), text, t.font_small(), t.ui.text_strong);
        out
    }

    fn editor_area(&mut self, ui: &mut Ui, rect: Rect, t: &Theme) {
        let mac = self.mac;
        let Some(ai) = self.tabs.active else {
            welcome::empty_hints(ui, rect, t, mac);
            return;
        };
        if matches!(self.tabs.list[ai].kind, TabKind::Settings) {
            let before = self.settings.clone();
            let out = crate::settings_pane::show(ui, rect, &mut self.settings, t, mac);
            if out.changed && self.settings != before {
                self.settings_changed(out.reindex);
            }
            return;
        }
        let tab_id = self.tabs.list[ai].id;
        let code_hints = self.settings.code_hints;
        let mut opts = EditorOptions {
            tab_width: self.settings.tab_width,
            soft_tabs: self.settings.soft_tabs,
            wrap: self.settings.wrap,
            line_numbers: self.settings.line_numbers,
            highlight_current_line: self.settings.highlight_line,
            show_whitespace: self.settings.show_whitespace,
            show_lenses: code_hints,
            request_focus: false,
        };
        let focus = std::mem::take(&mut self.focus_editor);
        let mut area = rect;
        // Banners.
        let (banner, bad) = match self.tabs.list[ai].file() {
            Some(f) => (f.banner, f.bad_utf8),
            None => (None, false),
        };
        let bh = (t.row_h() + 8.0).round();
        if let Some(b) = banner {
            let br = Rect::from_min_size(area.min, vec2(area.width(), bh));
            let buttons: &[&str] = match b {
                Banner::Conflict => &["Overwrite", "Reload"],
                Banner::ChangedDirty => &["Reload", "Keep mine"],
            };
            let msg = match b {
                Banner::Conflict => "Changed on disk since it was opened · not saved",
                Banner::ChangedDirty => "Changed on disk · you have unsaved edits",
            };
            match (b, self.banner(ui, br, t, msg, buttons, false)) {
                (Banner::Conflict, Some(0)) => self.save_tab(tab_id, true),
                (Banner::Conflict, Some(1)) | (Banner::ChangedDirty, Some(0)) => self.reload_tab(tab_id),
                (Banner::ChangedDirty, Some(1)) => {
                    if let Some(f) = self.tabs.file_by_id(tab_id) {
                        f.banner = None;
                    }
                }
                _ => {}
            }
            area.min.y = br.bottom();
        }
        if bad {
            let br = Rect::from_min_size(area.min, vec2(area.width(), (t.row_h() - 2.0).round()));
            ui.painter().rect_filled(br, 0.0, t.ui.panel_bg);
            ui.painter().hline(br.x_range(), br.bottom() - 0.5, t.border());
            widgets::text_at(ui, br, br.left() + 16.0, "Not valid UTF-8 — read-only", t.font_small(), t.ui.text_muted);
            area.min.y = br.bottom();
        }
        let Some(f) = self.tabs.list[ai].file_mut() else { return };
        if matches!(f.view, View::Text(_)) {
            opts.request_focus = focus || std::mem::take(&mut f.focus_editor);
        } else if matches!(f.view, View::Loading) {
            // Keep the request until the text arrives.
            f.focus_editor |= focus;
        }
        let mut result = None;
        match &mut f.view {
            View::Loading => {
                ui.painter().rect_filled(area, 0.0, t.code.bg);
            }
            View::Binary(size) => {
                ui.painter().rect_filled(area, 0.0, t.code.bg);
                let s = format!("Binary file · {}", human_size(*size));
                ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, s, t.font_ui(), t.ui.text_muted);
            }
            View::Message(m) => {
                ui.painter().rect_filled(area, 0.0, t.code.bg);
                ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, m.as_str(), t.font_ui(), t.ui.text_muted);
            }
            View::Text(doc) => {
                let lenses: &[margin_editor::Lens] = if code_hints { &f.hints.lenses } else { &[] };
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).id_salt(("editor", tab_id)));
                child.set_clip_rect(area);
                self.editor_id = Some(child.make_persistent_id("margin-editor"));
                result = Some(margin_editor::show(&mut child, doc, &mut f.st, &opts, t, lenses));
            }
        }
        if let Some(out) = result {
            self.word_at_cursor = out.word_at_cursor;
            if let Some((word, _)) = out.goto_definition {
                self.goto_definition(word);
            }
            if let Some((line, seg)) = out.lens_clicked {
                self.lens_clicked(tab_id, line, seg);
            }
        }
        if self.results.is_some() && ui.input(|i| i.key_pressed(Key::Escape)) && !self.tabs.active_file().is_some_and(|f| f.st.find_open()) {
            self.results = None;
        }
    }

    fn status_info(&self) -> StatusInfo {
        let mut info = StatusInfo { gui_sandbox: self.gui_sandbox.clone(), engine_sandbox: self.opener.sandbox().to_owned(), ..Default::default() };
        info.index = self.ws.as_ref().and_then(|w| w.index);
        if let Some(f) = self.tabs.active_file() {
            info.path = clean(f.path.as_str());
            if let Some(doc) = f.doc() {
                let (l, c) = f.st.cursor();
                let line = doc.line(l);
                let chars = line[..crate::sanitize::floor_char(line, c as usize)].chars().count() as u32;
                info.pos = Some((l + 1, chars + 1));
                info.lang = Some(doc.lang().name());
                if f.bad_utf8 {
                    info.notes.push("not UTF-8");
                }
                if doc.read_only() {
                    info.notes.push("read-only");
                }
            }
        }
        info
    }

    fn timers(&mut self, ctx: &egui::Context) {
        let now = self.now;
        let mut wait: Option<f64> = None;
        let mut want = |w: f64| wait = Some(wait.map_or(w, |x: f64| x.min(w)));
        // Search.
        if let Some(q) = self.search.due(now) {
            self.start_search(q);
        }
        if let Some(w) = self.search.wait_left(now) {
            want(w);
        }
        // Palette files.
        if let Some(p) = self.palette.as_mut() {
            if let Some(q) = p.file_due(now) {
                if let Some(ws) = self.ws.as_mut() {
                    if let Some(old) = p.req.take() {
                        ws.cancel(old);
                    }
                    p.req = Some(ws.send(margin_proto::Request::FindFiles { query: q, limit: crate::palette::FILE_LIMIT }, crate::workspace::Pending::FindFiles));
                }
            }
            if let Some(w) = p.wait_left(now) {
                want(w);
            }
        }
        // Hints.
        if let Some(w) = self.update_hints() {
            want(w);
        }
        // Tabs list → state.
        if std::mem::take(&mut self.tabs_changed) {
            self.snapshot_tabs();
        }
        let active = self.tabs.active_tab().map(|x| x.id);
        if active != self.last_active {
            self.last_active = active;
            if let Some(p) = self.tabs.active_file().map(|f| f.path.clone()) {
                for d in self.tree.reveal(&p) {
                    self.list_dir(d);
                }
            }
        }
        // Saves.
        if let Some(at) = self.settings_at {
            if now - at >= SETTINGS_DELAY {
                self.settings_at = None;
                self.persist.save_settings(self.settings.write());
            } else {
                want(SETTINGS_DELAY - (now - at));
            }
        }
        if let Some(at) = self.state_at {
            if now - at >= SETTINGS_DELAY {
                self.state_at = None;
                self.persist.save_state(self.state.write());
            } else {
                want(SETTINGS_DELAY - (now - at));
            }
        }
        if let Some(w) = wait {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(w + 0.005));
        }
    }

    fn smoke_check(&mut self, ctx: &egui::Context) {
        let Some(s) = self.smoke.as_mut() else { return };
        if s.done {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
        let fatal = self.ws.as_ref().and_then(|w| w.fatal.clone()).or_else(|| self.open_error.clone());
        let verdict = if let Some(msg) = fatal {
            Some(Err(format!("smoke: fatal {}", sanitize(&msg, 500))))
        } else if s.start.elapsed() > std::time::Duration::from_secs(20) {
            Some(Err("smoke: timeout".to_owned()))
        } else {
            let ready = self.ws.as_ref().is_some_and(|w| w.root.is_some() && w.ready());
            let file_ok = self.start_file.is_none() && self.tabs.active_file().is_none_or(|f| !matches!(f.view, View::Loading));
            if ready && file_ok {
                s.frames_after += 1;
            }
            (s.frames_after > 3).then(|| {
                let ws = self.ws.as_ref();
                let name = ws.map(Workspace::display_name).unwrap_or_default();
                let files = ws.and_then(|w| w.index).map_or(0, |i| i.files_total);
                let tab = self.tabs.active_file().map_or("-".to_owned(), |f| clean(f.path.as_str()));
                Ok(format!("smoke: ok root={name} files={files} tab={tab} sandbox={}/{}", self.gui_sandbox, self.opener.sandbox()))
            })
        };
        if let Some(v) = verdict {
            s.done = true;
            if let Ok(mut g) = s.verdict.lock() {
                *g = Some(v);
            }
            self.quit_ok = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

pub fn human_size(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f < K {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.1} KB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MB", f / K / K)
    } else {
        format!("{:.1} GB", f / K / K / K)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes() {
        assert_eq!(super::human_size(12), "12 B");
        assert_eq!(super::human_size(1_258_291), "1.2 MB");
    }
}
