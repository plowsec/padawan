//! What the app does: commands, engine replies, file operations, overlays, hints scheduling.

use std::path::PathBuf;

use egui::{pos2, vec2, Id, Key, Order, Rect, Sense};
use margin_code::Lang;
use margin_editor::{Document, EditAction, Wrap};
use margin_proto::limits::MAX_SNIPPET;
use margin_proto::{ErrCode, Event, IndexPhase, Out, RelPath, Reply, Request, SearchQuery, Site};

use crate::app::{waker, AfterSave, App, DocDone, Loc, Modal, Then, BIG_DOC, REINDEX_EVERY};
use crate::commands::Cmd;
use crate::hints::{self, SEG_CALLEES, SEG_CALLERS, SEG_REACH, SEG_SINKS};
use crate::menubar::MenuPick;
use crate::search_panel::thousands;
use crate::palette::{self, Palette, PaletteOut};
use crate::paths_view::PathsPanel;
use crate::picker::{Picker, PickerOut};
use crate::results_panel::ResultsPanel;
use crate::sanitize::{clean, floor_char, sanitize};
use crate::side::SideMode;
use crate::state::FolderTabs;
use crate::tabs::{Banner, FileTab, TabKind, View};
use crate::tree::{EditKind, TreeAction};
use crate::widgets;
use crate::workspace::{Pending, Workspace};

const SITE_LIMIT: u32 = 2000;
/// Paths asked for per request.
pub const PATHS_LIMIT: u32 = 200;
const MAX_NAV: usize = 100;

fn decode(data: Vec<u8>) -> (String, bool) {
    match String::from_utf8(data) {
        Ok(s) => (s, false),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), true),
    }
}

/// A line of the document around `col`, as a snippet `(text, text_col)` (≤ `MAX_SNIPPET` bytes).
fn local_snippet(line: &str, col: u32) -> (String, u32) {
    let c = floor_char(line, col as usize);
    let a = floor_char(line, c.saturating_sub(120));
    let b = floor_char(line, (a + MAX_SNIPPET).min(line.len()));
    (line[a..b].to_owned(), (c - a) as u32)
}

impl App {
    // ---- incoming ----

    pub(crate) fn drain(&mut self) {
        let mut outs = Vec::new();
        if let Some(ws) = &self.ws {
            // Bounded per frame: a flood cannot freeze the UI; the rest comes next frame.
            while outs.len() < 4096 {
                match ws.poll() {
                    Some(o) => outs.push(o),
                    None => break,
                }
            }
            if outs.len() == 4096 {
                self.ctx.request_repaint();
            }
        }
        for o in outs {
            match o {
                Out::Event(e) => self.on_event(e),
                Out::Reply(id, r) => self.on_reply(id, r),
            }
        }
        while let Some(l) = self.opener.poll_browse() {
            if let Some(p) = self.picker.as_mut() {
                p.set_listing(l);
            }
        }
        while let Ok(d) = self.doc_rx.try_recv() {
            self.apply_doc(d);
        }
        while let Ok(o) = self.outline_rx.try_recv() {
            self.apply_outline(o);
        }
        while let Ok(m) = self.md_rx.try_recv() {
            if let Some(pv) = self.tabs.file_by_id(m.tab).and_then(|f| f.preview.as_mut()) {
                pv.apply(m.rev, m.doc);
            }
        }
    }

    fn on_event(&mut self, e: Event) {
        let Some(ws) = self.ws.as_mut() else { return };
        match e {
            Event::Hello { root, name, .. } => {
                ws.root = Some(root);
                ws.name = Some(name);
                let root = ws.root_str();
                self.state.touch_recent(&root);
                self.state_at = Some(self.now);
                self.open_error = None;
                self.picker = None;
                self.restore_tabs(&root);
            }
            Event::Index(s) => {
                let was_ready = ws.ready();
                ws.index = Some(s);
                if s.phase == IndexPhase::Ready && !was_ready {
                    for t in &mut self.tabs.list {
                        if let Some(f) = t.file_mut() {
                            f.hints.lens_sent = None;
                        }
                    }
                }
            }
            Event::Fatal { msg } => {
                if ws.root.is_none() {
                    // The folder never opened: say why where the user is.
                    let msg = sanitize(&msg, 300);
                    if let Some(p) = self.picker.as_mut() {
                        p.error = Some(msg.clone());
                    }
                    self.open_error = Some(msg);
                    self.ws = None;
                    self.tree.clear();
                } else {
                    ws.fatal = Some(msg);
                }
            }
        }
    }

