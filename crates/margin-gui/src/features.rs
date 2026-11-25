//! App features on top of `ops`: untitled buffers + hot exit, Save As, search tabs + replace, context menus,
//! Markdown preview wiring.
//!
//! | Feature | Rule |
//! |---|---|
//! | untitled | `Untitled-N`, plain text, no engine request; dirty = has text |
//! | hot exit | `<config>/untitled/untitled-N.txt`, 1 s after the last edit and on quit; ≤ 32 buffers × 4 MiB, else memory only (one toast) |
//! | Save As | `CreateDir` per parent level (`Exists` is fine), then `WriteFile { create_new }`; `Exists` → inline error |
//! | replace | confirm modal; `skip` = open tabs with unsaved edits; then reload clean tabs in `changed`, re-run the search |
//! | context menus | one at a time; same widget as the menu bar; Esc / outside click closes |

use egui::{Id, Pos2};
use margin_code::Lang;
use margin_md::MdDoc;
use margin_proto::{ErrCode, RelPath, Reply, Request, SearchQuery, Skipped};

use crate::app::{waker, App, Modal, SaveAs, Then, HOT_DELAY};
use crate::md_preview::{self, Link, Preview};
use crate::ops::abs_path;
use crate::sanitize::{clean, floor_char, sanitize};
use crate::search_panel::{thousands, SearchOut, SearchPanel};
use crate::state::{MAX_UNTITLED, MAX_UNTITLED_BYTES};
use crate::tabs::{FileTab, TabKind, View};
use crate::tree::EditKind;
use crate::widgets::{self, MenuEntry};
use crate::workspace::Pending;

/// What a context-menu entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum CtxAct {
    Copy(String),
    /// Copy the absolute path of a folder entry.
    CopyAbs(RelPath),
    NewFileIn(RelPath),
    NewFolderIn(RelPath),
    Rename(RelPath),
    Delete(RelPath, bool),
    NewWindow(RelPath),
    Open(RelPath),
    /// Tab `.1` of group `.0`.
    CloseTab(u32, u64),
    CloseOthers(u32, u64),
    /// Every tab of the group.
    CloseAll(u32),
    /// Focus tab `.1` of group `.0`, then run the command.
    TabCmd(u32, u64, crate::commands::Cmd),
    /// Copy the selection of the Markdown preview of tab.
    MdCopy(u64),
    MdSelectAll(u64),
}

pub struct CtxMenu {
    pub pos: Pos2,
    pub items: Vec<(MenuEntry, Option<CtxAct>)>,
    pub sel: Option<usize>,
    pub rect: Option<egui::Rect>,
    opened_frame: u64,
}

pub fn ctx_menu_id() -> Id {
    Id::unique("context-menu")
}

fn item(label: &str, act: CtxAct) -> (MenuEntry, Option<CtxAct>) {
    (MenuEntry::item(label, "", true), Some(act))
}

fn sep() -> (MenuEntry, Option<CtxAct>) {
    (MenuEntry::sep(), None)
}

impl App {
    // ---- untitled + hot exit ----

    /// Start-up: hot-exit buffers → tabs (not in a fresh window).
    pub(crate) fn restore_untitled(&mut self) {
        let max_on_disk = self.persist.untitled_numbers().last().copied().unwrap_or(0);
        self.untitled_next = max_on_disk.saturating_add(1).max(1);
        if self.fresh {
            return;
        }
        for (n, text, _ok) in self.persist.load_untitled() {
            let mut f = FileTab::untitled(n, &text);
            let rev = f.doc().map(|d| d.revision());
            f.hot.written = rev;
            f.hot.seen_rev = rev;
            f.focus_editor = false;
            self.tabs.add(TabKind::File(Box::new(f)));
        }
    }

    /// `Untitled-N` tab (works without a folder).
    pub(crate) fn new_untitled(&mut self) {
        let n = self.untitled_next;
        self.untitled_next = n.saturating_add(1);
        let mut f = FileTab::untitled(n, "");
        f.hot.written = f.doc().map(|d| d.revision());
        f.focus_editor = true;
        self.tabs.add(TabKind::File(Box::new(f)));
        self.focus_editor = true;
    }

