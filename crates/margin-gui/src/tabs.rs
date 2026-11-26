//! Open tabs (file: document + view state + disk metadata; Settings; search), editor groups, the tab strip.
//!
//! | Item | Rule |
//! |---|---|
//! | `Tabs::list` | open buffers, one per file / Settings / search tab |
//! | `Group` | one pane: tab ids in strip order + its active tab. At least one group; an empty one is dropped unless it is the last |
//! | `Node` | how the groups share the editor area: split right / down, nested, with weights |
//! | file in several groups | one buffer; cursor, scroll, find bar, preview state per group (`FileTab::st` = the view of `owner`, the others in `alts`) |
//! | `Tabs::sync` | makes the active group's view the primary one: code that reads `active_file().st` gets the right view |
//! | Settings / search tab | lives in one group; can be moved, not split |
//! | drag a tab | onto a strip: move / reorder there; onto a group's middle: move into it; onto an edge: new group on that side |
//!
//! Strip: flat; active tab = editor background + 2 px line on top (accent in the focused group); dirty = dot in the
//! close slot; × on hover; middle-click closes; right-click: context menu; same names get a muted parent-directory
//! suffix; scrolls horizontally. Untitled tabs (`Untitled-N`) have no path (`RelPath::root()`) until saved.

use egui::{pos2, vec2, CursorIcon, Id, Rect, Sense, Ui};
use margin_editor::{Document, EditorState, Lens};
use margin_proto::{Meta, RelPath, ReqId};
use margin_theme::Theme;

use crate::md_preview::{Preview, PvView};
use crate::sanitize::clean;
use crate::search_panel::SearchPanel;
use crate::widgets;

/// Groups open at once at most.
pub const MAX_GROUPS: usize = 8;
/// A group is never dragged narrower / lower than this.
pub const GROUP_MIN: f32 = 120.0;