    fn on_reply(&mut self, id: u64, r: Reply) {
        let Some(ws) = self.ws.as_mut() else { return };
        // Streams keep their id until the end.
        let streaming = matches!(r, Reply::SearchBatch { .. });
        let p = if streaming { ws.pending.get(&id).cloned() } else { ws.pending.remove(&id) };
        let Some(p) = p else { return };
        match (p, r) {
            (Pending::ListDir(d), Reply::Dir { entries, truncated }) => self.tree.set_dir(d, entries, truncated),
            (Pending::ListDir(d), Reply::Err { msg, .. }) => self.tree.set_error(d, sanitize(&msg, 200)),
            (Pending::Read { tab, reload }, Reply::File { data, meta, binary }) => self.on_file(tab, reload, data, meta, binary),
            (Pending::Read { tab, reload }, Reply::Err { code, msg }) => {
                if reload {
                    self.toasts.error(&msg);
                } else if let Some(f) = self.tabs.file_by_id(tab) {
                    f.view = View::Message(match code {
                        ErrCode::TooLarge => "Too large to open".into(),
                        ErrCode::NotFound => "Not found".into(),
                        _ => sanitize(&msg, 200),
                    });
                }
            }
            (Pending::Write { tab, rev }, Reply::Meta(m)) => {
                let path = self.tabs.file_by_id(tab).map(|f| {
                    f.meta = Some(m);
                    f.saving = None;
                    f.banner = None;
                    if let View::Text(d) = &mut f.view {
                        if d.revision() == rev {
                            d.mark_saved();
                        }
                    }
                    f.path.clone()
                });
                if let Some(a) = self.after_save.as_mut() {
                    a.waiting.retain(|&w| w != tab);
                    if a.waiting.is_empty() {
                        let then = self.after_save.take().map(|a| a.then);
                        if let Some(t) = then {
                            self.proceed(t);
                        }
                    }
                }
                let _ = path;
            }
            (Pending::Write { tab, .. }, Reply::Err { code, msg }) => {
                self.after_save = None;
                if let Some(f) = self.tabs.file_by_id(tab) {
                    f.saving = None;
                    if code == ErrCode::Conflict {
                        f.banner = Some(Banner::Conflict);
                    }
                }
                if code != ErrCode::Conflict {
                    self.toasts.error(&format!("Not saved: {msg}"));
                }
            }
            (Pending::Stat { tab }, Reply::Meta(m)) => {
                let Some(f) = self.tabs.file_by_id(tab) else { return };
                let changed = f.meta.is_some_and(|old| old.mtime_ns != m.mtime_ns || old.size != m.size);
                if changed && f.saving.is_none() {
                    if f.dirty() {
                        f.banner = Some(Banner::ChangedDirty);
                    } else {
                        self.reload_tab(tab);
                    }
                }
            }
            (Pending::FindFiles, Reply::FileMatches { items, total_files }) => {
                if let Some(p) = self.palette.as_mut().filter(|p| p.req == Some(id)) {
                    p.req = None;
                    p.set_files(items, total_files);
                }
            }
            (Pending::FindFiles, Reply::Err { .. }) => {
                if let Some(p) = self.palette.as_mut().filter(|p| p.req == Some(id)) {
                    p.req = None;
                }
            }
            (Pending::Names, Reply::FileMatches { items, total_files }) => {
                if self.side.req == Some(id) {
                    self.side.req = None;
                    self.side.set_items(items, total_files);
                }
            }
            (Pending::Names, Reply::Err { msg, code }) => {
                if self.side.req == Some(id) {
                    self.side.req = None;
                    self.side.set_items(Vec::new(), 0);
                    if code != ErrCode::Cancelled {
                        self.toasts.error(&msg);
                    }
                }
            }
            (Pending::Search(tab), r) => {
                let Some(s) = self.tabs.search_by_id(tab).filter(|s| s.req == Some(id)) else {
                    if !streaming {
                        return;
                    }
                    // Stale stream (tab closed or re-run): stop tracking it.
                    if let Some(ws) = self.ws.as_mut() {
                        ws.pending.remove(&id);
                    }
                    return;
                };
                match r {
                    Reply::SearchBatch { hits } => s.on_batch(hits),
                    Reply::SearchDone(st) => s.on_done(st),
                    Reply::Err { code: ErrCode::Cancelled, .. } => {}
                    Reply::Err { msg, .. } => s.on_err(&msg),
                    _ => {}
                }
                if !streaming {
                    if let Some(ws) = self.ws.as_mut() {
                        ws.pending.remove(&id);
                    }
                }
            }
            (Pending::Replace(tab), Reply::Replaced { files, replacements, changed, skipped, cancelled }) => self.on_replaced(tab, files, replacements, changed, skipped, cancelled),
            (Pending::Replace(_), Reply::Err { msg, .. }) => self.toasts.error(&format!("Not replaced: {msg}")),
            (Pending::Paths(_), Reply::Paths { paths, truncated }) => {
                if let Some(p) = self.paths.as_mut() {
                    p.set(paths, truncated);
                }
            }
            (Pending::Paths(_), Reply::Err { msg, .. }) => {
                if let Some(p) = self.paths.as_mut() {
                    p.set(Vec::new(), false);
                }
                self.toasts.error(&msg);
            }
            (Pending::SaveAsDir { tab, path, rest }, r) => self.on_save_as_dir(tab, path, rest, r),
            (Pending::SaveAs { tab, path, rev }, r) => self.on_save_as(tab, path, rev, r),
            (Pending::Lens { tab, rev }, Reply::Lens { fns }) => {
                let Some(f) = self.tabs.file_by_id(tab) else { return };
                if f.hints.outline_rev != Some(rev) {
                    return;
                }
                if let Some(o) = f.hints.outline.clone() {
                    let mut fns = fns;
                    fns.truncate(o.fns.len());
                    f.hints.lenses = hints::build(&o, &fns);
                }
            }
            (Pending::Callers(_) | Pending::Reach(_), Reply::Sites { sites, truncated }) => {
                if let Some(r) = self.results.as_mut() {
                    r.set(sites, truncated);
                }
            }
            (Pending::Defs(name), Reply::Sites { mut sites, truncated }) => match sites.len() {
                0 => self.toasts.notice(&format!("No definition of `{}` in this folder", sanitize(&name, 60))),
                1 => {
                    let s = sites.remove(0);
                    self.open_file(s.path, Some((s.line, s.col, s.len)), true);
                }
                _ => {
                    let mut r = ResultsPanel::new(format!("Definitions of {}", sanitize(&name, 60)), self.results_h);
                    r.set(sites, truncated);
                    r.focus = true;
                    self.paths = None;
                    self.results = Some(r);
                }
            },
            (Pending::Callers(_) | Pending::Reach(_) | Pending::Defs(_), Reply::Err { msg, .. }) => {
                if let Some(r) = self.results.as_mut() {
                    r.set(Vec::new(), false);
                }
                self.toasts.error(&msg);
            }
            (Pending::Create(p), Reply::Meta(_)) => {
                self.refresh_dir(p.parent().unwrap_or_default());
                self.open_file(p, None, true);
            }
            (Pending::FileOp(d), Reply::Done) => self.refresh_dir(d),
            (Pending::Deleted(p), Reply::Done) => {
                self.refresh_dir(p.parent().unwrap_or_default());
                let ids: Vec<u64> = self.tabs.list.iter().filter(|t| t.file().is_some_and(|f| f.path.starts_with(&p) && !f.dirty())).map(|t| t.id).collect();
                for id in ids {
                    self.close_tab_now(id);
                }
            }
            (Pending::Rename { from, to }, Reply::Done) => {
                self.refresh_dir(from.parent().unwrap_or_default());
                if to.parent() != from.parent() {
                    self.refresh_dir(to.parent().unwrap_or_default());
                }
                for t in &mut self.tabs.list {
                    if let Some(f) = t.file_mut() {
                        if f.path.starts_with(&from) {
                            let rest = &f.path.as_str()[from.as_str().len()..];
                            if let Ok(np) = RelPath::new(&format!("{}{rest}", to.as_str())) {
                                if let View::Text(d) = &mut f.view {
                                    d.set_lang(Lang::from_path(np.as_str()));
                                }
                                f.path = np;
                            }
                        }
                    }
                }
                self.tabs_changed = true;
            }
            (Pending::Create(_) | Pending::FileOp(_) | Pending::Deleted(_) | Pending::Rename { .. }, Reply::Err { msg, .. }) => self.toasts.error(&msg),
            (Pending::Stat { .. } | Pending::Lens { .. } | Pending::Ignore, _) => {}
            _ => {}
        }
    }

    fn on_file(&mut self, tab: u64, reload: bool, data: Vec<u8>, meta: margin_proto::Meta, binary: bool) {
        let Some(f) = self.tabs.file_by_id(tab) else { return };
        if binary {
            f.view = View::Binary(meta.size);
            f.meta = Some(meta);
            return;
        }
        let lang = Lang::from_path(f.path.as_str());
        if data.len() > BIG_DOC {
            let tx = self.doc_tx.clone();
            let wake = waker(&self.ctx);
            let _ = std::thread::Builder::new().name("margin-doc".into()).spawn(move || {
                let (text, bad) = decode(data);
                let doc = Document::new(&text, lang);
                let _ = tx.send(DocDone { tab, doc, bad_utf8: bad, meta, reload });
                wake();
            });
            return;
        }
        let (text, bad) = decode(data);
        let doc = Document::new(&text, lang);
        self.apply_doc(DocDone { tab, doc, bad_utf8: bad, meta, reload });
    }

    fn apply_doc(&mut self, d: DocDone) {
        let Some(f) = self.tabs.file_by_id(d.tab) else { return };
        let mut doc = d.doc;
        doc.set_read_only(d.bad_utf8 || d.meta.readonly);
        let keep = (f.st.cursor(), f.st.top_line());
        f.view = View::Text(Box::new(doc));
        f.meta = Some(d.meta);
        f.bad_utf8 = d.bad_utf8;
        f.banner = None;
        f.hints.outline_rev = None;
        f.hints.lens_sent = None;
        if d.reload {
            f.st.goto(keep.0 .0, keep.0 .1);
            f.st.set_top_line(keep.1);
        } else if let Some((l, c, n)) = f.pending_select.take() {
            if n > 0 {
                f.st.select(l, c, n);
            } else {
                f.st.goto(l, c);
            }
        }
        if f.preview.is_none() && f.markdown() {
            f.preview = Some(Box::new(crate::md_preview::Preview { on: self.settings.md_preview, ..Default::default() }));
        }
        if let Some(frag) = f.pending_anchor.take() {
            self.jump_anchor(d.tab, &frag);
        }
    }

    // ---- folder ----

    /// Open a folder, asking about unsaved tabs first.
    pub(crate) fn open_folder(&mut self, path: PathBuf) {
        let dirty = self.dirty_file_tabs();
        if dirty.is_empty() {
            self.open_folder_now(path);
        } else {
            self.modal = Some(Modal::Unsaved { tabs: dirty, then: Then::Open(path) });
        }
    }

