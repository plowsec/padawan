//! Folder picker (cmd+O): our own overlay; directory names come from `Opener::browse` (async).
//!
//! | Key | Does |
//! |---|---|
//! | Enter in the path field | go there (or descend into the selected row) |
//! | Tab | complete the last path segment against the listing |
//! | Up / Down | move; Enter / double-click descends; `..` goes up |
//! | cmd+Enter | open this folder |
//! | Esc | close |

use std::path::PathBuf;

use egui::{pos2, vec2, CursorIcon, Id, Key, Order, Rect, Sense};
use margin_proto::DirListing;
use margin_theme::Theme;

use crate::sanitize::clean;
use crate::widgets;

#[derive(Clone, Debug, PartialEq)]
pub enum PickerOut {
    Browse(Option<PathBuf>),
    Open(PathBuf),
    Close,
}

#[derive(Clone, Debug, PartialEq)]
enum Row {
    Up,
    Dir(usize),
    Section,
    Recent(usize),
}

pub struct Picker {
    pub path: String,
    pub listing: Option<DirListing>,
    /// Path asked for, not yet answered.
    pub waiting: bool,
    pub error: Option<String>,
    selected: usize,
    fresh: bool,
    reveal: bool,
    last_click: Option<(usize, f64)>,
}

pub fn input_id() -> Id {
    Id::unique("picker-input")
}

/// Join a directory and a name with `/` (paths shown and typed are plain strings).
fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') || dir.ends_with('\\') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// Longest common prefix (by chars) of names starting with `prefix` (case-insensitive).
pub fn complete(names: &[String], prefix: &str) -> Option<String> {
    let lp = prefix.to_lowercase();
    let hits: Vec<&String> = names.iter().filter(|n| n.to_lowercase().starts_with(&lp)).collect();
    let first = hits.first()?;
    let mut common: Vec<char> = first.chars().collect();
    for h in &hits[1..] {
        let n = common.iter().zip(h.chars()).take_while(|(a, b)| **a == *b).count();
        common.truncate(n);
    }
    let c: String = common.into_iter().collect();
    (c.chars().count() >= prefix.chars().count()).then_some(c)
}

impl Picker {
    pub fn new() -> Picker {
        Picker { path: String::new(), listing: None, waiting: true, error: None, selected: 0, fresh: true, reveal: false, last_click: None }
    }

    pub fn set_listing(&mut self, mut l: DirListing) {
        l.dirs.truncate(margin_proto::limits::MAX_DIR_ENTRIES);
        self.waiting = false;
        self.path = l.path.clone();
        self.selected = usize::from(l.parent.is_some() && !l.dirs.is_empty());
        self.listing = Some(l);
        self.fresh = true;
    }

    fn rows(&self, recent: &[String]) -> Vec<Row> {
        let mut v = Vec::new();
        if let Some(l) = &self.listing {
            if l.parent.is_some() {
                v.push(Row::Up);
            }
            v.extend((0..l.dirs.len()).map(Row::Dir));
        }
        if !recent.is_empty() {
            v.push(Row::Section);
            v.extend((0..recent.len()).map(Row::Recent));
        }
        v
    }

    fn target(&self, row: &Row, recent: &[String]) -> Option<PickerOut> {
        let l = self.listing.as_ref();
        Some(match row {
            Row::Up => PickerOut::Browse(Some(PathBuf::from(l?.parent.clone()?))),
            Row::Dir(i) => {
                let l = l?;
                PickerOut::Browse(Some(PathBuf::from(join(&l.path, l.dirs.get(*i)?))))
            }
            Row::Recent(i) => PickerOut::Open(PathBuf::from(recent.get(*i)?)),
            Row::Section => return None,
        })
    }