pub enum View {
    Loading,
    Text(Box<Document>),
    Binary(u64),
    Message(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Banner {
    /// Save refused: the file changed on disk since we read it.
    Conflict,
    /// Changed on disk while we have unsaved edits.
    ChangedDirty,
}

/// Outline + hint lines of one tab.
#[derive(Default)]
pub struct HintState {
    /// Revision the outline was computed from.
    pub outline_rev: Option<u64>,
    pub outline: Option<std::sync::Arc<margin_code::Outline>>,
    /// Revision of an outline job in flight.
    pub computing: Option<u64>,
    /// When the revision last changed (egui time), for the debounce.
    pub changed_at: f64,
    pub seen_rev: Option<u64>,
    pub lenses: Vec<Lens>,
    /// Outline revision a `Lens` request went out for (`None`: ask again).
    pub lens_sent: Option<u64>,
}

/// View of a file in a group other than `FileTab::owner`.
pub struct AltView {
    pub group: u32,
    pub st: EditorState,
    pub pv: PvView,
    pub focus_editor: bool,
}

pub struct FileTab {
    pub path: RelPath,
    pub view: View,
    /// View state of group `owner`.
    pub st: EditorState,
    pub meta: Option<Meta>,
    pub banner: Option<Banner>,
    /// Not valid UTF-8: shown lossy, read-only.
    pub bad_utf8: bool,
    pub saving: Option<ReqId>,
    pub hints: HintState,
    /// Cursor/selection to apply when the content arrives.
    pub pending_select: Option<(u32, u32, u32)>,
    pub focus_editor: bool,
    /// `Some(n)`: `Untitled-n`, not on disk; `path` is the root.
    pub untitled: Option<u32>,
    /// Untitled buffer saved outside any folder: the absolute path it was written to. `cmd+s` writes there again.
    pub external: Option<String>,
    pub hot: HotExit,
    pub preview: Option<Box<Preview>>,
    /// `#fragment` to jump to once a Markdown file loads.
    pub pending_anchor: Option<String>,
    /// Group whose view is in `st`, `focus_editor` and the view part of `preview` (0: none yet).
    pub owner: u32,
    /// The views in the other groups showing this file.
    pub alts: Vec<AltView>,
}

/// Hot-exit bookkeeping of an untitled tab.
#[derive(Default)]
pub struct HotExit {
    /// Revision written to `<config>/untitled/` (or found there at start).
    pub written: Option<u64>,
    pub seen_rev: Option<u64>,
    pub changed_at: f64,
    /// Too large / too many: kept in memory only (the user was told once).
    pub memory_only: bool,
}

impl FileTab {
    pub fn new(path: RelPath) -> FileTab {
        FileTab {
            path,
            view: View::Loading,
            st: EditorState::default(),
            meta: None,
            banner: None,
            bad_utf8: false,
            saving: None,
            hints: HintState::default(),
            pending_select: None,
            focus_editor: true,
            untitled: None,
            external: None,
            hot: HotExit::default(),
            preview: None,
            pending_anchor: None,
            owner: 0,
            alts: Vec::new(),
        }
    }

    /// `Untitled-n` with `text` (plain text until saved).
    pub fn untitled(n: u32, text: &str) -> FileTab {
        let mut f = FileTab::new(RelPath::root());
        f.untitled = Some(n);
        f.view = View::Text(Box::new(Document::new(text, margin_code::Lang::Plain)));
        f
    }

    pub fn is_untitled(&self) -> bool {
        self.untitled.is_some()
    }

    /// Untitled and never saved anywhere: saving it means asking for a path.
    pub fn unsaved_buffer(&self) -> bool {
        self.untitled.is_some() && self.external.is_none()
    }

    pub fn doc(&self) -> Option<&Document> {
        match &self.view {
            View::Text(d) => Some(d),
            _ => None,
        }
    }

    /// Untitled: has text. File (in the folder, or saved outside one): edited since saved.
    pub fn dirty(&self) -> bool {
        match self.untitled {
            Some(_) if self.external.is_none() => self.doc().is_some_and(|d| d.len_bytes() > 0),
            _ => self.doc().is_some_and(Document::is_dirty),
        }
    }

    pub fn title(&self) -> String {
        match (&self.external, self.untitled) {
            (Some(p), _) => clean(p.rsplit(['/', '\\']).next().unwrap_or(p)),
            (None, Some(n)) => format!("Untitled-{n}"),
            (None, None) => clean(self.path.name()),
        }
    }

    /// Markdown, or an untitled buffer (previewable as Markdown).
    pub fn markdown(&self) -> bool {
        self.is_untitled() || self.doc().is_some_and(|d| d.lang() == margin_code::Lang::Markdown)
    }

    /// A view in any group has the Markdown preview on.
    pub fn preview_wanted(&self) -> bool {
        self.preview.as_ref().is_some_and(|p| p.on) || (self.preview.is_some() && self.alts.iter().any(|a| a.pv.on))
    }

    /// Put the view of `group` into `st` (a new view starts where the current one is).
    pub fn make_primary(&mut self, group: u32) {
        if self.owner == group {
            return;
        }
        if self.owner == 0 {
            self.owner = group;
            return;
        }
        let i = match self.alts.iter().position(|a| a.group == group) {
            Some(i) => i,
            None => {
                let mut st = EditorState::default();
                let (l, c) = self.st.cursor();
                st.goto(l, c);
                st.set_top_line(self.st.top_line());
                let pv = PvView { on: self.preview.as_ref().is_some_and(|p| p.on), ..Default::default() };
                self.alts.push(AltView { group, st, pv, focus_editor: false });
                self.alts.len() - 1
            }
        };
        let a = &mut self.alts[i];
        std::mem::swap(&mut self.st, &mut a.st);
        std::mem::swap(&mut self.focus_editor, &mut a.focus_editor);
        if let Some(pv) = self.preview.as_mut() {
            pv.swap_view(&mut a.pv);
        }
        a.group = self.owner;
        self.owner = group;
    }

    /// The view of `group` is gone.
    pub fn drop_view(&mut self, group: u32) {
        if self.owner == group {
            match self.alts.last().map(|a| a.group) {
                Some(next) => self.make_primary(next),
                None => return,
            }
        }
        self.alts.retain(|a| a.group != group);
    }

    /// The view of group `from` now belongs to group `to`.
    pub fn move_view(&mut self, from: u32, to: u32) {
        if self.owner == from {
            self.owner = to;
        } else if let Some(a) = self.alts.iter_mut().find(|a| a.group == from) {
            a.group = to;
        }
    }
}

pub enum TabKind {
    File(Box<FileTab>),
    Settings,
    Search(Box<SearchPanel>),
}

pub struct Tab {
    pub id: u64,
    pub kind: TabKind,
}

impl Tab {
    pub fn file(&self) -> Option<&FileTab> {
        match &self.kind {
            TabKind::File(f) => Some(f),
            _ => None,
        }
    }

    pub fn search(&self) -> Option<&SearchPanel> {
        match &self.kind {
            TabKind::Search(s) => Some(s),
            _ => None,
        }
    }

    pub fn search_mut(&mut self) -> Option<&mut SearchPanel> {
        match &mut self.kind {
            TabKind::Search(s) => Some(s),
            _ => None,
        }
    }

    pub fn file_mut(&mut self) -> Option<&mut FileTab> {
        match &mut self.kind {
            TabKind::File(f) => Some(f),
            _ => None,
        }
    }

    pub fn dirty(&self) -> bool {
        self.file().is_some_and(FileTab::dirty)
    }

    pub fn title(&self) -> String {
        match &self.kind {
            TabKind::File(f) => f.title(),
            TabKind::Settings => "Settings".into(),
            TabKind::Search(s) => s.title(),
        }
    }
}

// ---- layout ----

/// How the groups share the editor area. Leaves are group ids.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(u32),
    /// `down`: children stacked top to bottom, else side by side. Weights are relative.
    Split { down: bool, kids: Vec<(Node, f32)> },
}

/// The line between two children of a split.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    /// Hit area.
    pub rect: Rect,
    pub down: bool,
    /// Child indices from the root to the split.
    pub path: Vec<usize>,
    /// Between child `index` and `index + 1`.
    pub index: usize,
    /// Pixels the children share along the split.
    pub span: f32,
}

const MAX_DEPTH: usize = 8;