    pub(crate) fn open_folder_now(&mut self, path: PathBuf) {
        self.snapshot_tabs();
        // Untitled buffers belong to no folder: they stay.
        self.tabs.retain(|t| t.file().is_some_and(FileTab::is_untitled));
        self.tree.clear();
        self.side = Default::default();
        self.results = None;
        self.paths = None;
        self.back.clear();
        self.forward.clear();
        self.restored = false;
        self.ws = None;
        let client = self.opener.open(&path, &self.settings.index_opts(), waker(&self.ctx));
        let mut ws = Workspace::new(client, path);
        ws.send(Request::ListDir { path: RelPath::root() }, Pending::ListDir(RelPath::root()));
        self.ws = Some(ws);
        self.last_reindex = self.now;
        self.state_at = Some(self.now);
    }

    pub(crate) fn reopen(&mut self, path: PathBuf) {
        // Keep the tabs' documents; re-read nothing (unsaved edits survive). The new engine serves them.
        let tabs = std::mem::take(&mut self.tabs);
        self.open_folder_now(path);
        self.tabs = tabs;
        self.restored = true;
        let dirs = self.tree.loaded_dirs();
        for d in dirs {
            self.list_dir(d);
        }
    }

    fn restore_tabs(&mut self, root: &str) {
        if self.restored {
            return;
        }
        self.restored = true;
        let mut active = None;
        if self.settings.restore_tabs {
            if let Some(ft) = self.state.tabs_of(root).cloned() {
                for t in &ft.tabs {
                    if let Ok(p) = RelPath::new(t) {
                        self.open_file(p, None, false);
                    }
                }
                active = ft.active.and_then(|a| RelPath::new(&a).ok());
            }
        }
        if let Some(f) = self.start_file.take() {
            let f = f.replace('\\', "/");
            match RelPath::new(f.trim_start_matches("./")) {
                Ok(p) if !p.is_root() => active = Some(p),
                _ => self.toasts.error("The file to open is not inside the folder"),
            }
        }
        if let Some(a) = active {
            self.open_file(a, None, false);
        }
        self.tabs_changed = true;
    }

    pub(crate) fn snapshot_tabs(&mut self) {
        let Some(ws) = &self.ws else { return };
        if !self.restored || ws.root.is_none() {
            return;
        }
        let tabs: Vec<String> = self.tabs.list.iter().filter_map(|t| t.file().filter(|f| !f.is_untitled()).map(|f| f.path.to_string())).collect();
        let active = self.tabs.active_file().filter(|f| !f.is_untitled()).map(|f| f.path.to_string());
        let ft = FolderTabs { root: ws.root_str(), tabs, active };
        if self.state.tabs_of(&ft.root) != Some(&ft) {
            self.state.set_tabs(ft);
            self.state_at = Some(self.now);
        }
    }

    pub(crate) fn close_folder(&mut self) {
        let dirty = self.dirty_file_tabs();
        if !dirty.is_empty() {
            self.modal = Some(Modal::Unsaved { tabs: dirty, then: Then::CloseFolder });
            return;
        }
        self.close_folder_now();
    }

    fn close_folder_now(&mut self) {
        self.snapshot_tabs();
        self.tabs.retain(|t| t.file().is_some_and(FileTab::is_untitled));
        self.tree.clear();
        self.side = Default::default();
        self.results = None;
        self.paths = None;
        self.ws = None;
        self.palette = None;
    }

    /// Dirty tabs with a file on disk (untitled buffers are kept by hot exit: no prompt for them).
    pub(crate) fn dirty_file_tabs(&self) -> Vec<u64> {
        self.tabs.list.iter().filter(|t| t.dirty() && t.file().is_some_and(|f| !f.is_untitled())).map(|t| t.id).collect()
    }