    pub fn show(&mut self, ctx: &egui::Context, t: &Theme, mac: bool, recent: &[String], now: f64) -> Option<PickerOut> {
        let screen = ctx.content_rect();
        let mut out = None;
        let scrim = egui::Area::new(Id::unique("picker-scrim")).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, t.ui.scrim);
            ui.interact(screen, Id::unique("picker-scrim-hit"), Sense::click())
        });
        if scrim.inner.clicked() {
            out = Some(PickerOut::Close);
        }
        let rows = self.rows(recent);
        let n = rows.len();
        let (up, down, enter, cmd_enter, esc, tab) = ctx.input_mut(|i| {
            let m = egui::Modifiers::NONE;
            let ce = i.consume_key(egui::Modifiers::COMMAND, Key::Enter);
            (i.consume_key(m, Key::ArrowUp), i.consume_key(m, Key::ArrowDown), i.consume_key(m, Key::Enter), ce, i.consume_key(m, Key::Escape), i.consume_key(m, Key::Tab))
        });
        let selectable = |r: &Row| *r != Row::Section;
        let mv = |s: &mut Self, d: isize| {
            if n == 0 {
                return;
            }
            let mut i = (s.selected as isize + d).clamp(0, n as isize - 1) as usize;
            if !selectable(&rows[i]) {
                i = (i as isize + d).clamp(0, n as isize - 1) as usize;
            }
            s.selected = i;
            s.reveal = true;
        };
        if down {
            mv(self, 1);
        }
        if up {
            mv(self, -1);
        }
        if esc {
            out = Some(PickerOut::Close);
        }
        let shown = self.listing.as_ref().map(|l| l.path.clone()).unwrap_or_default();
        if cmd_enter && !shown.is_empty() {
            out = Some(PickerOut::Open(PathBuf::from(&shown)));
        } else if enter {
            if self.path.trim() != shown && !self.path.trim().is_empty() {
                out = Some(PickerOut::Browse(Some(PathBuf::from(self.path.trim()))));
            } else if let Some(r) = rows.get(self.selected) {
                out = self.target(r, recent).or(out);
            }
        }
        if tab {
            if let Some(l) = &self.listing {
                if let Some((dir, prefix)) = self.path.rsplit_once('/') {
                    let dir_s = if dir.is_empty() { "/".to_owned() } else { dir.to_owned() };
                    if dir_s.trim_end_matches('/') == l.path.trim_end_matches('/') || (dir.is_empty() && l.path == "/") {
                        if let Some(c) = complete(&l.dirs, prefix) {
                            self.path = join(&dir_s, &c);
                            self.fresh = true;
                        }
                    }
                }
            }
        }

        let row_h = t.row_h();
        let w = 620.0f32.min(screen.width() - 32.0);
        let in_h = (row_h + 18.0).round();
        let list_rows = n.clamp(6, 14);
        let foot = row_h + 20.0;
        let h = in_h + 1.0 + list_rows as f32 * row_h + 8.0 + foot;
        let h = h.min(screen.height() - 48.0);
        let pos = pos2((screen.center().x - w / 2.0).round(), (screen.top() + (screen.height() * 0.1).clamp(24.0, 80.0)).round());
        let rect = Rect::from_min_size(pos, vec2(w, h));
        egui::Area::new(Id::unique("picker")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            let p = ui.painter().clone();
            p.add(ctx.global_style().visuals.window_shadow.as_shape(rect, 6));
            p.rect_filled(rect, 6.0, t.ui.overlay_bg);
            p.rect_stroke(rect, 6.0, t.border(), egui::StrokeKind::Inside);
            ui.interact(rect, Id::unique("picker-body"), Sense::click());
            let ir = Rect::from_min_size(rect.min + vec2(16.0, 6.0), vec2(w - 32.0, in_h - 12.0));
            let resp = widgets::field_bare(ui, ir, input_id(), &mut self.path, "Folder path", t, t.font_mono_ui());
            if std::mem::take(&mut self.fresh) {
                widgets::cursor_to_end(ctx, input_id(), &self.path);
            }
            if !resp.has_focus() {
                resp.request_focus();
            }
            p.hline(rect.x_range(), rect.top() + in_h + 0.5, t.border());
            let footer = Rect::from_min_max(pos2(rect.left(), rect.bottom() - foot), rect.max);
            p.hline(rect.x_range(), footer.top() + 0.5, t.border());
            let lr = Rect::from_min_max(pos2(rect.left(), rect.top() + in_h + 1.0), pos2(rect.right(), footer.top() - 4.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(lr));
            child.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let reveal = std::mem::take(&mut self.reveal).then_some(self.selected);
            let mut clicked: Option<(usize, bool)> = None;
            let listing = self.listing.as_ref();
            if listing.is_none() || (self.waiting && listing.is_none()) {
                let r = Rect::from_min_size(lr.min, vec2(lr.width(), row_h));
                widgets::text_at(&child, r, r.left() + 12.0, "Loading…", t.font_small(), t.ui.text_faint);
            }
            widgets::virtual_rows(&mut child, Id::unique("picker-rows"), n, row_h, reveal, |ui, i, r| {
                let row = &rows[i];
                let r = r.shrink2(vec2(4.0, 0.0));
                if *row == Row::Section {
                    widgets::text_at(ui, r, r.left() + 8.0, "Recent", t.font_small(), t.ui.text_muted);
                    return;
                }
                let resp = ui.interact(r, Id::unique(("picker-row", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                if i == self.selected {
                    ui.painter().rect_filled(r, 4.0, t.ui.row_selected);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, t.ui.row_hover);
                }
                let x = r.left() + 8.0;
                match row {
                    Row::Up => {
                        widgets::text_at(ui, r, x, "..", t.font_mono_ui(), t.ui.text_muted);
                    }
                    Row::Dir(d) => {
                        let name = listing.and_then(|l| l.dirs.get(*d)).map(|s| clean(s)).unwrap_or_default();
                        widgets::chevron(ui.painter(), pos2(r.right() - 14.0, r.center().y), false, t.ui.text_faint);
                        widgets::text_at(ui, r.with_max_x(r.right() - 24.0), x, &name, t.font_ui(), t.ui.text);
                    }
                    Row::Recent(k) => {
                        let full = recent.get(*k).cloned().unwrap_or_default();
                        let name = full.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(&full).to_owned();
                        widgets::paint_job(ui, r.with_max_x(r.right() - 8.0), x, &[(&clean(&name), t.font_ui(), t.ui.text_strong), ("   ", t.font_small(), t.ui.text_muted), (&clean(&full), t.font_small(), t.ui.text_muted)]);
                    }
                    Row::Section => {}
                }
                if resp.clicked() {
                    clicked = Some((i, resp.double_clicked()));
                }
            });
            if let Some((i, dbl)) = clicked {
                let again = self.last_click.is_some_and(|(j, at)| j == i && now - at < 0.4);
                self.selected = i;
                self.last_click = Some((i, now));
                let is_recent = matches!(rows[i], Row::Recent(_) | Row::Up);
                if dbl || again || is_recent {
                    out = self.target(&rows[i], recent).or(out.clone());
                }
            }
            // Footer: error / where we are, and the primary action.
            let mut f = ui.new_child(egui::UiBuilder::new().max_rect(footer.shrink2(vec2(12.0, 8.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
            let label = format!("Open this folder   {}", if mac { "⌘↩" } else { "Ctrl+Enter" });
            if widgets::primary_button(&mut f, &label, t).clicked() && !shown.is_empty() {
                out = Some(PickerOut::Open(PathBuf::from(&shown)));
            }
            if widgets::quiet_button(&mut f, "Cancel", t).clicked() {
                out = Some(PickerOut::Close);
            }
            let msg_r = Rect::from_min_max(footer.min + vec2(12.0, 0.0), pos2(f.max_rect().left() + f.available_width() - 8.0, footer.bottom()));
            let err = self.error.clone().or_else(|| self.listing.as_ref().filter(|l| !l.error.is_empty()).map(|l| l.error.clone()));
            match err {
                Some(e) => widgets::text_at(ui, msg_r, msg_r.left(), &crate::sanitize::sanitize(&e, 200), t.font_small(), t.ui.error),
                None => widgets::text_at(ui, msg_r, msg_r.left(), &clean(&shown), t.font_small(), t.ui.text_muted),
            };
        });
        out
    }
}

impl Default for Picker {
    fn default() -> Self {
        Picker::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_completion() {
        let names: Vec<String> = ["Documents", "Downloads", "dev", "Desktop"].iter().map(|s| s.to_string()).collect();
        assert_eq!(complete(&names, "Dow").as_deref(), Some("Downloads"));
        assert_eq!(complete(&names, "Do").as_deref(), Some("Do"));
        assert_eq!(complete(&names, "x"), None);
        assert_eq!(join("/a", "b"), "/a/b");
        assert_eq!(join("/", "b"), "/b");
    }
}