impl Node {
    /// Group ids, left to right / top to bottom.
    pub fn leaves(&self, out: &mut Vec<u32>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { kids, .. } => kids.iter().for_each(|(k, _)| k.leaves(out)),
        }
    }

    /// Put leaf `new` next to leaf `at` (`before`: left of / above it). `false`: no such leaf.
    pub fn insert(&mut self, at: u32, new: u32, down: bool, before: bool) -> bool {
        match self {
            Node::Leaf(id) if *id == at => {
                let (a, b) = (Node::Leaf(at), Node::Leaf(new));
                *self = Node::Split { down, kids: if before { vec![(b, 0.5), (a, 0.5)] } else { vec![(a, 0.5), (b, 0.5)] } };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { down: d, kids } => {
                if *d == down {
                    if let Some(i) = kids.iter().position(|(k, _)| matches!(k, Node::Leaf(id) if *id == at)) {
                        let w = kids[i].1 / 2.0;
                        kids[i].1 = w;
                        kids.insert(if before { i } else { i + 1 }, (Node::Leaf(new), w));
                        return true;
                    }
                }
                kids.iter_mut().any(|(k, _)| k.insert(at, new, down, before))
            }
        }
    }

    /// Replace leaf `at` with `sub`.
    pub fn graft(&mut self, at: u32, sub: &Node) -> bool {
        match self {
            Node::Leaf(id) if *id == at => {
                *self = sub.clone();
                true
            }
            Node::Leaf(_) => false,
            Node::Split { kids, .. } => {
                let hit = kids.iter_mut().any(|(k, _)| k.graft(at, sub));
                if hit {
                    self.tidy();
                }
                hit
            }
        }
    }

    /// Drop leaf `id`; splits left with one child collapse.
    pub fn remove(&mut self, id: u32) {
        if let Node::Split { kids, .. } = self {
            kids.retain(|(k, _)| !matches!(k, Node::Leaf(x) if *x == id));
            for (k, _) in kids.iter_mut() {
                k.remove(id);
            }
            self.tidy();
        }
    }

    /// One child → that child; a child split in the same direction → its children; weights sum to 1.
    fn tidy(&mut self) {
        let Node::Split { down, kids } = self else { return };
        let mut flat: Vec<(Node, f32)> = Vec::with_capacity(kids.len());
        for (k, w) in kids.drain(..) {
            match k {
                Node::Split { down: d, kids: inner } if d == *down => {
                    let sum: f32 = inner.iter().map(|x| x.1).sum::<f32>().max(1e-6);
                    flat.extend(inner.into_iter().map(|(n, x)| (n, w * x / sum)));
                }
                Node::Split { kids: inner, .. } if inner.is_empty() => {}
                k => flat.push((k, w)),
            }
        }
        if flat.len() == 1 {
            *self = flat.remove(0).0;
            return;
        }
        let sum: f32 = flat.iter().map(|x| x.1).sum::<f32>().max(1e-6);
        for x in &mut flat {
            x.1 = (x.1 / sum).max(1e-3);
        }
        *kids = flat;
    }

    /// Rects of the groups and the dividers between them. Dividers are 1 px.
    pub fn place(&self, rect: Rect, path: &mut Vec<usize>, out: &mut Vec<(u32, Rect)>, divs: &mut Vec<Divider>) {
        let (down, kids) = match self {
            Node::Leaf(id) => {
                out.push((*id, rect));
                return;
            }
            Node::Split { down, kids } => (*down, kids),
        };
        let n = kids.len();
        if n == 0 {
            return;
        }
        let total: f32 = kids.iter().map(|k| k.1).sum::<f32>().max(1e-6);
        let (start, end) = if down { (rect.top(), rect.bottom()) } else { (rect.left(), rect.right()) };
        let span = (end - start - (n - 1) as f32).max(0.0);
        let mut at = start;
        for (i, (k, w)) in kids.iter().enumerate() {
            let to = if i + 1 == n { end } else { (at + span * w / total).round().min(end) };
            let r = if down { Rect::from_min_max(pos2(rect.left(), at), pos2(rect.right(), to)) } else { Rect::from_min_max(pos2(at, rect.top()), pos2(to, rect.bottom())) };
            path.push(i);
            k.place(r, path, out, divs);
            path.pop();
            at = to;
            if i + 1 < n {
                let hit = if down { Rect::from_min_max(pos2(rect.left(), at - 3.0), pos2(rect.right(), at + 4.0)) } else { Rect::from_min_max(pos2(at - 3.0, rect.top()), pos2(at + 4.0, rect.bottom())) };
                divs.push(Divider { rect: hit, down, path: path.clone(), index: i, span });
                at += 1.0;
            }
        }
    }

    /// Move the divider after child `index` of the split at `path` by `px`.
    pub fn resize(&mut self, path: &[usize], index: usize, px: f32, span: f32) {
        let mut n = self;
        for &i in path {
            match n {
                Node::Split { kids, .. } if i < kids.len() => n = &mut kids[i].0,
                _ => return,
            }
        }
        let Node::Split { kids, .. } = n else { return };
        if index + 1 >= kids.len() || span <= 0.0 {
            return;
        }
        let total: f32 = kids.iter().map(|k| k.1).sum::<f32>().max(1e-6);
        let (a, b) = (kids[index].1, kids[index + 1].1);
        let min = (GROUP_MIN / span * total).min((a + b) / 2.0);
        let na = (a + px / span * total).clamp(min, a + b - min);
        kids[index].1 = na;
        kids[index + 1].1 = a + b - na;
    }

    /// `r(0:500,d(1:500,2:500):500)`: `r` side by side, `d` stacked, leaves = `index(group id)`, `:weight` in ‰.
    pub fn write(&self, index: &impl Fn(u32) -> Option<usize>) -> String {
        match self {
            Node::Leaf(id) => index(*id).unwrap_or(0).to_string(),
            Node::Split { down, kids } => {
                let total: f32 = kids.iter().map(|k| k.1).sum::<f32>().max(1e-6);
                let parts: Vec<String> = kids.iter().map(|(k, w)| format!("{}:{}", k.write(index), ((w / total * 1000.0).round() as u32).clamp(1, 1000))).collect();
                format!("{}({})", if *down { 'd' } else { 'r' }, parts.join(","))
            }
        }
    }

    /// Inverse of `write` for `n` groups: every index below `n` exactly once, leaves = indices. `None`: not that.
    pub fn parse(s: &str, n: usize) -> Option<Node> {
        if s.len() > 512 || n == 0 || n > MAX_GROUPS {
            return None;
        }
        let b = s.as_bytes();
        let mut at = 0;
        let node = parse_node(b, &mut at, 0)?;
        if at != b.len() {
            return None;
        }
        let mut seen = Vec::new();
        node.leaves(&mut seen);
        seen.sort_unstable();
        (seen.len() == n && seen.iter().enumerate().all(|(i, x)| *x as usize == i)).then_some(node)
    }

    /// Leaves through `f`.
    pub fn map(&self, f: &impl Fn(u32) -> u32) -> Node {
        match self {
            Node::Leaf(id) => Node::Leaf(f(*id)),
            Node::Split { down, kids } => Node::Split { down: *down, kids: kids.iter().map(|(k, w)| (k.map(f), *w)).collect() },
        }
    }
}

fn parse_num(b: &[u8], at: &mut usize) -> Option<u32> {
    let s = *at;
    while b.get(*at).is_some_and(u8::is_ascii_digit) && *at - s < 5 {
        *at += 1;
    }
    std::str::from_utf8(&b[s..*at]).ok()?.parse().ok()
}