    /// Write changed untitled buffers (debounced; `force`: now). Returns the wait until the next write is due.
    pub(crate) fn hot_exit(&mut self, force: bool) -> Option<f64> {
        let now = self.now;
        let mut wait: Option<f64> = None;
        let mut kept = 0usize;
        let mut warn = None;
        for t in &mut self.tabs.list {
            let Some(f) = t.file_mut() else { continue };
            let Some(n) = f.untitled else { continue };
            let View::Text(d) = &f.view else { continue };
            let rev = d.revision();
            let len = d.len_bytes();
            if len > 0 {
                kept += 1;
            }
            if f.hot.seen_rev != Some(rev) {
                f.hot.seen_rev = Some(rev);
                f.hot.changed_at = now;
            }
            if f.hot.written == Some(rev) {
                continue;
            }
            let left = HOT_DELAY - (now - f.hot.changed_at);
            if !force && left > 0.0 {
                wait = Some(wait.map_or(left, |w: f64| w.min(left)));
                continue;
            }
            f.hot.written = Some(rev);
            if len == 0 {
                self.persist.remove_untitled(n);
            } else if len > MAX_UNTITLED_BYTES || kept > MAX_UNTITLED {
                if !f.hot.memory_only {
                    f.hot.memory_only = true;
                    warn = Some(f.title());
                }
                self.persist.remove_untitled(n);
            } else {
                f.hot.memory_only = false;
                self.persist.save_untitled(n, d.text());
            }
        }
        if let Some(name) = warn {
            self.toasts.notice(&format!("{name} is kept in memory only: too large or too many unsaved buffers"));
        }
        wait
    }

    // ---- Save As ----

    pub(crate) fn ask_save_as(&mut self, tab: u64, then: Option<Then>) {
        let Some(f) = self.tabs.file_by_id(tab) else { return };
        if f.doc().is_none() {
            return;
        }
        let text = if f.is_untitled() { String::new() } else { f.path.to_string() };
        self.palette = None;
        self.ctx_menu = None;
        self.modal = Some(Modal::SaveAs(SaveAs { tab, text, error: None, busy: false, then, focus: true }));
    }

    fn save_as_mut(&mut self, tab: u64) -> Option<&mut SaveAs> {
        match self.modal.as_mut() {
            Some(Modal::SaveAs(s)) if s.tab == tab => Some(s),
            _ => None,
        }
    }

    fn save_as_fail(&mut self, tab: u64, msg: String) {
        if let Some(s) = self.save_as_mut(tab) {
            s.busy = false;
            s.error = Some(msg);
        }
    }

    /// Enter / Save in the dialog (path already checked).
    pub(crate) fn save_as_submit(&mut self, tab: u64) {
        let Some(text) = self.save_as_mut(tab).map(|s| s.text.clone()) else { return };
        let path = match crate::ops::save_as_check(&text) {
            Ok(p) => p,
            Err(e) => return self.save_as_fail(tab, e),
        };
        if self.tabs.find_file(&path).is_some_and(|i| self.tabs.list[i].id != tab) {
            return self.save_as_fail(tab, "Open in another tab".to_owned());
        }
        if let Some(s) = self.save_as_mut(tab) {
            s.busy = true;
            s.error = None;
        }
        // Every parent level, top down.
        let mut dirs = Vec::new();
        let mut p = path.parent();
        while let Some(d) = p.filter(|d| !d.is_root()) {
            p = d.parent();
            dirs.push(d);
            if dirs.len() > 64 {
                return self.save_as_fail(tab, "Too deep".to_owned());
            }
        }
        dirs.reverse();
        self.save_as_next(tab, path, dirs);
    }

    fn save_as_next(&mut self, tab: u64, path: RelPath, mut rest: Vec<RelPath>) {
        if rest.is_empty() {
            let Some((data, rev)) = self.tabs.file_by_id(tab).and_then(|f| f.doc().map(|d| (d.text().into_bytes(), d.revision()))) else { return };
            if let Some(ws) = self.ws.as_mut() {
                ws.send(Request::WriteFile { path: path.clone(), data, expect_mtime_ns: None, create_new: true }, Pending::SaveAs { tab, path, rev });
            }
            return;
        }
        let d = rest.remove(0);
        if let Some(ws) = self.ws.as_mut() {
            ws.send(Request::CreateDir { path: d }, Pending::SaveAsDir { tab, path, rest });
        }
    }

