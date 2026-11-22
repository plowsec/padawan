//! In-window menu bar, built from `commands::MENUS` (titles and shortcuts from the command table).
//!
//! | Input (a menu open) | Does |
//! |---|---|
//! | click a title | open / close it; hovering another title switches |
//! | Up / Down | move (separators and disabled items skipped) |
//! | Left / Right | previous / next menu; Right on `Open Recent` opens it, Left closes it |
//! | Enter | run the item |
//! | Esc, click outside | close |

use egui::{pos2, vec2, CursorIcon, Id, Pos2, Rect, Sense, Ui};
use margin_theme::Theme;

use crate::commands::{self, Cmd, MenuItem, MENUS};
use crate::sanitize::sanitize;
use crate::widgets::{self, MenuEntry};

#[derive(Clone, Debug, PartialEq)]
pub enum MenuPick {
    Cmd(Cmd),
    /// A recent folder (absolute path, as stored).
    Recent(String),
}

#[derive(Default)]
pub struct MenuBar {
    /// Open menu (index into `MENUS`).
    pub open: Option<usize>,
    pub sel: Option<usize>,
    /// `Open Recent` submenu shown.
    pub sub_open: bool,
    pub sub_sel: Option<usize>,
    bar: Option<Rect>,
    titles: Vec<Rect>,
    popup: Option<Rect>,
    sub: Option<Rect>,
    /// Frame the menu opened in (the opening click is not an outside click).
    opened_frame: u64,
    /// Title under the pointer last frame (hover switches menus only when this changes).
    hover: Option<usize>,
}

pub fn title_id(i: usize) -> Id {
    Id::unique(("menubar-title", i))
}

pub fn popup_id(menu: usize) -> Id {
    Id::unique(("menubar-menu", menu))
}

pub fn recent_id() -> Id {
    Id::unique("menubar-recent")
}

pub fn height(t: &Theme) -> f32 {
    t.row_h().round()
}

/// Entries of menu `m`.
pub fn entries(m: usize, mac: bool, enabled: &dyn Fn(Cmd) -> bool, has_recent: bool) -> Vec<MenuEntry> {
    let Some((_, items)) = MENUS.get(m) else { return Vec::new() };
    items
        .iter()
        .map(|it| match it {
            MenuItem::Cmd(c) => MenuEntry::item(commands::def(*c).title, commands::shortcut(*c, mac), enabled(*c)),
            MenuItem::Sep => MenuEntry::sep(),
            MenuItem::Recent => MenuEntry { submenu: true, ..MenuEntry::item("Open Recent", "", has_recent) },
        })
        .collect()
}

fn recent_entries(recent: &[String]) -> Vec<MenuEntry> {
    recent.iter().take(12).map(|r| MenuEntry::item(sanitize(r, 80), "", true)).collect()
}

impl MenuBar {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn close(&mut self) {
        self.open = None;
        self.sel = None;
        self.sub_open = false;
        self.sub_sel = None;
        self.popup = None;
        self.sub = None;
    }

    fn open_menu(&mut self, i: usize, frame: u64) {
        self.open = Some(i);
        self.sel = None;
        self.sub_open = false;
        self.sub_sel = None;
        self.opened_frame = frame;
    }