    pub(crate) fn proceed(&mut self, then: Then) {
        match then {
            Then::CloseTab(id) => self.close_tab_now(id),
            Then::CloseTabs(ids) => {
                for id in ids {
                    self.close_tab_now(id);
                }
            }
            Then::CloseFolder => self.close_folder_now(),
            Then::Open(p) => self.open_folder_now(p),
            Then::Quit => {
                self.quit_ok = true;
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    // ---- files ----

    pub(crate) fn list_dir(&mut self, d: RelPath) {
        if let Some(ws) = self.ws.as_mut() {
            if ws.pending.values().any(|p| matches!(p, Pending::ListDir(x) if *x == d)) {
                return;
            }
            ws.send(Request::ListDir { path: d.clone() }, Pending::ListDir(d));
        }
    }

    pub(crate) fn refresh_dir(&mut self, d: RelPath) {
        if d.is_root() || self.tree.expanded.contains(&d) || self.tree.dirs.contains_key(&d) {
            self.list_dir(d);
        }
    }

    fn here(&self) -> Option<Loc> {
        let f = self.tabs.active_file().filter(|f| !f.is_untitled())?;
        let (line, col) = f.st.cursor();
        Some(Loc { path: f.path.clone(), line, col })
    }

    /// Open `path` (or focus its tab); `select`: (line, col, len) to select once loaded; `nav`: push history.
    pub(crate) fn open_file(&mut self, path: RelPath, select: Option<(u32, u32, u32)>, nav: bool) {
        if self.ws.is_none() || path.is_root() {
            return;
        }
        if nav {
            if let Some(h) = self.here() {
                if h.path != path || select.is_some_and(|s| s.0 != h.line) {
                    self.back.push(h);
                    if self.back.len() > MAX_NAV {
                        self.back.remove(0);
                    }
                    self.forward.clear();
                }
            }
        }
        if let Some(i) = self.tabs.find_file(&path) {
            self.tabs.activate(i);
            if let Some(f) = self.tabs.list[i].file_mut() {
                f.focus_editor = true;
                if let Some((l, c, n)) = select {
                    if matches!(f.view, View::Text(_)) {
                        if n > 0 {
                            f.st.select(l, c, n);
                        } else {
                            f.st.goto(l, c);
                        }
                    } else {
                        f.pending_select = Some((l, c, n));
                    }
                }
            }
        } else {
            let mut ft = FileTab::new(path.clone());
            ft.pending_select = select;
            let i = self.tabs.add(TabKind::File(Box::new(ft)));
            let id = self.tabs.list[i].id;
            if let Some(ws) = self.ws.as_mut() {
                ws.send(Request::ReadFile { path: path.clone() }, Pending::Read { tab: id, reload: false });
            }
        }
        self.tabs_changed = true;
        self.focus_editor = true;
    }

    pub(crate) fn save_tab(&mut self, id: u64, overwrite: bool) {
        let Some(f) = self.tabs.file_by_id(id) else { return };
        if f.is_untitled() {
            self.ask_save_as(id, None);
            return;
        }
        let View::Text(doc) = &f.view else { return };
        if doc.read_only() || f.saving.is_some() {
            return;
        }
        let req = Request::WriteFile { path: f.path.clone(), data: doc.text().into_bytes(), expect_mtime_ns: if overwrite { None } else { f.meta.map(|m| m.mtime_ns) }, create_new: false };
        let rev = doc.revision();
        if let Some(ws) = self.ws.as_mut() {
            let rid = ws.send(req, Pending::Write { tab: id, rev });
            if let Some(f) = self.tabs.file_by_id(id) {
                f.saving = Some(rid);
            }
        }
    }

    pub(crate) fn reload_tab(&mut self, id: u64) {
        let Some(f) = self.tabs.file_by_id(id).filter(|f| !f.is_untitled()) else { return };
        let path = f.path.clone();
        f.banner = None;
        if let Some(ws) = self.ws.as_mut() {
            ws.send(Request::ReadFile { path }, Pending::Read { tab: id, reload: true });
        }
    }

    pub(crate) fn close_tab(&mut self, id: u64) {
        let Some(i) = self.tabs.by_id(id) else { return };
        if self.tabs.list[i].dirty() {
            self.modal = Some(Modal::Unsaved { tabs: vec![id], then: Then::CloseTab(id) });
        } else {
            self.close_tab_now(id);
        }
    }

    pub(crate) fn close_tab_now(&mut self, id: u64) {
        if let Some(i) = self.tabs.by_id(id) {
            if let Some(t) = self.tabs.remove(i) {
                match &t.kind {
                    TabKind::Search(s) => {
                        if let (Some(r), Some(ws)) = (s.req, self.ws.as_mut()) {
                            ws.cancel(r);
                        }
                    }
                    TabKind::File(f) => {
                        if let Some(n) = f.untitled {
                            self.persist.remove_untitled(n);
                        }
                    }
                    TabKind::Settings => {}
                }
            }
            self.tabs_changed = true;
            self.focus_editor = true;
        }
    }

    pub(crate) fn tree_action(&mut self, a: TreeAction) {
        let Some(ws) = self.ws.as_mut() else { return };
        match a {
            TreeAction::Open(p) => {
                self.open_file(p, None, true);
            }
            TreeAction::Load(d) => self.list_dir(d),
            TreeAction::CreateFile(p) => {
                ws.send(Request::WriteFile { path: p.clone(), data: Vec::new(), expect_mtime_ns: None, create_new: true }, Pending::Create(p));
            }
            TreeAction::CreateDir(p) => {
                let parent = p.parent().unwrap_or_default();
                ws.send(Request::CreateDir { path: p }, Pending::FileOp(parent));
            }
            TreeAction::Rename(from, to) => {
                ws.send(Request::Rename { from: from.clone(), to: to.clone() }, Pending::Rename { from, to });
            }
            TreeAction::AskDelete(path, dir) => self.modal = Some(Modal::Delete { path, dir }),
            TreeAction::Copy(s) => self.ctx.copy_text(s),
            TreeAction::CopyAbs(p) => {
                let abs = abs_path(&ws.root_str(), &p);
                self.ctx.copy_text(abs);
            }
            TreeAction::Error(m) => self.toasts.error(&m),
            TreeAction::Menu(row, pos) => self.open_tree_menu(row, pos),
        }
    }

    /// Run `q` in search tab `tab` (cancels its running request).
    pub(crate) fn start_search(&mut self, tab: u64, q: SearchQuery) {
        let Some(ws) = self.ws.as_mut() else { return };
        let Some(s) = self.tabs.list.iter_mut().find(|t| t.id == tab).and_then(|t| t.search_mut()) else { return };
        if let Some(old) = s.req.take() {
            ws.cancel(old);
        }
        s.edited_at = None;
        let id = ws.send(Request::Search { query: q.clone() }, Pending::Search(tab));
        s.started(id, q);
    }

    // ---- window ----

    pub(crate) fn window_events(&mut self, ctx: &egui::Context) {
        // The last WindowFocused event of the frame wins over the polled flag.
        let focused = ctx.input(|i| {
            i.raw.events.iter().rev().find_map(|e| if let egui::Event::WindowFocused(f) = e { Some(*f) } else { None }).unwrap_or(i.focused)
        });
        if focused && !self.was_focused {
            self.focus_regained();
        }
        if !focused && self.was_focused && self.settings.autosave {
            for id in self.dirty_file_tabs() {
                self.save_tab(id, false);
            }
        }
        self.was_focused = focused;
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.open_folder(p);
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_ok {
            let dirty = self.dirty_file_tabs();
            if dirty.is_empty() {
                self.quit_ok = true;
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.modal = Some(Modal::Unsaved { tabs: dirty, then: Then::Quit });
            }
        }
        if self.quit_ok {
            self.snapshot_tabs();
        }
    }

    pub(crate) fn focus_regained(&mut self) {
        let Some(ws) = self.ws.as_mut() else { return };
        for t in &self.tabs.list {
            if let Some(f) = t.file() {
                if f.meta.is_some() && !f.is_untitled() {
                    ws.send(Request::Stat { path: f.path.clone() }, Pending::Stat { tab: t.id });
                }
            }
        }
        if self.now - self.last_reindex >= REINDEX_EVERY {
            self.last_reindex = self.now;
            ws.send(Request::Reindex { opts: self.settings.index_opts() }, Pending::Ignore);
        }
        for d in self.tree.loaded_dirs() {
            self.list_dir(d);
        }
    }

    // ---- settings ----

    pub(crate) fn settings_changed(&mut self, reindex: bool) {
        self.theme = margin_theme::Theme::new(self.settings.mode, self.settings.look);
        self.settings_at = Some(self.now);
        if reindex {
            if let Some(ws) = self.ws.as_mut() {
                self.last_reindex = self.now;
                ws.send(Request::Reindex { opts: self.settings.index_opts() }, Pending::Ignore);
            }
        }
        self.ctx.request_repaint();
    }

    pub(crate) fn note_mru(&mut self, c: Cmd) {
        if matches!(c, Cmd::Palette) {
            return;
        }
        self.mru.retain(|m| *m != c);
        self.mru.insert(0, c);
        self.mru.truncate(16);
    }

    // ---- commands ----

    fn active_id(&self) -> Option<u64> {
        self.tabs.active_tab().map(|t| t.id)
    }

    fn editor_selection(&self) -> Option<String> {
        let f = self.tabs.active_file()?;
        let s = f.st.selected_text(f.doc()?);
        (!s.is_empty() && !s.contains('\n') && s.len() <= 200).then_some(s)
    }

    pub(crate) fn run_cmd(&mut self, c: Cmd) {
        // Menu bar / palette Copy and Select All: the Markdown preview when it was used last.
        if matches!(c, Cmd::Copy | Cmd::SelectAll) {
            let on_preview = self.tabs.active_file().is_some_and(|f| f.doc().is_some() && f.preview.as_ref().is_some_and(|p| p.on && p.active));
            if on_preview && self.active_id().is_some_and(|tab| self.md_edit(tab, c == Cmd::SelectAll)) {
                return;
            }
        }
        if let Some(a) = edit_action(c) {
            self.edit(a);
            return;
        }
        match c {
            Cmd::Palette => {
                if self.palette.as_ref().is_some_and(|p| p.mode == palette::Mode::All) {
                    self.close_palette();
                } else {
                    self.picker = None;
                    self.palette = Some(Palette::new(palette::Mode::All, ""));
                }
            }
            Cmd::Shortcuts => {
                self.picker = None;
                self.palette = Some(Palette::new(palette::Mode::All, ">"));
            }
            Cmd::GoToFile => {
                if self.ws.is_some() {
                    self.picker = None;
                    self.palette = Some(Palette::new(palette::Mode::Files, ""));
                }
            }
            Cmd::GoToLine => {
                self.picker = None;
                self.palette = Some(Palette::new(palette::Mode::All, ":"));
            }
            Cmd::GoToSymbol => {
                self.picker = None;
                self.palette = Some(Palette::new(palette::Mode::All, "@"));
            }
            Cmd::OpenFolder => {
                self.palette = None;
                self.modal = None;
                let mut p = Picker::new();
                let start = self.ws.as_ref().and_then(|w| PathBuf::from(w.root_str()).parent().map(PathBuf::from));
                if let Some(s) = &start {
                    p.path = s.to_string_lossy().into_owned();
                }
                self.picker = Some(p);
                self.opener.browse(start.as_deref(), waker(&self.ctx));
            }
            Cmd::CloseFolder => self.close_folder(),
            Cmd::NewFile => self.new_untitled(),
            Cmd::NewWindow => self.opener.new_window(None),
            Cmd::NewFileInFolder | Cmd::NewFolder => {
                if self.ws.is_none() {
                    return;
                }
                self.show_sidebar();
                self.side.mode = SideMode::Names;
                self.side.query.clear();
                self.side.include.clear();
                self.side.exclude.clear();
                let dir = self
                    .tree
                    .selected
                    .clone()
                    .or_else(|| self.tabs.active_file().filter(|f| !f.is_untitled()).map(|f| f.path.clone()))
                    .map(|p| if self.tree.is_dir(&p) { p } else { p.parent().unwrap_or_default() })
                    .unwrap_or_default();
                self.tree.start_edit(if c == Cmd::NewFileInFolder { EditKind::NewFile(dir) } else { EditKind::NewFolder(dir) });
            }
            Cmd::Save => {
                if let Some(id) = self.active_id() {
                    self.save_tab(id, false);
                }
            }
            Cmd::SaveAs => {
                if let Some(id) = self.tabs.active_tab().filter(|t| t.file().is_some_and(|f| f.doc().is_some())).map(|t| t.id) {
                    self.ask_save_as(id, None);
                }
            }
            Cmd::SaveAll => {
                for id in self.dirty_file_tabs() {
                    self.save_tab(id, false);
                }
            }
            Cmd::CloseTab => {
                if let Some(id) = self.active_id() {
                    self.close_tab(id);
                }
            }
            Cmd::NextTab => {
                self.tabs.cycle(1);
                self.focus_editor = true;
            }
            Cmd::PrevTab => {
                self.tabs.cycle(-1);
                self.focus_editor = true;
            }
            Cmd::Find | Cmd::Replace => {
                if let Some(s) = self.tabs.active.and_then(|a| self.tabs.list.get_mut(a)).and_then(|t| t.search_mut()) {
                    // In a search tab: show its replace row.
                    s.show_replace = true;
                    if c == Cmd::Replace {
                        s.focus_replace = true;
                    } else {
                        s.focus_query = true;
                    }
                    return;
                }
                let seed = self.editor_selection();
                if let Some(f) = self.tabs.active_file_mut() {
                    f.st.open_find(seed.as_deref(), c == Cmd::Replace);
                }
            }
            Cmd::FindNext | Cmd::FindPrev => {
                if let Some(f) = self.tabs.active_file_mut() {
                    if let View::Text(d) = &f.view {
                        if c == Cmd::FindNext {
                            f.st.find_next(d);
                        } else {
                            f.st.find_prev(d);
                        }
                    }
                }
            }
            Cmd::SearchFolder | Cmd::ShowFiles => {
                if self.ws.is_none() {
                    return;
                }
                if c == Cmd::SearchFolder {
                    if let Some(s) = self.editor_selection() {
                        self.side.query = s;
                    }
                    self.side.mode = SideMode::Content;
                } else {
                    if self.side.mode != SideMode::Names {
                        self.side.query.clear();
                    }
                    self.side.mode = SideMode::Names;
                }
                self.side.edited_at = Some(self.now - 1.0);
                self.show_sidebar();
                self.side.focus = true;
                // The field wins over a pending editor focus request.
                self.focus_editor = false;
                if let Some(f) = self.tabs.active_file_mut() {
                    f.focus_editor = false;
                }
            }
            Cmd::ReplaceFolder => {
                if self.ws.is_none() {
                    return;
                }
                self.open_search_tab(None, true);
            }
            Cmd::GoToDef => {
                let w = self.word_at_cursor.clone();
                self.goto_definition(w);
            }
            Cmd::FindCallers => {
                let w = self.word_at_cursor.clone();
                self.find_callers(w);
            }
            Cmd::Back => {
                if let Some(l) = self.back.pop() {
                    if let Some(h) = self.here() {
                        self.forward.push(h);
                    }
                    self.open_file(l.path, Some((l.line, l.col, 0)), false);
                }
            }
            Cmd::Forward => {
                if let Some(l) = self.forward.pop() {
                    if let Some(h) = self.here() {
                        self.back.push(h);
                    }
                    self.open_file(l.path, Some((l.line, l.col, 0)), false);
                }
            }
            Cmd::ToggleSidebar => {
                self.state.sidebar_visible = !self.state.sidebar_visible;
                self.state_at = Some(self.now);
            }
            Cmd::ToggleMenuBar => {
                self.settings.menu_bar = !self.settings.menu_bar;
                self.settings_changed(false);
            }
            Cmd::MdPreview => self.toggle_preview(),
            Cmd::ToggleWrap => {
                let prose = self.tabs.active_file().and_then(|f| f.doc()).is_some_and(|d| d.lang().is_prose());
                self.settings.wrap = match self.settings.wrap {
                    Wrap::Off => Wrap::On,
                    Wrap::On => Wrap::Off,
                    Wrap::Auto => {
                        if prose {
                            Wrap::Off
                        } else {
                            Wrap::On
                        }
                    }
                };
                self.settings_changed(false);
            }
            Cmd::ToggleHints => {
                self.settings.code_hints = !self.settings.code_hints;
                self.settings_changed(false);
            }
            Cmd::ToggleWhitespace => {
                self.settings.show_whitespace = !self.settings.show_whitespace;
                self.settings_changed(false);
            }
            Cmd::ToggleLineNumbers => {
                self.settings.line_numbers = !self.settings.line_numbers;
                self.settings_changed(false);
            }
            Cmd::ToggleTheme => {
                self.settings.mode = match self.settings.mode {
                    margin_theme::Mode::Dark => margin_theme::Mode::Light,
                    margin_theme::Mode::Light => margin_theme::Mode::Dark,
                };
                self.settings_changed(false);
            }
            Cmd::ZoomIn | Cmd::ZoomOut | Cmd::ZoomReset => {
                let z = match c {
                    Cmd::ZoomIn => self.settings.zoom + 0.1,
                    Cmd::ZoomOut => self.settings.zoom - 0.1,
                    _ => 1.0,
                };
                self.settings.zoom = ((z * 10.0).round() / 10.0).clamp(*crate::settings::ZOOM.start(), *crate::settings::ZOOM.end());
                self.settings_changed(false);
            }
            Cmd::Settings => match self.tabs.list.iter().position(|t| matches!(t.kind, TabKind::Settings)) {
                Some(i) => self.tabs.activate(i),
                None => {
                    self.tabs.add(TabKind::Settings);
                }
            },
            Cmd::About => {
                self.palette = None;
                self.modal = Some(Modal::About);
            }
            Cmd::RebuildIndex => {
                if let Some(ws) = self.ws.as_mut() {
                    self.last_reindex = self.now;
                    ws.send(Request::Reindex { opts: self.settings.index_opts() }, Pending::Ignore);
                    self.toasts.notice("Rebuilding the index");
                }
            }
            Cmd::RevertFile => {
                if let Some(id) = self.tabs.active_file().filter(|f| !f.is_untitled()).and_then(|_| self.active_id()) {
                    self.reload_tab(id);
                }
            }
            Cmd::CopyPath | Cmd::CopyRelPath => {
                if let (Some(f), Some(ws)) = (self.tabs.active_file().filter(|f| !f.is_untitled()), self.ws.as_ref()) {
                    let s = if c == Cmd::CopyPath { abs_path(&ws.root_str(), &f.path) } else { f.path.to_string() };
                    self.ctx.copy_text(s);
                }
            }
            Cmd::Quit => {
                let dirty = self.dirty_file_tabs();
                if dirty.is_empty() {
                    self.proceed(Then::Quit);
                } else {
                    self.modal = Some(Modal::Unsaved { tabs: dirty, then: Then::Quit });
                }
            }
            Cmd::Undo | Cmd::Redo | Cmd::Cut | Cmd::Copy | Cmd::Paste | Cmd::SelectAll | Cmd::ToggleComment | Cmd::Indent | Cmd::Outdent | Cmd::DuplicateLine | Cmd::DeleteLine | Cmd::MoveLineUp | Cmd::MoveLineDown => {}
        }
    }

    fn show_sidebar(&mut self) {
        if !self.state.sidebar_visible {
            self.state.sidebar_visible = true;
            self.state_at = Some(self.now);
        }
    }

    /// Queue an edit command on the active text tab.
    pub(crate) fn edit(&mut self, a: EditAction) {
        let Some(f) = self.tabs.active_file_mut().filter(|f| f.doc().is_some()) else { return };
        f.st.run(a);
        f.focus_editor = true;
        if self.edit_log.len() >= 64 {
            self.edit_log.remove(0);
        }
        self.edit_log.push(a);
        self.ctx.request_repaint();
    }

    /// Can `c` run now? (menu bar greys the others).
    pub(crate) fn enabled(&self, c: Cmd) -> bool {
        let ws = self.ws.is_some();
        let text = self.tabs.active_file().is_some_and(|f| f.doc().is_some());
        let file = self.tabs.active_file().is_some_and(|f| !f.is_untitled() && f.doc().is_some());
        let editable = self.tabs.active_file().and_then(|f| f.doc()).is_some_and(|d| !d.read_only());
        match c {
            Cmd::Undo => editable && self.tabs.active_file().and_then(|f| f.doc()).is_some_and(Document::can_undo),
            Cmd::Redo => editable && self.tabs.active_file().and_then(|f| f.doc()).is_some_and(Document::can_redo),
            Cmd::Cut | Cmd::Paste | Cmd::ToggleComment | Cmd::Indent | Cmd::Outdent | Cmd::DuplicateLine | Cmd::DeleteLine | Cmd::MoveLineUp | Cmd::MoveLineDown => editable,
            Cmd::Copy | Cmd::SelectAll | Cmd::Find | Cmd::FindNext | Cmd::FindPrev | Cmd::GoToLine | Cmd::GoToSymbol | Cmd::SaveAs => text,
            Cmd::Replace => text || self.tabs.active_tab().is_some_and(|t| t.search().is_some()),
            Cmd::Save => self.tabs.active_tab().is_some_and(|t| t.dirty()),
            Cmd::SaveAll => !self.dirty_file_tabs().is_empty(),
            Cmd::RevertFile | Cmd::CopyPath | Cmd::CopyRelPath => file,
            Cmd::CloseTab | Cmd::NextTab | Cmd::PrevTab => !self.tabs.list.is_empty(),
            Cmd::CloseFolder | Cmd::GoToFile | Cmd::SearchFolder | Cmd::ReplaceFolder | Cmd::ShowFiles | Cmd::RebuildIndex | Cmd::NewFileInFolder | Cmd::NewFolder => ws,
            Cmd::GoToDef | Cmd::FindCallers => ws && text,
            Cmd::Back => !self.back.is_empty(),
            Cmd::Forward => !self.forward.is_empty(),
            Cmd::MdPreview => self.tabs.active_file().is_some_and(|f| f.doc().is_some() && f.markdown()),
            Cmd::ToggleSidebar => ws,
            _ => true,
        }
    }

    fn close_palette(&mut self) {
        if let (Some(p), Some(ws)) = (self.palette.take(), self.ws.as_mut()) {
            if let Some(r) = p.req {
                ws.cancel(r);
            }
        }
        self.focus_editor = true;
    }

    // ---- code navigation ----

    pub(crate) fn goto_definition(&mut self, word: String) {
        if word.is_empty() || self.ws.is_none() {
            return;
        }
        let w = word.chars().take(200).collect::<String>();
        if let Some(ws) = self.ws.as_mut() {
            ws.send(Request::Defs { name: w.clone(), limit: SITE_LIMIT }, Pending::Defs(w));
        }
    }

    /// Call paths from function `name` to sinks → paths view.
    pub(crate) fn open_paths(&mut self, name: String) {
        if name.is_empty() || self.ws.is_none() {
            return;
        }
        let w: String = name.chars().take(200).collect();
        self.results = None;
        self.paths = Some(PathsPanel::new(&w, self.results_h));
        if let Some(ws) = self.ws.as_mut() {
            ws.send(Request::Paths { name: w.clone(), sink: String::new(), limit: PATHS_LIMIT }, Pending::Paths(w));
        }
    }

    pub(crate) fn find_callers(&mut self, word: String) {
        if word.is_empty() || self.ws.is_none() {
            return;
        }
        let w: String = word.chars().take(200).collect();
        let mut r = ResultsPanel::new(format!("Callers of {}", sanitize(&w, 60)), self.results_h);
        r.focus = true;
        self.paths = None;
        self.results = Some(r);
        if let Some(ws) = self.ws.as_mut() {
            ws.send(Request::Callers { name: w.clone(), limit: SITE_LIMIT }, Pending::Callers(w));
        }
    }

    pub(crate) fn lens_clicked(&mut self, tab: u64, line: u32, seg: u32) {
        let Some(f) = self.tabs.file_by_id(tab) else { return };
        let Some(o) = f.hints.outline.clone() else { return };
        let Some(def) = o.fns.iter().find(|d| d.line == line) else { return };
        let name = def.name.clone();
        let path = f.path.clone();
        let local = |sinks: bool| -> Vec<Site> {
            let doc = f.doc();
            def.calls
                .iter()
                .filter(|c| !sinks || c.sink.is_some())
                .map(|c| {
                    let (text, text_col) = doc.map(|d| local_snippet(d.line(c.line), c.col)).unwrap_or_default();
                    let context = if sinks { c.sink.map(|k| k.label().to_owned()).unwrap_or_default() } else { c.qualified.clone() };
                    Site { path: path.clone(), line: c.line, col: c.col, len: c.name.len() as u32, text, text_col, context }
                })
                .collect()
        };
        match seg {
            SEG_CALLERS => self.find_callers(name),
            SEG_CALLEES => {
                let sites = local(false);
                let mut r = ResultsPanel::new(format!("Calls in {}", sanitize(&name, 60)), self.results_h);
                r.set(sites, false);
                r.focus = true;
                self.paths = None;
                self.results = Some(r);
            }
            SEG_SINKS | SEG_REACH => self.open_paths(name),
            _ => {}
        }
    }

    /// Schedule outline / lens work for the active tab. Returns seconds until the next check.
    pub(crate) fn update_hints(&mut self) -> Option<f64> {
        let now = self.now;
        let want_lens = self.settings.code_hints;
        let ai = self.tabs.active?;
        let tab = self.tabs.list.get_mut(ai)?;
        let id = tab.id;
        let f = tab.file_mut()?;
        let View::Text(doc) = &f.view else { return None };
        let lang = doc.lang();
        if !lang.has_outline() || doc.len_bytes() > hints::MAX_OUTLINE_BYTES {
            return None;
        }
        let rev = doc.revision();
        let h = &mut f.hints;
        if h.seen_rev != Some(rev) {
            h.seen_rev = Some(rev);
            h.changed_at = now;
        }
        if h.outline_rev != Some(rev) && h.computing != Some(rev) {
            let left = hints::DEBOUNCE - (now - h.changed_at);
            if h.outline_rev.is_some() && h.outline.is_some() && left > 0.0 {
                return Some(left);
            }
            h.computing = Some(rev);
            hints::spawn_outline(id, rev, lang, doc.text(), self.outline_tx.clone(), {
                let w = waker(&self.ctx);
                move || w()
            });
            return None;
        }
        if want_lens && h.outline_rev.is_some() && h.lens_sent != h.outline_rev {
            let (Some(o), Some(orev)) = (h.outline.clone(), h.outline_rev) else { return None };
            h.lens_sent = Some(orev);
            if let Some(ws) = self.ws.as_mut() {
                ws.send(Request::Lens { fns: hints::lens_request(&o) }, Pending::Lens { tab: id, rev: orev });
            }
        }
        None
    }

    fn apply_outline(&mut self, o: crate::hints::OutlineDone) {
        let Some(f) = self.tabs.file_by_id(o.tab) else { return };
        let Some(doc) = f.doc() else { return };
        let current = doc.revision() == o.rev;
        let h = &mut f.hints;
        if h.computing == Some(o.rev) {
            h.computing = None;
        }
        if !current {
            return;
        }
        let first = h.outline.is_none();
        h.outline_rev = Some(o.rev);
        h.outline = Some(o.outline.clone());
        h.lens_sent = None;
        if first {
            // Nothing on screen yet: show what the outline alone knows until the engine answers.
            h.lenses = hints::build(&o.outline, &[]);
        }
        self.ctx.request_repaint();
    }

    // ---- overlays ----

    pub(crate) fn overlays(&mut self, ctx: &egui::Context) {
        let t = self.theme;
        if self.modal.is_some() {
            self.ctx_menu = None;
            self.menubar.close();
            self.modal_ui(ctx);
            return;
        }
        if self.picker.is_some() {
            let recent = self.state.recent.clone();
            let out = self.picker.as_mut().and_then(|p| p.show(ctx, &t, self.mac, &recent, self.now));
            match out {
                Some(PickerOut::Close) => {
                    self.picker = None;
                    self.focus_editor = true;
                }
                Some(PickerOut::Browse(p)) => {
                    if let Some(pk) = self.picker.as_mut() {
                        pk.waiting = true;
                        pk.error = None;
                        if let Some(p) = &p {
                            pk.path = p.to_string_lossy().into_owned();
                        }
                    }
                    self.opener.browse(p.as_deref(), waker(ctx));
                }
                Some(PickerOut::Open(p)) => {
                    if let Some(pk) = self.picker.as_mut() {
                        pk.error = None;
                    }
                    self.open_folder(p);
                }
                None => {}
            }
            return;
        }
        if self.palette.is_some() {
            self.ctx_menu = None;
            self.menubar.close();
            let outline = self.tabs.active_file().and_then(|f| f.hints.outline.clone());
            let has_tab = self.tabs.active_file().is_some_and(|f| f.doc().is_some());
            let mru = self.mru.clone();
            let out = self.palette.as_mut().and_then(|p| p.show(ctx, &t, self.mac, &mru, outline, has_tab, self.now));
            let Some(out) = out else { return };
            self.close_palette();
            match out {
                PaletteOut::Close => {}
                PaletteOut::Run(c) => {
                    self.note_mru(c);
                    self.run_cmd(c);
                }
                PaletteOut::Open(p) => self.open_file(p, None, true),
                PaletteOut::Line(l) => self.jump_here(l, 0),
                PaletteOut::Symbol(l, c) => self.jump_here(l, c),
            }
            return;
        }
        if self.ctx_menu.is_some() {
            self.menubar.close();
            self.ctx_menu_ui(ctx);
            return;
        }
        if self.menubar.is_open() {
            let recent = self.state.recent.clone();
            let mut mb = std::mem::take(&mut self.menubar);
            let enabled = |c: Cmd| self.enabled(c);
            let pick = mb.popup(ctx, &t, self.mac, &enabled, &recent, self.frame);
            self.menubar = mb;
            match pick {
                Some(MenuPick::Cmd(c)) => {
                    self.note_mru(c);
                    self.run_cmd(c);
                }
                Some(MenuPick::Recent(p)) => self.open_folder(PathBuf::from(p)),
                None => {}
            }
        }
    }

    fn jump_here(&mut self, line: u32, col: u32) {
        let Some(f) = self.tabs.active_file_mut() else { return };
        if f.is_untitled() {
            f.st.goto(line, col);
            f.focus_editor = true;
            return;
        }
        let p = f.path.clone();
        self.open_file(p, Some((line, col, 0)), true);
    }

    fn modal_ui(&mut self, ctx: &egui::Context) {
        let t = self.theme;
        let Some(m) = self.modal.clone() else { return };
        // (title, body, buttons, button Enter picks, button Esc picks)
        let (title, body, buttons, enter_pick): (String, String, Vec<&str>, Option<usize>) = match &m {
            Modal::Unsaved { tabs, then } => {
                let names: Vec<String> = tabs.iter().filter_map(|id| self.tabs.by_id(*id)).map(|i| self.tabs.list[i].title()).collect();
                let untitled = tabs.len() == 1 && tabs.iter().any(|id| self.tabs.list.iter().any(|t| t.id == *id && t.file().is_some_and(FileTab::is_untitled)));
                let title = if names.len() == 1 { format!("Save changes to {}?", names[0]) } else { format!("Save changes to {} files?", names.len()) };
                let what = match then {
                    Then::Quit => "before quitting",
                    Then::CloseTab(_) | Then::CloseTabs(_) => "before closing",
                    Then::CloseFolder | Then::Open(_) => "before closing the folder",
                };
                if untitled {
                    (title, "This buffer is not saved to a file. Discarded text cannot be recovered.".to_owned(), vec!["Save…", "Discard", "Cancel"], Some(0))
                } else {
                    let body = if names.len() > 1 { format!("{} — {what}.", names.join(", ")) } else { format!("Unsaved edits are lost if you don't save them {what}.") };
                    (title, body, vec!["Save", "Don't Save", "Cancel"], Some(0))
                }
            }
            Modal::Delete { path, dir } => {
                let name = clean(path.name());
                let body = if *dir { "The folder must be empty. This cannot be undone.".to_owned() } else { "This cannot be undone.".to_owned() };
                (format!("Delete {name}?"), body, vec!["Delete", "Cancel"], Some(0))
            }
            Modal::Replace { hits, files, with, .. } => {
                let m = if *hits == 1 { "match".to_owned() } else { "matches".to_owned() };
                let f = if *files == 1 { "file" } else { "files" };
                let title = if with.is_empty() {
                    format!("Delete {} {m} in {} {f}?", thousands(*hits as u64), thousands(*files as u64))
                } else {
                    format!("Replace {} {m} in {} {f} with “{}”?", thousands(*hits as u64), thousands(*files as u64), sanitize(with, 40))
                };
                // Enter does not confirm: Cancel is the default.
                (title, "Files on disk are rewritten. This cannot be undone.".to_owned(), vec!["Replace", "Cancel"], Some(1))
            }
            Modal::SaveAs(_) if self.ws.is_none() => ("Save As".to_owned(), "No folder is open. Open a folder to save into it.".to_owned(), vec!["Open Folder…", "Cancel"], Some(0)),
            Modal::SaveAs(_) => ("Save As".to_owned(), "Path inside the folder.".to_owned(), vec!["Save", "Cancel"], Some(0)),
            Modal::About => {
                let body = format!("Version {}\nWindow sandbox: {}\nEngine sandbox: {}", env!("CARGO_PKG_VERSION"), sanitize(&self.gui_sandbox, 40), sanitize(self.opener.sandbox(), 40));
                ("Margin".to_owned(), body, vec!["Close"], Some(0))
            }
        };
        let save_as = match &m {
            Modal::SaveAs(s) if self.ws.is_some() => Some(s.clone()),
            _ => None,
        };
        let screen = ctx.content_rect();
        egui::Area::new(Id::unique("modal-scrim")).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, t.ui.scrim);
            ui.interact(screen, Id::unique("modal-scrim-hit"), Sense::click());
        });
        let (enter, esc) = ctx.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, Key::Enter), i.consume_key(egui::Modifiers::NONE, Key::Escape)));
        let w = 440.0f32.min(screen.width() - 32.0);
        let title_g = ctx.fonts_mut(|f| f.layout(title.clone(), egui::FontId::proportional(t.look.ui_size + 1.0), t.ui.text_strong, w - 48.0));
        let body_g = ctx.fonts_mut(|f| f.layout(body.clone(), t.font_ui(), t.ui.text, w - 48.0));
        let field_h = if save_as.is_some() { t.row_h() + 6.0 + 10.0 + t.row_h() } else { 0.0 };
        let h = 24.0 + title_g.size().y + 10.0 + body_g.size().y + field_h + 24.0 + t.row_h() + 2.0 + 20.0;
        let pos = pos2((screen.center().x - w / 2.0).round(), (screen.top() + screen.height() * 0.28).round());
        let rect = Rect::from_min_size(pos, vec2(w, h));
        let mut clicked: Option<usize> = None;
        let mut text = save_as.as_ref().map(|s| s.text.clone()).unwrap_or_default();
        let mut field_enter = false;
        let mut want_focus = save_as.as_ref().is_some_and(|s| s.focus);
        let busy = save_as.as_ref().is_some_and(|s| s.busy);
        let mut live_error: Option<String> = None;
        let mut seen = Vec::new();
        egui::Area::new(Id::unique("modal")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            let p = ui.painter().clone();
            p.add(ctx.global_style().visuals.window_shadow.as_shape(rect, 6));
            p.rect_filled(rect, 6.0, t.ui.overlay_bg);
            p.rect_stroke(rect, 6.0, t.border(), egui::StrokeKind::Inside);
            ui.interact(rect, Id::unique("modal-body"), Sense::click());
            let ty = rect.top() + 24.0;
            let tg_h = title_g.size().y;
            p.galley(pos2(rect.left() + 24.0, ty), title_g, t.ui.text_strong);
            let by = ty + tg_h + 10.0;
            let bg_h = body_g.size().y;
            p.galley(pos2(rect.left() + 24.0, by), body_g, t.ui.text_muted);
            if let Some(sa) = &save_as {
                let fr = Rect::from_min_size(pos2(rect.left() + 24.0, by + bg_h + 10.0), vec2(w - 48.0, t.row_h() + 6.0));
                let resp = widgets::field(ui, fr, save_as_id(), &mut text, "notes/idea.md", &t, t.font_mono_ui());
                if want_focus {
                    resp.request_focus();
                    widgets::cursor_to_end(ctx, save_as_id(), &text);
                    want_focus = false;
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    field_enter = true;
                }
                live_error = save_as_check(&text).err().or_else(|| if text == sa.text { sa.error.clone() } else { None });
                let er = Rect::from_min_size(pos2(fr.left(), fr.bottom() + 6.0), vec2(fr.width(), t.row_h()));
                if let Some(e) = &live_error {
                    widgets::text_at(ui, er, er.left(), e, t.font_small(), t.ui.error);
                } else if busy {
                    widgets::text_at(ui, er, er.left(), "Saving…", t.font_small(), t.ui.text_muted);
                }
            }
            let br = Rect::from_min_max(pos2(rect.left() + 24.0, rect.bottom() - 20.0 - t.row_h() - 2.0), pos2(rect.right() - 24.0, rect.bottom() - 20.0));
            let mut b = ui.new_child(egui::UiBuilder::new().max_rect(br).layout(egui::Layout::right_to_left(egui::Align::Center)));
            b.spacing_mut().item_spacing = vec2(8.0, 0.0);
            for (i, label) in buttons.iter().enumerate() {
                let resp = if i == 0 { widgets::primary_button(&mut b, label, &t) } else { widgets::quiet_button(&mut b, label, &t) };
                seen.push(((*label).to_owned(), resp.rect));
                if resp.clicked() {
                    clicked = Some(i);
                }
            }
        });
        self.modal_seen = (title.clone(), seen);
        if let (Some(Modal::SaveAs(s)), Some(_)) = (self.modal.as_mut(), &save_as) {
            if s.text != text {
                s.text = text;
                s.error = None;
            }
            s.focus = want_focus;
        }
        let enter = enter || field_enter;
        let pick = clicked.or(if enter { enter_pick } else { None }).or(if esc { Some(buttons.len() - 1) } else { None });
        let Some(pick) = pick else { return };
        match m {
            Modal::SaveAs(sa) => {
                if pick != 0 {
                    self.modal = None;
                    self.focus_editor = true;
                } else if self.ws.is_none() {
                    self.modal = None;
                    self.run_cmd(Cmd::OpenFolder);
                } else if !busy && live_error.is_none() {
                    self.save_as_submit(sa.tab);
                }
                return;
            }
            Modal::About => {
                self.modal = None;
            }
            Modal::Unsaved { tabs, then } => {
                self.modal = None;
                let untitled = tabs.len() == 1 && tabs.iter().any(|id| self.tabs.list.iter().any(|t| t.id == *id && t.file().is_some_and(FileTab::is_untitled)));
                match pick {
                    0 if untitled => {
                        if let Some(id) = tabs.first() {
                            self.ask_save_as(*id, Some(then));
                        }
                        return;
                    }
                    0 => {
                        let tabs: Vec<u64> = tabs.into_iter().filter(|id| self.tabs.list.iter().any(|t| t.id == *id && t.dirty())).collect();
                        // Read-only buffers cannot be saved: they count as done.
                        let saveable: Vec<u64> = tabs.iter().copied().filter(|id| self.tabs.list.iter().any(|t| t.id == *id && t.file().and_then(|f| f.doc()).is_some_and(|d| !d.read_only()))).collect();
                        if saveable.is_empty() {
                            self.proceed(then);
                        } else {
                            for id in &saveable {
                                self.save_tab(*id, false);
                            }
                            self.after_save = Some(AfterSave { waiting: saveable, then });
                        }
                    }
                    1 => self.proceed(then),
                    _ => {}
                }
            }
            Modal::Delete { path, .. } => {
                self.modal = None;
                if pick == 0 {
                    if let Some(ws) = self.ws.as_mut() {
                        ws.send(Request::Delete { path: path.clone() }, Pending::Deleted(path));
                    }
                }
            }
            Modal::Replace { tab, only, .. } => {
                self.modal = None;
                if pick == 0 {
                    self.send_replace(tab, only);
                }
                return;
            }
        }
        self.focus_editor = true;
    }
}