    pub(crate) fn on_save_as_dir(&mut self, tab: u64, path: RelPath, rest: Vec<RelPath>, r: Reply) {
        match r {
            Reply::Done | Reply::Err { code: ErrCode::Exists, .. } => self.save_as_next(tab, path, rest),
            Reply::Err { msg, .. } => self.save_as_fail(tab, sanitize(&msg, 160)),
            _ => self.save_as_fail(tab, "Unexpected reply".to_owned()),
        }
    }

    pub(crate) fn on_save_as(&mut self, tab: u64, path: RelPath, rev: u64, r: Reply) {
        match r {
            Reply::Meta(m) => {
                let then = self.save_as_mut(tab).and_then(|s| s.then.take());
                if matches!(self.modal, Some(Modal::SaveAs(ref s)) if s.tab == tab) {
                    self.modal = None;
                }
                let md = self.settings.md_preview;
                let mut hot = None;
                if let Some(f) = self.tabs.file_by_id(tab) {
                    hot = f.untitled.take();
                    f.path = path.clone();
                    f.meta = Some(m);
                    f.banner = None;
                    f.hints = Default::default();
                    if let View::Text(d) = &mut f.view {
                        if d.revision() == rev {
                            d.mark_saved();
                        }
                        d.set_lang(Lang::from_path(path.as_str()));
                    }
                    if !f.markdown() {
                        f.preview = None;
                    } else if f.preview.is_none() {
                        f.preview = Some(Box::new(Preview { on: md, ..Default::default() }));
                        for a in &mut f.alts {
                            a.pv.on = md;
                        }
                    }
                    f.focus_editor = true;
                }
                if let Some(n) = hot {
                    self.persist.remove_untitled(n);
                }
                let parent = path.parent().unwrap_or_default();
                for d in self.tree.reveal(&path) {
                    self.list_dir(d);
                }
                self.list_dir(parent);
                self.tabs_changed = true;
                if let Some(t) = then {
                    self.proceed(t);
                }
            }
            Reply::Err { code: ErrCode::Exists, .. } => self.save_as_fail(tab, format!("{} already exists", clean(path.name()))),
            Reply::Err { msg, .. } => self.save_as_fail(tab, sanitize(&msg, 160)),
            _ => self.save_as_fail(tab, "Unexpected reply".to_owned()),
        }
    }

    // ---- search tabs ----

    /// New search tab: `q` runs at once; else seeded from the editor selection, focused (`replace`: replace row shown).
    pub(crate) fn open_search_tab(&mut self, q: Option<SearchQuery>, replace: bool) {
        if self.ws.is_none() {
            return;
        }
        let seed = self.selection_seed();
        let id = self.tabs.next_id();
        let mut s = SearchPanel::new(id);
        s.show_replace = replace;
        match &q {
            Some(q) => {
                s.query = q.pattern.clone();
                s.case = q.case_sensitive;
                s.word = q.whole_word;
                s.regex = q.regex;
                s.include = q.include.clone();
                s.exclude = q.exclude.clone();
                if replace {
                    s.focus_replace = true;
                }
            }
            None => {
                s.query = seed.unwrap_or_default();
                if replace && !s.query.is_empty() {
                    s.focus_replace = true;
                } else {
                    s.focus_query = true;
                }
                if s.make_query().is_some() {
                    s.edited_at = Some(self.now - 1.0);
                }
            }
        }
        self.tabs.add(TabKind::Search(Box::new(s)));
        if let Some(q) = q {
            self.start_search(id, q);
        }
        self.tabs_changed = true;
    }

    fn selection_seed(&self) -> Option<String> {
        let f = self.tabs.active_file()?;
        let s = f.st.selected_text(f.doc()?);
        (!s.is_empty() && !s.contains('\n') && s.len() <= 200).then_some(s)
    }

    pub(crate) fn search_out(&mut self, tab: u64, out: SearchOut) {
        if let Some(q) = out.send {
            self.start_search(tab, q);
        }
        if let Some(h) = out.open {
            self.open_file(h.path, Some((h.line, h.col, h.len)), true);
        }
        if let Some(only) = out.replace {
            let Some(s) = self.tabs.search_by_id(tab) else { return };
            if s.last.is_none() || s.running {
                return;
            }
            let (hits, files) = match &only {
                Some(p) => (s.hits_in(p), 1),
                None => (s.hits.len(), s.files()),
            };
            if hits == 0 {
                return;
            }
            let with = s.replace.clone();
            self.modal = Some(Modal::Replace { tab, only, hits, files, with });
        }
        if let Some((line, path, pos)) = out.menu {
            self.open_hit_menu(line, path, pos);
        }
    }