fn parse_node(b: &[u8], at: &mut usize, depth: usize) -> Option<Node> {
    if depth > MAX_DEPTH {
        return None;
    }
    let down = match b.get(*at)? {
        b'r' => false,
        b'd' => true,
        _ => return parse_num(b, at).map(Node::Leaf),
    };
    *at += 1;
    if b.get(*at) != Some(&b'(') {
        return None;
    }
    *at += 1;
    let mut kids = Vec::new();
    loop {
        let k = parse_node(b, at, depth + 1)?;
        if b.get(*at) != Some(&b':') {
            return None;
        }
        *at += 1;
        let w = parse_num(b, at).filter(|w| (1..=1000).contains(w))?;
        kids.push((k, w as f32 / 1000.0));
        if kids.len() > MAX_GROUPS {
            return None;
        }
        match b.get(*at)? {
            b',' => *at += 1,
            b')' => {
                *at += 1;
                break;
            }
            _ => return None,
        }
    }
    (kids.len() >= 2).then_some(Node::Split { down, kids })
}

// ---- groups ----

/// One pane of the editor area.
pub struct Group {
    pub id: u32,
    /// Tab ids, strip order.
    pub tabs: Vec<u64>,
    pub active: Option<u64>,
    pub reveal_active: bool,
}

impl Group {
    fn new(id: u32) -> Group {
        Group { id, tabs: Vec::new(), active: None, reveal_active: false }
    }

    pub fn pos(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| *t == id)
    }

    /// Add `id` after the active tab and activate it.
    fn insert(&mut self, id: u64) {
        if self.pos(id).is_none() {
            let at = self.active.and_then(|a| self.pos(a)).map_or(self.tabs.len(), |a| a + 1);
            self.tabs.insert(at, id);
        }
        self.active = Some(id);
        self.reveal_active = true;
    }

    /// Put `id` at strip position `at` (clamped) and activate it.
    fn insert_at(&mut self, id: u64, at: usize) {
        if let Some(i) = self.pos(id) {
            self.tabs.remove(i);
        }
        self.tabs.insert(at.min(self.tabs.len()), id);
        self.active = Some(id);
        self.reveal_active = true;
    }

    /// Drop `id`; the active tab moves to its right neighbour (else the last).
    fn remove(&mut self, id: u64) -> bool {
        let Some(i) = self.pos(id) else { return false };
        self.tabs.remove(i);
        if self.active == Some(id) {
            self.active = self.tabs.get(i.min(self.tabs.len().saturating_sub(1))).copied();
        }
        self.reveal_active = true;
        true
    }
}

pub struct Tabs {
    /// Open buffers (order means nothing; the strips are in `groups`).
    pub list: Vec<Tab>,
    /// Never empty.
    pub groups: Vec<Group>,
    /// Index of the focused group.
    pub cur: usize,
    pub layout: Node,
    next_id: u64,
    next_group: u32,
}

impl Default for Tabs {
    fn default() -> Self {
        Tabs { list: Vec::new(), groups: vec![Group::new(1)], cur: 0, layout: Node::Leaf(1), next_id: 0, next_group: 1 }
    }
}

/// Where a dragged tab is dropped on a group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropAt {
    /// Into the group, at this strip position (`None`: after its active tab).
    Into(Option<usize>),
    /// A new group next to it: `down` below / above, else right / left; `before`: above / left.
    Side { down: bool, before: bool },
}

pub enum StripAction {
    Activate(u64),
    Close(u64),
    /// The tab is being dragged.
    Drag(u64),
    /// Right-click: context menu at the pointer.
    Menu(u64, egui::Pos2),
}

/// Strip widget of tab `id` in group `group`.
pub fn tab_id(group: u32, id: u64) -> Id {
    Id::unique(("tab", group, id))
}

impl Tabs {
    /// New buffer, shown in the focused group after its active tab. Returns its index in `list`.
    pub fn add(&mut self, kind: TabKind) -> usize {
        self.next_id += 1;
        let id = self.next_id;
        self.list.push(Tab { id, kind });
        self.groups[self.cur].insert(id);
        self.sync();
        self.list.len() - 1
    }

    /// Show buffer `i` in the focused group: activate its tab there, or add one (files); a Settings / search
    /// tab of another group takes the focus to that group.
    pub fn show(&mut self, i: usize) {
        let Some(t) = self.list.get(i) else { return };
        let id = t.id;
        if self.groups[self.cur].pos(id).is_none() && t.file().is_none() {
            if let Some(g) = self.groups.iter().position(|g| g.pos(id).is_some()) {
                self.cur = g;
            }
        }
        self.groups[self.cur].insert(id);
        self.sync();
    }

    /// Activate tab `id` of group `g` and focus that group.
    pub fn activate_in(&mut self, g: usize, id: u64) {
        if self.groups.get(g).is_some_and(|x| x.pos(id).is_some()) {
            self.cur = g;
            self.groups[g].active = Some(id);
            self.groups[g].reveal_active = true;
            self.sync();
        }
    }

    pub fn focus_group(&mut self, g: usize) {
        if g < self.groups.len() {
            self.cur = g;
            self.sync();
        }
    }

    /// The focused group's active view becomes the primary one of its file.
    pub fn sync(&mut self) {
        self.primary_for(self.cur);
    }

    /// Group `g`'s active view becomes the primary one of its file (before drawing it).
    pub fn primary_for(&mut self, g: usize) {
        let Some((gid, Some(id))) = self.groups.get(g).map(|g| (g.id, g.active)) else { return };
        if let Some(f) = self.file_by_id(id) {
            f.make_primary(gid);
        }
    }

    /// Close buffer `i` in every group.
    pub fn remove(&mut self, i: usize) -> Option<Tab> {
        if i >= self.list.len() {
            return None;
        }
        let t = self.list.remove(i);
        for g in &mut self.groups {
            g.remove(t.id);
        }
        self.prune();
        Some(t)
    }

