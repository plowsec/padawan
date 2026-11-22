//! File tree: lazy listings (one `ListDir` per expanded directory), cached, flattened into rows only
//! when something changes; drawn virtualised.
//!
//! | Key (tree focused) | Does |
//! |---|---|
//! | Up / Down | move |
//! | Right | expand, or go to the first child |
//! | Left | collapse, or go to the parent |
//! | Enter | open file / toggle folder |
//! | F2 | rename |
//!
//! Right-click: `TreeAction::Menu` (the app draws the shared context menu: New File in Folder…, New Folder…,
//! Rename…, Delete…, Copy Path, Copy Relative Path, Open in New Window for folders).

use std::collections::{HashMap, HashSet};

use egui::{pos2, vec2, CursorIcon, Id, Key, Rect, Sense, Ui};
use margin_proto::limits::MAX_DIR_ENTRIES;
use margin_proto::{DirEntry, EntryKind, RelPath};
use margin_theme::Theme;

use crate::sanitize::clean;
use crate::widgets;

#[derive(Clone, Debug, Default)]
pub struct Dir {
    pub entries: Vec<DirEntry>,
    pub truncated: bool,
    pub loaded: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub path: RelPath,
    pub depth: u16,
    pub kind: EntryKind,
    pub ignored: bool,
    pub open: bool,
    /// Placeholder row holding the inline "new file/folder" field.
    pub new_entry: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditKind {
    Rename(RelPath),
    NewFile(RelPath),
    NewFolder(RelPath),
}

#[derive(Clone, Debug)]
pub struct Edit {
    pub kind: EditKind,
    pub text: String,
    fresh: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TreeAction {
    Open(RelPath),
    Load(RelPath),
    CreateFile(RelPath),
    CreateDir(RelPath),
    Rename(RelPath, RelPath),
    AskDelete(RelPath, bool),
    Copy(String),
    /// Copy the absolute path (the app knows the root).
    CopyAbs(RelPath),
    Error(String),
    /// Right-click on a row (`Some(path, is_dir)`) or on empty space (`None`), at the pointer.
    Menu(Option<(RelPath, bool)>, egui::Pos2),
}

#[derive(Default)]
pub struct Tree {
    pub dirs: HashMap<RelPath, Dir>,
    pub expanded: HashSet<RelPath>,
    pub rows: Vec<Row>,
    dirty: bool,
    pub selected: Option<RelPath>,
    reveal: Option<RelPath>,
    pub edit: Option<Edit>,
    /// Focus the tree on the next frame.
    pub want_focus: bool,
}

pub fn tree_id() -> Id {
    Id::unique("margin-tree")
}

impl Tree {
    pub fn clear(&mut self) {
        *self = Tree::default();
    }

    /// Store a listing (capped).
    pub fn set_dir(&mut self, path: RelPath, mut entries: Vec<DirEntry>, truncated: bool) {
        let truncated = truncated || entries.len() > MAX_DIR_ENTRIES;
        entries.truncate(MAX_DIR_ENTRIES);
        self.dirs.insert(path, Dir { entries, truncated, loaded: true, error: None });
        self.dirty = true;
    }

    pub fn set_error(&mut self, path: RelPath, msg: String) {
        let d = self.dirs.entry(path.clone()).or_default();
        d.loaded = true;
        d.error = Some(msg);
        if d.entries.is_empty() && !path.is_root() {
            self.expanded.remove(&path);
        }
        self.dirty = true;
    }

    pub fn is_dir(&self, p: &RelPath) -> bool {
        p.is_root() || self.entry(p).is_some_and(|e| e.kind == EntryKind::Dir)
    }

    pub fn entry(&self, p: &RelPath) -> Option<&DirEntry> {
        let parent = p.parent()?;
        self.dirs.get(&parent)?.entries.iter().find(|e| e.name == p.name())
    }

    /// Directories to re-list after focus or a file operation: root + expanded ones.
    pub fn loaded_dirs(&self) -> Vec<RelPath> {
        let mut v: Vec<RelPath> = std::iter::once(RelPath::root()).chain(self.expanded.iter().cloned()).collect();
        v.sort();
        v
    }

    /// Expand/collapse `p`; returns a directory to load.
    pub fn toggle(&mut self, p: &RelPath) -> Option<RelPath> {
        if self.expanded.remove(p) {
            self.dirty = true;
            None
        } else {
            self.expand(p)
        }
    }

    pub fn expand(&mut self, p: &RelPath) -> Option<RelPath> {
        self.expanded.insert(p.clone());
        self.dirty = true;
        (!self.dirs.get(p).is_some_and(|d| d.loaded)).then(|| p.clone())
    }