    /// Confirmed: send the replace for search tab `tab`.
    pub(crate) fn send_replace(&mut self, tab: u64, only: Option<RelPath>) {
        let skip: Vec<RelPath> = self.tabs.list.iter().filter_map(|t| t.file()).filter(|f| !f.is_untitled() && f.dirty()).map(|f| f.path.clone()).collect();
        let Some(s) = self.tabs.search_by_id(tab) else { return };
        let Some(query) = s.last.clone() else { return };
        let replacement = s.replace.clone();
        s.note = None;
        if let Some(ws) = self.ws.as_mut() {
            ws.send(Request::Replace { query, replacement, only: only.into_iter().collect(), skip }, Pending::Replace(tab));
        }
    }

    pub(crate) fn on_replaced(&mut self, tab: u64, files: u32, replacements: u32, changed: Vec<RelPath>, skipped: Vec<Skipped>, cancelled: bool) {
        let mut msg = format!("Replaced {} in {} {}", thousands(u64::from(replacements)), thousands(u64::from(files)), if files == 1 { "file" } else { "files" });
        if !skipped.is_empty() {
            msg.push_str(&format!(" · {} skipped", thousands(skipped.len() as u64)));
        }
        if cancelled {
            msg.push_str(" · stopped");
        }
        self.toasts.notice(&msg);
        // Reload open tabs without edits that were rewritten.
        let ids: Vec<u64> = self.tabs.list.iter().filter(|t| t.file().is_some_and(|f| !f.is_untitled() && !f.dirty() && changed.contains(&f.path))).map(|t| t.id).collect();
        for id in ids {
            self.reload_tab(id);
        }
        let note = (!skipped.is_empty()).then(|| {
            let mut parts: Vec<String> = skipped.iter().take(3).map(|s| format!("{}: {}", clean(s.path.as_str()), sanitize(&s.why, 60))).collect();
            if skipped.len() > 3 {
                parts.push(format!("+{} more", skipped.len() - 3));
            }
            format!("Skipped {}", sanitize(&parts.join(" · "), 300))
        });
        let q = self.tabs.search_by_id(tab).and_then(|s| {
            s.note = note;
            s.last.clone()
        });
        if let Some(q) = q {
            self.start_search(tab, q);
        }
    }

    // ---- context menus ----

    fn open_ctx(&mut self, pos: Pos2, items: Vec<(MenuEntry, Option<CtxAct>)>) {
        self.menubar.close();
        self.ctx_menu = Some(CtxMenu { pos, items, sel: None, rect: None, opened_frame: self.frame });
    }

    pub(crate) fn open_tree_menu(&mut self, row: Option<(RelPath, bool)>, pos: Pos2) {
        if self.ws.is_none() {
            return;
        }
        let mut items = Vec::new();
        match row {
            Some((p, dir)) => {
                let base = if dir { p.clone() } else { p.parent().unwrap_or_default() };
                items.push(item("New File in Folder…", CtxAct::NewFileIn(base.clone())));
                items.push(item("New Folder…", CtxAct::NewFolderIn(base)));
                items.push(sep());
                items.push(item("Rename", CtxAct::Rename(p.clone())));
                items.push(item("Delete", CtxAct::Delete(p.clone(), dir)));
                items.push(sep());
                items.push(item("Copy Path", CtxAct::CopyAbs(p.clone())));
                items.push(item("Copy Relative Path", CtxAct::Copy(p.to_string())));
                if dir {
                    items.push(sep());
                    items.push(item("Open in New Window", CtxAct::NewWindow(p)));
                }
            }
            None => {
                items.push(item("New File in Folder…", CtxAct::NewFileIn(RelPath::root())));
                items.push(item("New Folder…", CtxAct::NewFolderIn(RelPath::root())));
            }
        }
        self.open_ctx(pos, items);
    }