    /// Groups showing tab `id`.
    pub fn views(&self, id: u64) -> usize {
        self.groups.iter().filter(|g| g.pos(id).is_some()).count()
    }

    /// Close the tab of `id` in group `group` only (the buffer stays open in the others).
    pub fn remove_view(&mut self, group: u32, id: u64) {
        let Some(g) = self.group_index(group) else { return };
        if self.groups[g].remove(id) {
            if let Some(f) = self.file_by_id(id) {
                f.drop_view(group);
            }
            self.prune();
        }
    }

    /// Drop empty groups (the last one stays); the focus goes to the group before.
    pub fn prune(&mut self) {
        let order = self.order();
        let cur_id = self.groups[self.cur].id;
        let mut dead: Vec<u32> = self.groups.iter().filter(|g| g.tabs.is_empty()).map(|g| g.id).collect();
        if dead.len() == self.groups.len() {
            dead.retain(|d| *d != cur_id);
        }
        if !dead.is_empty() {
            for d in &dead {
                self.layout.remove(*d);
            }
            self.groups.retain(|g| !dead.contains(&g.id));
            // The focused group went: the one before it (else the next) takes the focus.
            let at = order.iter().position(|x| *x == cur_id).unwrap_or(0);
            let next = if dead.contains(&cur_id) { order[..at].iter().rev().chain(order[at..].iter()).find(|x| !dead.contains(x)).copied() } else { Some(cur_id) };
            self.cur = next.and_then(|id| self.group_index(id)).unwrap_or(0);
        }
        self.sync();
    }

    /// Tab id the next `add` will use.
    pub fn next_id(&self) -> u64 {
        self.next_id + 1
    }

    /// Keep only tabs matching `keep` (ids keep counting up).
    pub fn retain(&mut self, keep: impl Fn(&Tab) -> bool) {
        self.list.retain(|t| keep(t));
        let ids: Vec<u64> = self.list.iter().map(|t| t.id).collect();
        for g in &mut self.groups {
            g.tabs.retain(|t| ids.contains(t));
            if g.active.is_none_or(|a| !g.tabs.contains(&a)) {
                g.active = g.tabs.first().copied();
            }
            g.reveal_active = true;
        }
        self.prune();
    }

    pub fn find_file(&self, p: &RelPath) -> Option<usize> {
        self.list.iter().position(|t| t.file().is_some_and(|f| !f.is_untitled() && &f.path == p))
    }

    pub fn search_by_id(&mut self, id: u64) -> Option<&mut SearchPanel> {
        self.list.iter_mut().find(|t| t.id == id).and_then(Tab::search_mut)
    }

    pub fn by_id(&self, id: u64) -> Option<usize> {
        self.list.iter().position(|t| t.id == id)
    }

    pub fn file_by_id(&mut self, id: u64) -> Option<&mut FileTab> {
        self.list.iter_mut().find(|t| t.id == id).and_then(Tab::file_mut)
    }

    /// Index in `list` of the focused group's active tab.
    pub fn active(&self) -> Option<usize> {
        self.groups[self.cur].active.and_then(|id| self.by_id(id))
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active().and_then(|a| self.list.get(a))
    }

    pub fn active_file(&self) -> Option<&FileTab> {
        self.active_tab().and_then(Tab::file)
    }

    pub fn active_file_mut(&mut self) -> Option<&mut FileTab> {
        let a = self.active()?;
        self.list.get_mut(a).and_then(Tab::file_mut)
    }

    /// Next / previous tab of the focused group.
    pub fn cycle(&mut self, d: isize) {
        let g = &mut self.groups[self.cur];
        let n = g.tabs.len() as isize;
        if let (Some(a), true) = (g.active.and_then(|a| g.pos(a)), n > 0) {
            g.active = Some(g.tabs[(a as isize + d).rem_euclid(n) as usize]);
            g.reveal_active = true;
        }
        self.sync();
    }

    /// Group ids in layout order (left to right, top to bottom).
    pub fn order(&self) -> Vec<u32> {
        let mut v = Vec::new();
        self.layout.leaves(&mut v);
        v
    }

    pub fn group_index(&self, id: u32) -> Option<usize> {
        self.groups.iter().position(|g| g.id == id)
    }

    /// Active tab ids of all groups, no duplicates.
    pub fn visible(&self) -> Vec<u64> {
        let mut v: Vec<u64> = Vec::new();
        for id in self.groups.iter().filter_map(|g| g.active) {
            if !v.contains(&id) {
                v.push(id);
            }
        }
        v
    }

    fn new_group(&mut self) -> u32 {
        self.next_group += 1;
        self.groups.push(Group::new(self.next_group));
        self.next_group
    }

    /// New group right of / below the focused one, showing the same file; it takes the focus.
    /// `false`: the active tab is not a file, or `MAX_GROUPS` are open.
    pub fn split(&mut self, down: bool) -> bool {
        let g = &self.groups[self.cur];
        let (from, Some(id)) = (g.id, g.active) else { return false };
        if self.groups.len() >= MAX_GROUPS || !self.by_id(id).is_some_and(|i| self.list[i].file().is_some()) {
            return false;
        }
        let new = self.new_group();
        self.layout.insert(from, new, down, false);
        self.cur = self.groups.len() - 1;
        self.groups[self.cur].insert(id);
        self.sync();
        true
    }

    /// Focus the group `d` steps away in layout order (wraps).
    pub fn focus_step(&mut self, d: isize) {
        let order = self.order();
        let cur = self.groups[self.cur].id;
        let n = order.len() as isize;
        if let (Some(at), true) = (order.iter().position(|x| *x == cur), n > 1) {
            let next = order[(at as isize + d).rem_euclid(n) as usize];
            if let Some(g) = self.group_index(next) {
                self.focus_group(g);
            }
        }
    }