pub fn save_as_id() -> Id {
    Id::unique("save-as-path")
}

/// Path typed in Save As → a file path inside the folder.
pub fn save_as_check(text: &str) -> Result<RelPath, String> {
    let s = text.trim();
    if s.is_empty() {
        return Err("Type a path, e.g. notes/idea.md".to_owned());
    }
    if s.ends_with('/') {
        return Err("That is a folder: add a file name".to_owned());
    }
    match RelPath::new(s) {
        Ok(p) if !p.is_root() => Ok(p),
        _ => Err("Not a path inside the folder".to_owned()),
    }
}

/// The editor command behind a menu / palette command.
pub fn edit_action(c: Cmd) -> Option<EditAction> {
    Some(match c {
        Cmd::Undo => EditAction::Undo,
        Cmd::Redo => EditAction::Redo,
        Cmd::Cut => EditAction::Cut,
        Cmd::Copy => EditAction::Copy,
        Cmd::Paste => EditAction::Paste,
        Cmd::SelectAll => EditAction::SelectAll,
        Cmd::ToggleComment => EditAction::ToggleComment,
        Cmd::Indent => EditAction::Indent,
        Cmd::Outdent => EditAction::Outdent,
        Cmd::DuplicateLine => EditAction::DuplicateLine,
        Cmd::DeleteLine => EditAction::DeleteLine,
        Cmd::MoveLineUp => EditAction::MoveLineUp,
        Cmd::MoveLineDown => EditAction::MoveLineDown,
        _ => return None,
    })
}