    /// Right-click on tab `id` in the strip of group `group`.
    pub(crate) fn open_tab_menu(&mut self, group: u32, id: u64, pos: Pos2) {
        use crate::commands::Cmd;
        let tab = self.tabs.by_id(id).map(|i| &self.tabs.list[i]);
        let is_file = tab.is_some_and(|t| t.file().is_some());
        let path = tab.and_then(|t| t.file()).filter(|f| !f.is_untitled()).map(|f| f.path.clone());
        let in_group = self.tabs.group_index(group).map_or(0, |g| self.tabs.groups[g].tabs.len());
        let groups = self.tabs.groups.len();
        let room = groups < crate::tabs::MAX_GROUPS;
        let key = |c: Cmd| crate::commands::shortcut(c, self.mac);
        let cmd = |label: &str, c: Cmd, on: bool| (MenuEntry::item(label, key(c), on), Some(CtxAct::TabCmd(group, id, c)));
        let mut items = vec![item("Close", CtxAct::CloseTab(group, id)), (MenuEntry::item("Close Others", "", in_group > 1), Some(CtxAct::CloseOthers(group, id))), item("Close All", CtxAct::CloseAll(group))];
        items.push(sep());
        items.push(cmd("Split Right", Cmd::SplitRight, is_file && room));
        items.push(cmd("Split Down", Cmd::SplitDown, is_file && room));
        let movable = groups > 1 || (in_group > 1 && room);
        items.push(cmd("Move to Next Group", Cmd::MoveTabRight, movable));
        items.push(cmd("Move to Previous Group", Cmd::MoveTabLeft, movable));
        if let Some(p) = path {
            items.push(sep());
            items.push(item("Copy Path", CtxAct::CopyAbs(p.clone())));
            items.push(item("Copy Relative Path", CtxAct::Copy(p.to_string())));
        }
        self.open_ctx(pos, items);
    }

    /// Right-click on a sidebar list row.
    pub(crate) fn open_path_menu(&mut self, p: RelPath, pos: Pos2) {
        let items = vec![item("Open", CtxAct::Open(p.clone())), sep(), item("Copy Path", CtxAct::CopyAbs(p.clone())), item("Copy Relative Path", CtxAct::Copy(p.to_string()))];
        self.open_ctx(pos, items);
    }

    /// Right-click on a search hit.
    pub(crate) fn open_hit_menu(&mut self, line: String, p: RelPath, pos: Pos2) {
        let items = vec![item("Copy Line", CtxAct::Copy(line)), item("Copy Path", CtxAct::CopyAbs(p.clone())), item("Copy Relative Path", CtxAct::Copy(p.to_string()))];
        self.open_ctx(pos, items);
    }

    /// Right-click on the Markdown preview of `tab`; `link`: the destination under the pointer.
    pub(crate) fn open_md_menu(&mut self, tab: u64, pos: Pos2, link: Option<String>) {
        let has_sel = self.tabs.file_by_id(tab).and_then(|f| f.preview.as_ref()).is_some_and(|p| p.doc.as_ref().is_some_and(|d| p.state.has_selection(d)));
        let key = |c: crate::commands::Cmd| crate::commands::shortcut(c, self.mac);
        let mut items = vec![(MenuEntry::item("Copy", key(crate::commands::Cmd::Copy), has_sel), Some(CtxAct::MdCopy(tab)))];
        if let Some(l) = link {
            items.push(item("Copy Link", CtxAct::Copy(l)));
        }
        items.push(sep());
        items.push((MenuEntry::item("Select All", key(crate::commands::Cmd::SelectAll), true), Some(CtxAct::MdSelectAll(tab))));
        self.open_ctx(pos, items);
    }

    /// Copy / Select All on the preview of `tab`. `false`: no preview there.
    pub(crate) fn md_edit(&mut self, tab: u64, all: bool) -> bool {
        let Some(pv) = self.tabs.file_by_id(tab).and_then(|f| f.preview.as_mut()) else { return false };
        let Some(d) = pv.doc.clone() else { return false };
        pv.active = true;
        pv.state.request_focus();
        if all {
            pv.state.select_all(&d);
        } else {
            let s = pv.state.selected_text(&d);
            if !s.is_empty() {
                self.ctx.copy_text(s);
            }
        }
        self.ctx.request_repaint();
        true
    }