    /// Expand the parents of `p` and select it. Returns directories to load.
    pub fn reveal(&mut self, p: &RelPath) -> Vec<RelPath> {
        let mut load = Vec::new();
        let mut cur = p.parent();
        while let Some(d) = cur {
            if !d.is_root() && !self.expanded.contains(&d) {
                load.extend(self.expand(&d));
            }
            cur = d.parent();
        }
        if self.selected.as_ref() != Some(p) {
            self.selected = Some(p.clone());
            self.reveal = Some(p.clone());
        }
        load
    }

    /// Row index of `p` (tests, menus).
    pub fn row_of(&self, p: &RelPath) -> Option<usize> {
        self.index_of(p)
    }

    pub fn start_edit(&mut self, kind: EditKind) {
        let text = match &kind {
            EditKind::Rename(p) => p.name().to_owned(),
            _ => String::new(),
        };
        let parent = match &kind {
            EditKind::NewFile(d) | EditKind::NewFolder(d) => Some(d.clone()),
            EditKind::Rename(_) => None,
        };
        self.edit = Some(Edit { kind, text, fresh: true });
        if let Some(d) = parent.filter(|d| !d.is_root()) {
            self.expand(&d);
        }
        self.dirty = true;
    }

    /// Flatten (explicit stack, no recursion).
    pub fn rebuild(&mut self) {
        self.dirty = false;
        self.rows.clear();
        let new_in = match self.edit.as_ref().map(|e| &e.kind) {
            Some(EditKind::NewFile(d) | EditKind::NewFolder(d)) => Some(d.clone()),
            _ => None,
        };
        let mut stack: Vec<(RelPath, u16, usize)> = vec![(RelPath::root(), 0, 0)];
        if new_in.as_ref().is_some_and(RelPath::is_root) {
            self.rows.push(Row { path: RelPath::root(), depth: 0, kind: EntryKind::File, ignored: false, open: false, new_entry: true });
        }
        while let Some((dir, depth, i)) = stack.pop() {
            let Some(d) = self.dirs.get(&dir) else { continue };
            let Some(e) = d.entries.get(i) else { continue };
            stack.push((dir.clone(), depth, i + 1));
            let Ok(path) = dir.join(&e.name) else { continue };
            let open = e.kind == EntryKind::Dir && self.expanded.contains(&path);
            self.rows.push(Row { path: path.clone(), depth, kind: e.kind, ignored: e.ignored, open, new_entry: false });
            if open && depth < 200 {
                if new_in.as_ref() == Some(&path) {
                    self.rows.push(Row { path: path.clone(), depth: depth + 1, kind: EntryKind::File, ignored: false, open: false, new_entry: true });
                }
                stack.push((path, depth + 1, 0));
            }
        }
    }

    fn index_of(&self, p: &RelPath) -> Option<usize> {
        self.rows.iter().position(|r| !r.new_entry && &r.path == p)
    }