    /// Draw the bar in `rect`.
    pub fn bar(&mut self, ui: &mut Ui, rect: Rect, t: &Theme, frame: u64) {
        let p = ui.painter().clone();
        p.rect_filled(rect, 0.0, t.ui.panel_bg);
        p.hline(rect.x_range(), rect.bottom() - 0.5, t.border());
        self.bar = Some(rect);
        self.titles.clear();
        let mut hover = None;
        let mut x = rect.left() + 6.0;
        for (i, (title, _)) in MENUS.iter().enumerate() {
            let w = ui.painter().layout_no_wrap((*title).to_owned(), t.font_ui(), t.ui.text).size().x + 20.0;
            let r = Rect::from_min_size(pos2(x, rect.top()), vec2(w.round(), rect.height() - 1.0));
            self.titles.push(r);
            x = r.right();
            let resp = ui.interact(r, title_id(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            let open = self.open == Some(i);
            if open || (resp.hovered() && self.open.is_none()) {
                p.rect_filled(r.shrink2(vec2(0.0, 3.0)), 4.0, if open { t.ui.row_selected } else { t.ui.row_hover });
            }
            p.text(r.center(), egui::Align2::CENTER_CENTER, *title, t.font_ui(), if open { t.ui.text_strong } else { t.ui.text });
            if resp.clicked() {
                if open {
                    self.close();
                } else {
                    self.open_menu(i, frame);
                }
            } else if resp.hovered() && self.open.is_some() && !open && self.hover != Some(i) {
                self.open_menu(i, frame);
            }
            if resp.hovered() {
                hover = Some(i);
            }
        }
        self.hover = hover;
    }

    /// Draw the open menu and handle its keys. Returns what was picked.
    pub fn popup(&mut self, ctx: &egui::Context, t: &Theme, mac: bool, enabled: &dyn Fn(Cmd) -> bool, recent: &[String], frame: u64) -> Option<MenuPick> {
        let m = self.open?;
        let list = entries(m, mac, enabled, !recent.is_empty());
        let rlist = recent_entries(recent);
        let recent_at = MENUS.get(m).and_then(|(_, items)| items.iter().position(|x| *x == MenuItem::Recent));
        let [up, down, left, right, enter, esc] = widgets::menu_keys(ctx);
        let n_menus = MENUS.len();
        let mut pick = None;
        if self.sub_open {
            if down {
                self.sub_sel = widgets::menu_step(&rlist, self.sub_sel, 1);
            }
            if up {
                self.sub_sel = widgets::menu_step(&rlist, self.sub_sel, -1);
            }
            if left || esc {
                self.sub_open = false;
                self.sub_sel = None;
            }
            if enter {
                if let Some(r) = self.sub_sel.and_then(|i| recent.get(i)) {
                    pick = Some(MenuPick::Recent(r.clone()));
                }
            }
        } else {
            if down {
                self.sel = widgets::menu_step(&list, self.sel, 1);
            }
            if up {
                self.sel = widgets::menu_step(&list, self.sel, -1);
            }
            let on_recent = self.sel.is_some() && self.sel == recent_at;
            if right {
                if on_recent && !rlist.is_empty() {
                    self.sub_open = true;
                    self.sub_sel = Some(0);
                } else {
                    self.open_menu((m + 1) % n_menus, frame);
                    return None;
                }
            }
            if left {
                self.open_menu((m + n_menus - 1) % n_menus, frame);
                return None;
            }
            if esc {
                self.close();
                return None;
            }
            if enter {
                if on_recent {
                    if !rlist.is_empty() {
                        self.sub_open = true;
                        self.sub_sel = Some(0);
                    }
                } else if let Some(i) = self.sel {
                    pick = item_pick(m, i, &list);
                }
            }
        }
        let pos = self.titles.get(m).map_or(Pos2::ZERO, |r| pos2(r.left(), r.bottom()));
        let resp = widgets::menu_popup(ctx, popup_id(m), pos, &list, self.sel, t);
        self.popup = Some(resp.rect);
        if let Some(h) = resp.hovered {
            if self.sel != Some(h) && list.get(h).is_some_and(|e| e.enabled) {
                self.sel = Some(h);
                if Some(h) == recent_at {
                    self.sub_open = !rlist.is_empty();
                } else {
                    self.sub_open = false;
                    self.sub_sel = None;
                }
            }
        }
        if let Some(i) = resp.clicked {
            if Some(i) == recent_at {
                self.sub_open = !rlist.is_empty();
            } else {
                pick = item_pick(m, i, &list);
            }
        }
        self.sub = None;
        if self.sub_open {
            if let Some(row) = recent_at.and_then(|i| resp.rows.get(i)) {
                let sr = widgets::menu_popup(ctx, recent_id(), pos2(resp.rect.right() - 4.0, row.top() - 4.0), &rlist, self.sub_sel, t);
                self.sub = Some(sr.rect);
                if let Some(h) = sr.hovered {
                    self.sub_sel = Some(h);
                }
                if let Some(i) = sr.clicked {
                    pick = recent.get(i).cloned().map(MenuPick::Recent);
                }
            }
        }
        // A press outside the bar and the menus closes.
        if frame > self.opened_frame {
            let outside = ctx.input(|i| {
                i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| ![self.bar, self.popup, self.sub].iter().flatten().any(|r| r.contains(p)))
            });
            if outside {
                self.close();
                return None;
            }
        }
        if pick.is_some() {
            self.close();
        }
        pick
    }
}

fn item_pick(m: usize, i: usize, list: &[MenuEntry]) -> Option<MenuPick> {
    if !list.get(i).is_some_and(|e| e.enabled && !e.sep) {
        return None;
    }
    match MENUS.get(m)?.1.get(i)? {
        MenuItem::Cmd(c) => Some(MenuPick::Cmd(*c)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_follow_the_command_table() {
        let all = |_: Cmd| true;
        let file = entries(0, true, &all, false);
        assert_eq!(file[0].label, "New File");
        assert_eq!(file[0].shortcut, "⌘N");
        let recent = file.iter().find(|e| e.submenu).unwrap();
        assert!(!recent.enabled, "no recent folders: disabled");
        let none = |c: Cmd| c != Cmd::Save;
        let file = entries(0, false, &none, true);
        let save = file.iter().find(|e| e.label == "Save").unwrap();
        assert!(!save.enabled && save.shortcut == "Ctrl+S");
        assert_eq!(item_pick(0, 0, &file), Some(MenuPick::Cmd(Cmd::NewFile)));
        let si = file.iter().position(|e| e.label == "Save").unwrap();
        assert_eq!(item_pick(0, si, &file), None, "disabled");
        assert_eq!(widgets::menu_step(&file, None, 1), Some(0));
        assert_eq!(widgets::menu_step(&file, Some(0), -1), Some(file.len() - 1), "wraps, skips separators");
    }
}