    /// Move the focused group's active tab to the next (`d > 0`) / previous group in layout order; past the
    /// edge: to a new group there (when the tab is not alone). The tab keeps the focus.
    pub fn move_active(&mut self, d: isize) -> bool {
        let (sid, Some(id)) = (self.groups[self.cur].id, self.groups[self.cur].active) else { return false };
        let order = self.order();
        let Some(at) = order.iter().position(|x| *x == sid) else { return false };
        let to = at as isize + d.signum();
        if to < 0 || to >= order.len() as isize {
            return self.drop_tab(sid, id, sid, DropAt::Side { down: false, before: d < 0 });
        }
        self.drop_tab(sid, id, order[to as usize], DropAt::Into(None))
    }

    /// Move tab `id` of group `from` to group `to` (`Into`), or to a new group on a side of `to` (`Side`).
    /// The tab ends up active and focused. A file already open in the target: the two tabs become one.
    /// `false`: nothing to do (a lone tab split off its own group, no room for a group, unknown ids).
    pub fn drop_tab(&mut self, from: u32, id: u64, to: u32, at: DropAt) -> bool {
        let Some(si) = self.group_index(from).filter(|_| self.group_index(to).is_some()) else { return false };
        let Some(k) = self.groups[si].pos(id) else { return false };
        let (dst, index) = match at {
            DropAt::Into(index) => (to, index),
            DropAt::Side { down, before } => {
                if (from == to && self.groups[si].tabs.len() < 2) || self.groups.len() >= MAX_GROUPS {
                    return false;
                }
                let new = self.new_group();
                self.layout.insert(to, new, down, before);
                (new, None)
            }
        };
        if dst == from {
            // Reorder; positions after the tab shift left once it is lifted out.
            if let Some(i) = index {
                self.groups[si].insert_at(id, if i > k { i - 1 } else { i });
            }
            self.groups[si].active = Some(id);
            self.cur = si;
            self.sync();
            return true;
        }
        let Some(di) = self.group_index(dst) else { return false };
        self.groups[si].remove(id);
        let there = self.groups[di].pos(id).is_some();
        if let Some(f) = self.file_by_id(id) {
            if there {
                f.drop_view(from);
            } else {
                f.move_view(from, dst);
            }
        }
        match index {
            Some(i) => self.groups[di].insert_at(id, i),
            None => self.groups[di].insert(id),
        }
        self.prune();
        self.cur = self.group_index(dst).unwrap_or(0);
        self.sync();
        true
    }

    /// Start-up: `n - 1` empty groups next to the focused one, laid out as `layout` says (`Node::write`;
    /// unreadable: a row). Returns the group id of each saved index. Call `prune` once they are filled.
    pub fn restore_groups(&mut self, n: usize, layout: &str) -> Vec<u32> {
        let first = self.groups[self.cur].id;
        let mut ids = vec![first];
        while ids.len() < n && self.groups.len() < MAX_GROUPS {
            ids.push(self.new_group());
        }
        if ids.len() > 1 {
            let sub = match Node::parse(layout, ids.len()) {
                Some(node) => node.map(&|i| ids[i as usize]),
                None => Node::Split { down: false, kids: ids.iter().map(|id| (Node::Leaf(*id), 1.0)).collect() },
            };
            self.layout.graft(first, &sub);
        }
        ids.resize(n.max(1), first);
        ids
    }

    /// Muted suffix (parent directory name) for tabs whose names collide.
    fn suffix(&self, i: usize) -> Option<String> {
        let f = self.list[i].file().filter(|f| !f.is_untitled())?;
        let dup = self.list.iter().enumerate().any(|(j, t)| j != i && t.file().is_some_and(|g| !g.is_untitled() && g.path.name() == f.path.name()));
        if !dup {
            return None;
        }
        let parent = f.path.parent().unwrap_or_default();
        Some(if parent.is_root() { "./".into() } else { clean(parent.name()) })
    }