    pub(crate) fn ctx_menu_ui(&mut self, ctx: &egui::Context) {
        let t = self.theme;
        let Some(m) = self.ctx_menu.as_mut() else { return };
        let entries: Vec<MenuEntry> = m.items.iter().map(|(e, _)| e.clone()).collect();
        let [up, down, _left, _right, enter, esc] = widgets::menu_keys(ctx);
        if down {
            m.sel = widgets::menu_step(&entries, m.sel, 1);
        }
        if up {
            m.sel = widgets::menu_step(&entries, m.sel, -1);
        }
        let resp = widgets::menu_popup(ctx, ctx_menu_id(), m.pos, &entries, m.sel, &t);
        m.rect = Some(resp.rect);
        if let Some(h) = resp.hovered.filter(|h| entries.get(*h).is_some_and(|e| e.enabled)) {
            m.sel = Some(h);
        }
        let mut pick = resp.clicked;
        if enter && pick.is_none() {
            pick = m.sel;
        }
        let outside = self.frame > m.opened_frame && ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !resp.rect.contains(p)));
        if esc || outside {
            self.ctx_menu = None;
            return;
        }
        let Some(i) = pick else { return };
        let act = m.items.get(i).filter(|(e, _)| e.enabled && !e.sep).and_then(|(_, a)| a.clone());
        self.ctx_menu = None;
        if let Some(a) = act {
            self.ctx_act(a);
        }
    }

    fn ctx_act(&mut self, a: CtxAct) {
        match a {
            CtxAct::Copy(s) => self.ctx.copy_text(s),
            CtxAct::CopyAbs(p) => {
                if let Some(ws) = &self.ws {
                    let s = abs_path(&ws.root_str(), &p);
                    self.ctx.copy_text(s);
                }
            }
            CtxAct::NewFileIn(d) => self.tree.start_edit(EditKind::NewFile(d)),
            CtxAct::NewFolderIn(d) => self.tree.start_edit(EditKind::NewFolder(d)),
            CtxAct::Rename(p) => self.tree.start_edit(EditKind::Rename(p)),
            CtxAct::Delete(path, dir) => self.modal = Some(Modal::Delete { path, dir }),
            CtxAct::NewWindow(p) => {
                if let Some(ws) = &self.ws {
                    let abs = std::path::PathBuf::from(abs_path(&ws.root_str(), &p));
                    self.opener.new_window(Some(&abs));
                }
            }
            CtxAct::Open(p) => self.open_file(p, None, true),
            CtxAct::CloseTab(group, id) => self.close_view(group, id),
            CtxAct::CloseOthers(group, keep) => {
                let ids: Vec<u64> = self.tabs.group_index(group).map(|g| self.tabs.groups[g].tabs.iter().copied().filter(|x| *x != keep).collect()).unwrap_or_default();
                self.close_many(group, ids);
            }
            CtxAct::CloseAll(group) => {
                let ids: Vec<u64> = self.tabs.group_index(group).map(|g| self.tabs.groups[g].tabs.clone()).unwrap_or_default();
                self.close_many(group, ids);
            }
            CtxAct::TabCmd(group, id, c) => {
                if let Some(g) = self.tabs.group_index(group) {
                    self.tabs.activate_in(g, id);
                    self.run_cmd(c);
                }
            }
            CtxAct::MdCopy(tab) => {
                self.md_edit(tab, false);
            }
            CtxAct::MdSelectAll(tab) => {
                self.md_edit(tab, true);
            }
        }
    }

    /// Close tabs of group `group`: clean ones (and ones open in another group too) now; unsaved files → one
    /// prompt; untitled buffers with text stay open.
    fn close_many(&mut self, group: u32, ids: Vec<u64>) {
        let mut kept_untitled = 0;
        let mut dirty = Vec::new();
        for id in ids {
            let Some(i) = self.tabs.by_id(id) else { continue };
            let tab = &self.tabs.list[i];
            if self.tabs.views(id) > 1 {
                self.close_view(group, id);
            } else if !tab.dirty() {
                self.close_tab_now(id);
            } else if tab.file().is_some_and(FileTab::is_untitled) {
                kept_untitled += 1;
            } else {
                dirty.push(id);
            }
        }
        if kept_untitled > 0 {
            self.toasts.notice(&format!("{kept_untitled} unsaved untitled {} kept open", if kept_untitled == 1 { "buffer" } else { "buffers" }));
        }
        if !dirty.is_empty() {
            self.modal = Some(Modal::Unsaved { tabs: dirty.clone(), then: Then::CloseTabs(dirty) });
        }
    }

    // ---- Markdown preview ----

    pub(crate) fn toggle_preview(&mut self) {
        let Some(f) = self.tabs.active_file_mut() else { return };
        if f.doc().is_none() || !f.markdown() {
            return;
        }
        let pv = f.preview.get_or_insert_with(Default::default);
        pv.on = !pv.on;
        pv.show_editor = false;
        f.focus_editor = true;
    }

    /// Parse the previews on screen when due. Returns the wait until the next parse.
    pub(crate) fn update_preview(&mut self) -> Option<f64> {
        let mut wait: Option<f64> = None;
        for id in self.tabs.visible() {
            if let Some(w) = self.update_preview_of(id) {
                wait = Some(wait.map_or(w, |x| x.min(w)));
            }
        }
        wait
    }

    fn update_preview_of(&mut self, tab: u64) -> Option<f64> {
        let now = self.now;
        let tx = self.md_tx.clone();
        let wake = waker(&self.ctx);
        let f = self.tabs.file_by_id(tab)?;
        if !f.preview_wanted() {
            return None;
        }
        let View::Text(d) = &f.view else { return None };
        let pv = f.preview.as_mut()?;
        let rev = d.revision();
        let (go, wait) = pv.due(rev, now);
        if !go {
            return wait;
        }
        let mut text = d.text();
        if text.len() > margin_md::MAX_SOURCE {
            let cut = floor_char(&text, margin_md::MAX_SOURCE);
            text.truncate(cut);
        }
        if text.len() > md_preview::BIG {
            pv.parsing = Some(rev);
            md_preview::spawn_parse(tab, rev, text, tx, move || wake());
        } else {
            let doc = std::sync::Arc::new(margin_md::parse(&text));
            pv.apply(rev, doc);
            self.ctx.request_repaint();
        }
        None
    }

    pub(crate) fn md_link(&mut self, tab: u64, dest: &str) {
        let Some(f) = self.tabs.file_by_id(tab) else { return };
        let dir = (!f.is_untitled()).then(|| f.path.parent().unwrap_or_default());
        match md_preview::resolve(dir.as_ref(), dest) {
            Link::Anchor(frag) => self.jump_anchor(tab, &frag),
            Link::File(p, frag) => {
                self.open_file(p.clone(), None, true);
                let md = Lang::from_path(p.as_str()) == Lang::Markdown;
                if let (Some(frag), true) = (frag, md) {
                    let Some(i) = self.tabs.find_file(&p) else { return };
                    let id = self.tabs.list[i].id;
                    let loaded = self.tabs.list[i].file().is_some_and(|f| f.doc().is_some());
                    if loaded {
                        self.jump_anchor(id, &frag);
                    } else if let Some(f) = self.tabs.file_by_id(id) {
                        f.pending_anchor = Some(frag.chars().take(200).collect());
                    }
                }
            }
            Link::External(s) => {
                self.ctx.copy_text(s);
                self.toasts.notice("Link copied. Margin does not open URLs.");
            }
            Link::Bad => self.toasts.notice("Link is not a file in this folder"),
        }
    }

    /// Cursor + preview scroll to the heading `frag`.
    pub(crate) fn jump_anchor(&mut self, tab: u64, frag: &str) {
        let Some(f) = self.tabs.file_by_id(tab) else { return };
        let Some(d) = f.doc() else { return };
        let parsed: Option<std::sync::Arc<MdDoc>> = f.preview.as_ref().and_then(|p| p.doc.clone());
        let doc = match parsed {
            Some(p) => p,
            None if d.len_bytes() <= md_preview::BIG => std::sync::Arc::new(margin_md::parse(&d.text())),
            None => return,
        };
        let lines = d.line_count();
        match doc.anchor_line(frag).filter(|l| *l < lines) {
            Some(l) => {
                f.st.goto(l, 0);
                f.st.set_top_line(l.saturating_sub(2));
                f.focus_editor = true;
                if let Some(pv) = f.preview.as_mut() {
                    pv.state.scroll_to_line(l);
                }
            }
            None => self.toasts.notice(&format!("No heading #{}", sanitize(frag, 60))),
        }
    }

    /// Task box on source line `line` clicked: flip it (one undo step).
    pub(crate) fn md_task(&mut self, tab: u64, line: u32) {
        let Some(f) = self.tabs.file_by_id(tab) else { return };
        let View::Text(d) = &mut f.view else { return };
        if d.read_only() || line >= d.line_count() {
            return;
        }
        let Some((col, new)) = md_preview::task_flip(d.line(line)) else { return };
        d.replace((line, col), (line, col + 1), new);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menus_have_actions() {
        let it = item("Copy Path", CtxAct::Copy("a".into()));
        assert!(it.0.enabled && it.1.is_some());
        assert!(sep().0.sep && sep().1.is_none());
    }
}