/// Absolute path of `p` under `root` (display / clipboard only; never used for I/O).
pub fn abs_path(root: &str, p: &RelPath) -> String {
    if p.is_root() {
        return root.to_owned();
    }
    let sep = if root.contains('\\') && !root.contains('/') { '\\' } else { '/' };
    let base = root.trim_end_matches(['/', '\\']);
    let rel = if sep == '\\' { p.as_str().replace('/', "\\") } else { p.as_str().to_owned() };
    format!("{base}{sep}{rel}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abs_and_snippets() {
        assert_eq!(abs_path("/r", &RelPath::new("a/b").unwrap()), "/r/a/b");
        assert_eq!(abs_path("/r/", &RelPath::root()), "/r/");
        assert_eq!(abs_path("C:\\x", &RelPath::new("a/b").unwrap()), "C:\\x\\a\\b");
        let long = format!("{}memcpy(a){}", "é".repeat(300), "x".repeat(900));
        let col = long.find("memcpy").unwrap() as u32;
        let (t, c) = local_snippet(&long, col);
        assert!(t.len() <= MAX_SNIPPET);
        assert!(t[c as usize..].starts_with("memcpy"));
        let (t, c) = local_snippet("abc", 99);
        assert_eq!((t.as_str(), c), ("abc", 3));
    }
}