    /// Draw the strip of group `g` in `rect`. `focused`: it is the focused group.
    pub fn strip(&mut self, ui: &mut Ui, rect: Rect, t: &Theme, g: usize, focused: bool) -> Option<StripAction> {
        let p = ui.painter().clone();
        p.rect_filled(rect, 0.0, t.ui.panel_bg);
        // Under everything: the active tab's fill covers it, joining the editor.
        p.hline(rect.x_range(), rect.bottom() - 0.5, t.border());
        let mut act = None;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        child.set_clip_rect(rect);
        let group = self.groups.get_mut(g)?;
        let gid = group.id;
        let reveal = std::mem::take(&mut group.reveal_active);
        let ids = group.tabs.clone();
        let active_id = group.active;
        egui::ScrollArea::horizontal().id_salt(("tab-strip", gid)).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).auto_shrink([false, false]).show(&mut child, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.horizontal(|ui| {
                for id in ids {
                    let Some(i) = self.by_id(id) else { continue };
                    let title = self.list[i].title();
                    let suffix = self.suffix(i);
                    let active = active_id == Some(id);
                    let dirty = self.list[i].dirty();
                    let name_w = ui.painter().layout_no_wrap(title.clone(), t.font_ui(), t.ui.text).size().x.min(220.0);
                    let suf_w = suffix.as_ref().map_or(0.0, |s| ui.painter().layout_no_wrap(s.clone(), t.font_small(), t.ui.text).size().x.min(120.0) + 6.0);
                    let w = (12.0 + name_w + suf_w + 8.0 + 16.0 + 8.0).round();
                    let (r, _) = ui.allocate_exact_size(vec2(w, rect.height()), Sense::hover());
                    let resp = ui.interact(r, tab_id(gid, id), Sense::click_and_drag());
                    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                    if active && reveal {
                        ui.scroll_to_rect(r, None);
                    }
                    let painter = ui.painter();
                    if active {
                        painter.rect_filled(r, 0.0, t.code.bg);
                        painter.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 2.0)), 0.0, if focused { t.ui.accent } else { t.ui.text_faint });
                    } else if resp.hovered() {
                        painter.rect_filled(r, 0.0, t.ui.row_hover);
                    }
                    painter.vline(r.right() - 0.5, r.y_range(), t.border());
                    let color = if active && focused { t.ui.text_strong } else if active { t.ui.text } else { t.ui.text_muted };
                    let x = r.left() + 12.0;
                    let tw = widgets::text_at(ui, r.with_max_x(r.right() - 28.0), x, &title, t.font_ui(), color);
                    if let Some(s) = &suffix {
                        widgets::text_at(ui, r.with_max_x(r.right() - 28.0), x + tw + 6.0, s, t.font_small(), t.ui.text_faint);
                    }
                    let c = pos2(r.right() - 16.0, r.center().y);
                    let cr = Rect::from_center_size(c, vec2(18.0, 18.0));
                    let close = ui.interact(cr, Id::unique(("tab-close", gid, id)), Sense::click());
                    let painter = ui.painter();
                    if close.hovered() {
                        painter.rect_filled(cr, 3.0, t.ui.row_selected);
                    }
                    if close.hovered() || (resp.hovered() && !dirty) || (active && !dirty) {
                        let col = if close.hovered() { t.ui.text_strong } else { t.ui.text_muted };
                        widgets::close_x(painter, c, 3.5, col);
                    } else if dirty {
                        widgets::dot(painter, c, 3.5, if active { t.ui.text } else { t.ui.text_muted });
                    }
                    if resp.secondary_clicked() {
                        act = Some(StripAction::Menu(id, resp.interact_pointer_pos().unwrap_or(r.center())));
                    } else if close.clicked() || resp.middle_clicked() {
                        act = Some(StripAction::Close(id));
                    } else if resp.clicked() {
                        act = Some(StripAction::Activate(id));
                    } else if resp.drag_started_by(egui::PointerButton::Primary) {
                        act = Some(StripAction::Drag(id));
                    }
                }
            });
        });
        act
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(tabs: &mut Tabs, path: &str) -> u64 {
        let i = tabs.add(TabKind::File(Box::new(FileTab::new(RelPath::new(path).unwrap()))));
        tabs.list[i].id
    }

    fn shape(t: &Tabs) -> Vec<(u32, Vec<u64>, Option<u64>)> {
        t.order().iter().filter_map(|id| t.group_index(*id)).map(|g| (t.groups[g].id, t.groups[g].tabs.clone(), t.groups[g].active)).collect()
    }

    #[test]
    fn split_shares_the_buffer_and_keeps_a_view_per_group() {
        let mut t = Tabs::default();
        let a = file(&mut t, "a.rs");
        let b = file(&mut t, "b.rs");
        t.file_by_id(b).unwrap().st.goto(7, 0);
        assert!(t.split(false));
        assert_eq!(shape(&t), vec![(1, vec![a, b], Some(b)), (2, vec![b], Some(b))]);
        assert_eq!((t.cur, t.list.len(), t.views(b)), (1, 2, 2));
        // The new view starts where the old one was, then moves on its own.
        let f = t.file_by_id(b).unwrap();
        assert_eq!((f.owner, f.alts.len(), f.st.cursor()), (2, 1, (7, 0)));
        f.st.goto(3, 0);
        t.focus_group(0);
        assert_eq!(t.active_file().unwrap().st.cursor(), (7, 0));
        t.focus_group(1);
        assert_eq!(t.active_file().unwrap().st.cursor(), (3, 0));
        // Closing one view keeps the buffer; the last one drops the group.
        t.remove_view(2, b);
        assert_eq!(shape(&t), vec![(1, vec![a, b], Some(b))]);
        let f = t.file_by_id(b).unwrap();
        assert_eq!((f.owner, f.alts.len(), f.st.cursor()), (1, 0, (7, 0)));
        assert_eq!(t.layout, Node::Leaf(1));
        // Settings cannot be split.
        t.add(TabKind::Settings);
        assert!(!t.split(false));
    }

    #[test]
    fn move_cycle_and_close() {
        let mut t = Tabs::default();
        let a = file(&mut t, "a.rs");
        let b = file(&mut t, "b.rs");
        let c = file(&mut t, "c.rs");
        // Past the right edge: a new group.
        assert!(t.move_active(1));
        assert_eq!(shape(&t), vec![(1, vec![a, b], Some(b)), (2, vec![c], Some(c))]);
        assert_eq!(t.groups[t.cur].id, 2);
        // A lone tab does not make a new group; moving left joins group 1 and drops group 2.
        assert!(!t.move_active(1));
        assert!(t.move_active(-1));
        assert_eq!(shape(&t), vec![(1, vec![a, b, c], Some(c))]);
        assert_eq!(t.file_by_id(c).unwrap().owner, 1);
        // The same file on both sides: moving merges the views.
        assert!(t.split(false));
        assert!(t.move_active(-1));
        assert_eq!(shape(&t), vec![(1, vec![a, b, c], Some(c))]);
        assert!(t.file_by_id(c).unwrap().alts.is_empty());
        // Cycle stays in the group; focus_step walks the layout.
        t.split(true);
        t.cycle(1);
        assert_eq!(t.groups[t.cur].active, Some(c));
        t.focus_step(1);
        assert_eq!(t.groups[t.cur].id, 1);
        t.cycle(1);
        assert_eq!(t.groups[t.cur].active, Some(a));
        // Closing a buffer closes it everywhere; the focus goes to the group before.
        t.focus_step(1);
        let i = t.by_id(c).unwrap();
        t.remove(i);
        assert_eq!(shape(&t), vec![(1, vec![a, b], Some(a))]);
        assert_eq!(t.cur, 0);
        t.retain(|_| false);
        assert_eq!((t.groups.len(), t.active()), (1, None));
    }

    #[test]
    fn drop_tab_moves_splits_and_reorders() {
        let mut t = Tabs::default();
        let a = file(&mut t, "a.rs");
        let b = file(&mut t, "b.rs");
        let c = file(&mut t, "c.rs");
        // Reorder: c before a, then a to the end.
        assert!(t.drop_tab(1, c, 1, DropAt::Into(Some(0))));
        assert_eq!(shape(&t), vec![(1, vec![c, a, b], Some(c))]);
        assert!(t.drop_tab(1, c, 1, DropAt::Into(Some(3))));
        assert_eq!(shape(&t), vec![(1, vec![a, b, c], Some(c))]);
        assert!(t.drop_tab(1, b, 1, DropAt::Into(Some(1))));
        assert_eq!(shape(&t), vec![(1, vec![a, b, c], Some(b))]);
        // Edges: below, then left of the first group.
        assert!(t.drop_tab(1, c, 1, DropAt::Side { down: true, before: false }));
        assert_eq!(shape(&t), vec![(1, vec![a, b], Some(b)), (2, vec![c], Some(c))]);
        assert!(matches!(&t.layout, Node::Split { down: true, .. }));
        assert_eq!((t.groups[t.cur].id, t.file_by_id(c).unwrap().owner), (2, 2));
        assert!(t.drop_tab(1, a, 1, DropAt::Side { down: false, before: true }));
        assert_eq!(shape(&t), vec![(3, vec![a], Some(a)), (1, vec![b], Some(b)), (2, vec![c], Some(c))]);
        // A lone tab is not split off its own group, but moves next to another one.
        assert!(!t.drop_tab(2, c, 2, DropAt::Side { down: false, before: false }));
        assert!(t.drop_tab(2, c, 3, DropAt::Side { down: true, before: false }));
        assert_eq!(t.order().len(), 3);
        // Into another group at a position; the emptied group goes.
        let gc = t.groups[t.cur].id;
        assert!(t.drop_tab(gc, c, 1, DropAt::Into(Some(0))));
        assert_eq!(shape(&t), vec![(3, vec![a], Some(a)), (1, vec![c, b], Some(c))]);
        assert_eq!(t.groups[t.cur].id, 1);
        assert!(!t.drop_tab(9, c, 1, DropAt::Into(None)));
        assert!(!t.drop_tab(3, c, 1, DropAt::Into(None)), "c is not in group 3");
    }

    #[test]
    fn layout_tree() {
        let mut n = Node::Leaf(1);
        assert!(n.insert(1, 2, false, false));
        assert!(n.insert(2, 3, true, false));
        assert!(n.insert(1, 4, false, false));
        assert!(!n.insert(9, 5, false, false));
        let mut v = Vec::new();
        n.leaves(&mut v);
        assert_eq!(v, vec![1, 4, 2, 3]);
        let idx = |id: u32| v.iter().position(|x| *x == id);
        let s = n.write(&idx);
        assert_eq!(s, "r(0:250,1:250,d(2:500,3:500):500)");
        assert_eq!(Node::parse(&s, 4).map(|p| p.map(&|i| v[i as usize])), Some(n.clone()));
        // Rects: 1 px dividers, everything inside.
        let (mut out, mut divs) = (Vec::new(), Vec::new());
        let area = Rect::from_min_size(pos2(10.0, 20.0), vec2(803.0, 401.0));
        n.place(area, &mut Vec::new(), &mut out, &mut divs);
        assert_eq!(out.len(), 4);
        assert_eq!(divs.len(), 3);
        assert_eq!(out[0].1, Rect::from_min_max(pos2(10.0, 20.0), pos2(210.0, 421.0)));
        assert_eq!(out[3].1.max, area.max);
        assert_eq!(out[2].1.bottom() + 1.0, out[3].1.top());
        // Drag the first divider right; it stops at the minimum size.
        let d = divs[0].clone();
        n.resize(&d.path, d.index, 50.0, d.span);
        let (mut out2, mut divs2) = (Vec::new(), Vec::new());
        n.place(area, &mut Vec::new(), &mut out2, &mut divs2);
        assert_eq!(out2[0].1.width(), 250.0);
        n.resize(&d.path, d.index, 5000.0, d.span);
        out2.clear();
        n.place(area, &mut Vec::new(), &mut out2, &mut divs2);
        assert!((out2[1].1.width() - GROUP_MIN).abs() <= 1.0, "{:?}", out2[1].1);
        // Removing collapses.
        n.remove(3);
        n.remove(4);
        assert!(matches!(&n, Node::Split { down: false, kids } if kids.len() == 2));
        n.remove(1);
        assert_eq!(n, Node::Leaf(2));
        // A split in the same direction as its parent is flattened.
        let mut g = Node::Leaf(1);
        g.graft(1, &Node::Split { down: false, kids: vec![(Node::Leaf(1), 0.5), (Node::Split { down: false, kids: vec![(Node::Leaf(2), 0.5), (Node::Leaf(3), 0.5)] }, 0.5)] });
        g.insert(1, 4, true, false);
        g.remove(4);
        assert!(matches!(&g, Node::Split { kids, .. } if kids.len() == 3));
    }

    #[test]
    fn hostile_layout_text() {
        for bad in ["", "0", "r(0:500)", "r(0:500,0:500)", "r(0:500,2:500)", "r(0:0,1:500)", "r(0:500,1:500", "x(0:1,1:1)", "r(0:500,1:500)x", "r(0:99999,1:1)", "r(0:1,1:1,2:1)"] {
            assert_eq!(Node::parse(bad, 2), None, "{bad}");
        }
        assert!(Node::parse("0", 1).is_some());
        let deep = format!("{}0:1,1:1{}", "r(".repeat(40), ")".repeat(40));
        assert_eq!(Node::parse(&deep, 2), None);
        assert_eq!(Node::parse(&"r(0:1,".repeat(200), 2), None);
        // Restore with an unreadable layout: a row.
        let mut t = Tabs::default();
        let ids = t.restore_groups(3, "garbage");
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(t.order(), vec![1, 2, 3]);
        t.prune();
        assert_eq!((t.groups.len(), t.layout.clone()), (1, Node::Leaf(1)));
        let ids = t.restore_groups(20, "");
        assert_eq!(ids.len(), 20);
        assert_eq!(t.groups.len(), MAX_GROUPS);
    }
}
