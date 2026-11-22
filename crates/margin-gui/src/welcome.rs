//! Welcome screen (no folder) and the empty-editor hints (folder, no tab). Calm, centred, no copy.

use egui::{pos2, vec2, CursorIcon, Id, Rect, Sense, Ui};
use margin_theme::Theme;

use crate::commands::{shortcut, Cmd};
use crate::sanitize::clean;
use crate::widgets;

pub enum WelcomeOut {
    OpenFolder,
    Recent(String),
}

const SHORTCUTS: &[(Cmd, &str)] = &[
    (Cmd::Palette, "Command palette"),
    (Cmd::NewFile, "New file"),
    (Cmd::GoToFile, "Go to file"),
    (Cmd::SearchFolder, "Search in folder"),
    (Cmd::ToggleSidebar, "Toggle sidebar"),
    (Cmd::Settings, "Settings"),
];

/// `error`: why the last open failed (sanitised here).
pub fn show(ui: &mut Ui, rect: Rect, t: &Theme, mac: bool, recent: &[String], error: Option<&str>) -> Option<WelcomeOut> {
    let mut out = None;
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.code.bg);
    let w = 420.0f32.min(rect.width() - 48.0);
    let row_h = t.row_h();
    let n_recent = recent.len().min(8);
    let block_h = 44.0 + 18.0 + 36.0 + 28.0 + if n_recent > 0 { 24.0 + n_recent as f32 * (row_h + 4.0) + 20.0 } else { 0.0 } + 24.0 + SHORTCUTS.len() as f32 * row_h + if error.is_some() { 32.0 } else { 0.0 };
    let top = (rect.center().y - block_h / 2.0).max(rect.top() + 32.0).round();
    let left = (rect.center().x - w / 2.0).round();
    let col = Rect::from_min_size(pos2(left, top), vec2(w, block_h));
    let mut y = col.top();
    let title = Rect::from_min_size(pos2(left, y), vec2(w, 44.0));
    widgets::text_at(ui, title, left, "Margin", egui::FontId::proportional(t.look.ui_size * 2.2), t.ui.text_strong);
    y += 44.0 + 18.0;
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(left, y), vec2(w, 36.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    if widgets::primary_button(&mut b, "Open Folder…", t).clicked() {
        out = Some(WelcomeOut::OpenFolder);
    }
    b.add_space(12.0);
    let kr = Rect::from_min_size(b.cursor().min, vec2(80.0, b.max_rect().height()));
    widgets::text_at(&b, kr, kr.left(), &shortcut(Cmd::OpenFolder, mac), egui::FontId::monospace((t.look.ui_size - 2.0).max(9.0)), t.ui.text_muted);
    y += 36.0;
    if let Some(e) = error {
        let r = Rect::from_min_size(pos2(left, y + 8.0), vec2(w, row_h));
        widgets::text_at(ui, r, left, &crate::sanitize::sanitize(e, 160), t.font_small(), t.ui.error);
        y += 32.0;
    }
    y += 28.0;
    if n_recent > 0 {
        let r = Rect::from_min_size(pos2(left, y), vec2(w, 20.0));
        widgets::text_at(ui, r, left, "Recent", t.font_small(), t.ui.text_muted);
        y += 24.0;
        for (i, full) in recent.iter().take(n_recent).enumerate() {
            let r = Rect::from_min_size(pos2(left - 8.0, y), vec2(w + 16.0, row_h + 2.0));
            let resp = ui.interact(r, Id::unique(("welcome-recent", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, t.ui.row_hover);
            }
            let name = full.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(full);
            widgets::paint_job(ui, r.with_max_x(r.right() - 8.0), left, &[(&clean(name), t.font_ui(), t.ui.text_strong), ("   ", t.font_small(), t.ui.text_muted), (&clean(full), t.font_small(), t.ui.text_muted)]);
            if resp.clicked() {
                out = Some(WelcomeOut::Recent(full.clone()));
            }
            y += row_h + 4.0;
        }
        y += 20.0;
    }
    y += 4.0;
    for (cmd, label) in SHORTCUTS {
        let r = Rect::from_min_size(pos2(left, y), vec2(w, row_h));
        widgets::text_at(ui, r, left, label, t.font_small(), t.ui.text_muted);
        widgets::key_hint(ui, r, r.right(), &shortcut(*cmd, mac), t);
        y += row_h;
    }
    out
}

/// Folder open, no tab.
pub fn empty_hints(ui: &mut Ui, rect: Rect, t: &Theme, mac: bool) {
    ui.painter().rect_filled(rect, 0.0, t.code.bg);
    let rows = [(Cmd::GoToFile, "Go to file"), (Cmd::Palette, "Command palette"), (Cmd::SearchFolder, "Search in folder"), (Cmd::ShowFiles, "Find file by name"), (Cmd::NewFile, "New file")];
    let w = 300.0f32.min(rect.width() - 32.0);
    let row_h = t.row_h();
    let mut y = (rect.center().y - rows.len() as f32 * row_h / 2.0).round();
    let left = (rect.center().x - w / 2.0).round();
    for (cmd, label) in rows {
        let r = Rect::from_min_size(pos2(left, y), vec2(w, row_h));
        widgets::text_at(ui, r, left, label, t.font_ui(), t.ui.text_faint);
        widgets::text_right(ui, r, r.right(), &shortcut(cmd, mac), egui::FontId::monospace((t.look.ui_size - 1.5).max(9.0)), t.ui.text_faint);
        y += row_h;
    }
}