    fn keyboard(&mut self, ui: &Ui, out: &mut Vec<TreeAction>) {
        let n = self.rows.len();
        if n == 0 || self.edit.is_some() {
            return;
        }
        let cur = self.selected.as_ref().and_then(|p| self.index_of(p));
        let (up, down, left, right, enter, f2) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowRight),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
                i.consume_key(egui::Modifiers::NONE, Key::F2),
            )
        });
        let go = |t: &mut Tree, i: usize| {
            let i = i.min(n - 1);
            t.selected = Some(t.rows[i].path.clone());
            t.reveal = t.selected.clone();
        };
        if down {
            go(self, cur.map_or(0, |c| c + 1));
        }
        if up {
            go(self, cur.map_or(0, |c| c.saturating_sub(1)));
        }
        let Some(c) = cur else { return };
        let row = self.rows[c].clone();
        if right && row.kind == EntryKind::Dir {
            if row.open {
                if c + 1 < n && self.rows[c + 1].depth > row.depth {
                    go(self, c + 1);
                }
            } else {
                out.extend(self.expand(&row.path).map(TreeAction::Load));
            }
        }
        if left {
            if row.open {
                self.toggle(&row.path);
            } else if let Some(p) = row.path.parent().filter(|p| !p.is_root()) {
                self.selected = Some(p.clone());
                self.reveal = Some(p);
            }
        }
        if enter {
            match row.kind {
                EntryKind::Dir => out.extend(self.toggle(&row.path).map(TreeAction::Load)),
                EntryKind::File | EntryKind::Symlink => out.push(TreeAction::Open(row.path.clone())),
                EntryKind::Other => {}
            }
        }
        if f2 {
            self.start_edit(EditKind::Rename(row.path));
        }
    }

    /// Draw in `rect`. `active`: path of the active tab (highlighted).
    pub fn show(&mut self, ui: &mut Ui, rect: Rect, t: &Theme, active: Option<&RelPath>) -> Vec<TreeAction> {
        let mut out = Vec::new();
        let id = tree_id();
        let bg = ui.interact(rect, id, Sense::click());
        if self.want_focus {
            self.want_focus = false;
            ui.memory_mut(|m| m.request_focus(id));
        }
        let focused = ui.memory(|m| m.has_focus(id));
        if focused {
            ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { vertical_arrows: true, horizontal_arrows: true, tab: false, escape: false }));
            self.keyboard(ui, &mut out);
            widgets::hold_focus(ui);
        }
        if self.dirty {
            self.rebuild();
        }
        if bg.clicked() {
            ui.memory_mut(|m| m.request_focus(id));
        }
        let mut bg_menu = bg.secondary_clicked().then(|| bg.interact_pointer_pos().unwrap_or(rect.center()));

        let row_h = t.row_h();
        let reveal = self.reveal.take().and_then(|p| self.index_of(&p)).or_else(|| self.edit.as_ref().and_then(|_| self.rows.iter().position(|r| r.new_entry)));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        child.spacing_mut().item_spacing = vec2(0.0, 0.0);
        if self.rows.is_empty() {
            let msg = match self.dirs.get(&RelPath::root()) {
                Some(d) if d.loaded => d.error.as_deref().map(clean).unwrap_or_else(|| "Empty folder".into()),
                _ => "Loading…".into(),
            };
            let r = Rect::from_min_size(rect.min + vec2(0.0, 4.0), vec2(rect.width(), row_h));
            widgets::text_at(&child, r, r.left() + 16.0, &msg, t.font_ui(), t.ui.text_faint);
        }
        let n = self.rows.len();
        let mut clicked: Option<usize> = None;
        let mut menu: Option<(usize, egui::Pos2)> = None;
        widgets::virtual_rows(&mut child, Id::unique("tree-scroll"), n, row_h, reveal, |ui, i, r| {
            let row = &self.rows[i];
            let indent = 8.0 + f32::from(row.depth) * 12.0;
            if row.new_entry || matches!(&self.edit, Some(Edit { kind: EditKind::Rename(p), .. }) if *p == row.path) {
                if let Some(e) = self.edit.as_mut() {
                    let fid = Id::unique("tree-edit");
                    let fr = Rect::from_min_max(pos2(r.left() + indent + 12.0, r.top() + 2.0), pos2(r.right() - 8.0, r.bottom() - 2.0));
                    let resp = widgets::field(ui, fr, fid, &mut e.text, if row.new_entry { "name" } else { "" }, t, t.font_ui());
                    if e.fresh {
                        e.fresh = false;
                        resp.request_focus();
                        widgets::cursor_to_end(ui.ctx(), fid, &e.text);
                    }
                    if resp.lost_focus() {
                        let commit = ui.input(|i| i.key_pressed(Key::Enter));
                        let e = self.edit.take();
                        self.dirty = true;
                        if let (true, Some(e)) = (commit, e) {
                            out.extend(commit_edit(e));
                        }
                        self.want_focus = true;
                    }
                }
                return;
            }
            let resp = ui.interact(r, id.with(("row", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            let selected = active == Some(&row.path) || (focused && self.selected.as_ref() == Some(&row.path));
            widgets::row_bg(ui.painter(), r, t, resp.hovered(), selected);
            if focused && self.selected.as_ref() == Some(&row.path) {
                ui.painter().rect_stroke(r.shrink(0.5), 0.0, egui::Stroke::new(1.0, margin_theme::mix(t.ui.row_selected, t.ui.accent, 0.7)), egui::StrokeKind::Inside);
            }
            let color = match (row.kind, row.ignored) {
                (EntryKind::Other, _) | (_, true) => t.ui.text_faint,
                _ if active == Some(&row.path) => t.ui.text_strong,
                _ => t.ui.text,
            };
            if row.kind == EntryKind::Dir {
                widgets::chevron(ui.painter(), pos2(r.left() + indent + 4.0, r.center().y), row.open, t.ui.text_muted);
            }
            let x = r.left() + indent + 14.0;
            let name = clean(row.path.name());
            let w = widgets::text_at(ui, r.with_max_x(r.right() - 22.0), x, &name, t.font_ui(), color);
            if row.kind == EntryKind::Symlink {
                widgets::text_at(ui, r, x + w + 6.0, "→", t.font_small(), t.ui.text_faint);
            }
            if resp.clicked() {
                clicked = Some(i);
            }
            if resp.secondary_clicked() {
                menu = Some((i, resp.interact_pointer_pos().unwrap_or(r.center())));
            }
        });
        if let Some(i) = clicked {
            ui.memory_mut(|m| m.request_focus(id));
            let row = self.rows[i].clone();
            self.selected = Some(row.path.clone());
            match row.kind {
                EntryKind::Dir => out.extend(self.toggle(&row.path).map(TreeAction::Load)),
                EntryKind::File | EntryKind::Symlink => out.push(TreeAction::Open(row.path)),
                EntryKind::Other => {}
            }
        }
        if let Some((i, pos)) = menu {
            let row = self.rows[i].clone();
            self.selected = Some(row.path.clone());
            bg_menu = None;
            out.push(TreeAction::Menu(Some((row.path, row.kind == EntryKind::Dir)), pos));
        }
        if let Some(pos) = bg_menu {
            out.push(TreeAction::Menu(None, pos));
        }
        if self.dirty {
            ui.ctx().request_repaint();
        }
        out
    }
}

fn commit_edit(e: Edit) -> Option<TreeAction> {
    let name = e.text.trim();
    if name.is_empty() {
        return None;
    }
    let bad = || Some(TreeAction::Error(format!("Not a valid name: {}", clean(name))));
    match e.kind {
        EditKind::NewFile(d) => d.join(name).map(TreeAction::CreateFile).ok().or_else(bad),
        EditKind::NewFolder(d) => d.join(name).map(TreeAction::CreateDir).ok().or_else(bad),
        EditKind::Rename(p) => {
            if name == p.name() {
                return None;
            }
            p.parent().unwrap_or_default().join(name).map(|to| TreeAction::Rename(p, to)).ok().or_else(bad)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, kind: EntryKind) -> DirEntry {
        DirEntry { name: name.into(), kind, size: 0, ignored: false }
    }

    fn p(s: &str) -> RelPath {
        RelPath::new(s).unwrap()
    }

    #[test]
    fn flattens_expanded_dirs_in_order() {
        let mut t = Tree::default();
        t.set_dir(RelPath::root(), vec![e("src", EntryKind::Dir), e("docs", EntryKind::Dir), e("a.md", EntryKind::File)], false);
        assert_eq!(t.toggle(&p("src")), Some(p("src")));
        t.set_dir(p("src"), vec![e("lib", EntryKind::Dir), e("main.rs", EntryKind::File)], false);
        t.expand(&p("src/lib"));
        t.set_dir(p("src/lib"), vec![e("x.rs", EntryKind::File)], false);
        t.rebuild();
        let names: Vec<(&str, u16)> = t.rows.iter().map(|r| (r.path.as_str(), r.depth)).collect();
        assert_eq!(names, vec![("src", 0), ("src/lib", 1), ("src/lib/x.rs", 2), ("src/main.rs", 1), ("docs", 0), ("a.md", 0)]);
        assert_eq!(t.toggle(&p("src")), None);
        t.rebuild();
        assert_eq!(t.rows.len(), 3);
        // Reveal expands the parents again.
        let load = t.reveal(&p("src/lib/x.rs"));
        assert!(load.is_empty(), "already loaded");
        t.rebuild();
        assert_eq!(t.rows.len(), 6);
        assert!(t.is_dir(&p("src/lib")) && !t.is_dir(&p("a.md")));
    }

    #[test]
    fn caps_and_bad_names() {
        let mut t = Tree::default();
        let many: Vec<DirEntry> = (0..MAX_DIR_ENTRIES + 10).map(|i| e(&format!("f{i}"), EntryKind::File)).collect();
        t.set_dir(RelPath::root(), many, false);
        assert_eq!(t.dirs[&RelPath::root()].entries.len(), MAX_DIR_ENTRIES);
        assert!(t.dirs[&RelPath::root()].truncated);
        let bad = Edit { kind: EditKind::NewFile(RelPath::root()), text: "../x".into(), fresh: false };
        assert!(matches!(commit_edit(bad), Some(TreeAction::Error(_))));
        let ok = Edit { kind: EditKind::Rename(p("d/a")), text: "b".into(), fresh: false };
        assert_eq!(commit_edit(ok), Some(TreeAction::Rename(p("d/a"), p("d/b"))));
    }
}
