//! Settings tab. One setting per row: label (+ muted description) left, control right.
//! Changes apply on the next frame; the app saves 500 ms after the last change.

use egui::{pos2, vec2, CornerRadius, CursorIcon, Id, Rect, Sense, Stroke, Ui};
use margin_editor::Wrap;
use margin_theme::{Accent, Look, Mode, Theme};

use crate::commands;
use crate::settings::{Settings, ZOOM};
use crate::widgets;

#[derive(Default)]
pub struct PaneOut {
    pub changed: bool,
    /// Index options changed: send `Reindex`.
    pub reindex: bool,
}

const COL: f32 = 640.0;

struct Rows<'a> {
    t: &'a Theme,
    left: f32,
    width: f32,
}

impl Rows<'_> {
    fn heading(&self, ui: &mut Ui, text: &str) {
        ui.add_space(18.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), self.t.row_h()), Sense::hover());
        widgets::text_at(ui, r, self.left, text, self.t.font_small(), self.t.ui.text_muted);
        ui.painter().hline(self.left..=self.left + self.width, r.bottom() - 0.5, self.t.border());
        ui.add_space(4.0);
    }

    /// A row; returns the control rect (right side, `cw` wide).
    fn row(&self, ui: &mut Ui, label: &str, desc: &str, cw: f32) -> Rect {
        let t = self.t;
        let h = if desc.is_empty() { t.row_h() + 10.0 } else { t.row_h() * 1.75 + 6.0 };
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h.round()), Sense::hover());
        let lr = Rect::from_min_max(pos2(self.left, r.top()), pos2(self.left + self.width - cw - 16.0, r.bottom()));
        if desc.is_empty() {
            widgets::text_at(ui, lr, lr.left(), label, t.font_ui(), t.ui.text);
        } else {
            let top = Rect::from_min_max(lr.min, pos2(lr.right(), lr.center().y + 1.0));
            let bot = Rect::from_min_max(pos2(lr.left(), lr.center().y + 1.0), lr.max);
            widgets::text_at(ui, top.translate(vec2(0.0, 2.0)), lr.left(), label, t.font_ui(), t.ui.text);
            widgets::text_at(ui, bot.translate(vec2(0.0, -2.0)), lr.left(), desc, t.font_small(), t.ui.text_muted);
        }
        let ch = (t.row_h() + 2.0).round();
        Rect::from_min_size(pos2(self.left + self.width - cw, r.center().y - ch / 2.0), vec2(cw, ch))
    }
}

fn switch(ui: &mut Ui, rect: Rect, on: &mut bool, t: &Theme, id: Id) -> bool {
    let r = Rect::from_center_size(pos2(rect.right() - 15.0, rect.center().y), vec2(30.0, 16.0));
    let resp = ui.interact(r.expand(4.0), id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let p = ui.painter();
    // Neutral, not the accent: the accent is kept for focus and selection.
    let track = if *on { margin_theme::mix(t.ui.row_selected, t.ui.text, 0.6) } else { t.ui.row_selected };
    p.rect_filled(r, CornerRadius::same(8), track);
    if !*on {
        p.rect_stroke(r, CornerRadius::same(8), t.border(), egui::StrokeKind::Inside);
    }
    let kx = if *on { r.right() - 8.0 } else { r.left() + 8.0 };
    p.circle_filled(pos2(kx, r.center().y), 5.5, if *on { t.code.bg } else { t.ui.text_muted });
    if resp.clicked() {
        *on = !*on;
        return true;
    }
    false
}

/// `−  value  +` stepper. Returns true when changed.
#[allow(clippy::too_many_arguments)]
fn stepper(ui: &mut Ui, rect: Rect, v: &mut f32, step: f32, range: std::ops::RangeInclusive<f32>, fmt: &dyn Fn(f32) -> String, t: &Theme, id: Id) -> bool {
    let p = ui.painter().clone();
    p.rect_stroke(rect, CornerRadius::same(4), t.border(), egui::StrokeKind::Inside);
    let bw = rect.height();
    let minus = Rect::from_min_size(rect.min, vec2(bw, rect.height()));
    let plus = Rect::from_min_size(pos2(rect.right() - bw, rect.top()), vec2(bw, rect.height()));
    let mut changed = false;
    for (r, d, k) in [(minus, -step, 0), (plus, step, 1)] {
        let resp = ui.interact(r, id.with(k), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if resp.hovered() {
            p.rect_filled(r.shrink(2.0), CornerRadius::same(3), t.ui.row_hover);
        }
        let c = r.center();
        let col = if resp.hovered() { t.ui.text_strong } else { t.ui.text_muted };
        p.hline(c.x - 4.0..=c.x + 4.0, c.y, Stroke::new(1.3, col));
        if k == 1 {
            p.vline(c.x, c.y - 4.0..=c.y + 4.0, Stroke::new(1.3, col));
        }
        if resp.clicked() {
            let nv = ((*v + d) / step).round() * step;
            let nv = nv.clamp(*range.start(), *range.end());
            if (nv - *v).abs() > f32::EPSILON {
                *v = nv;
                changed = true;
            }
        }
    }
    p.text(rect.center(), egui::Align2::CENTER_CENTER, fmt(*v), t.font_mono_ui(), t.ui.text_strong);
    changed
}

pub fn show(ui: &mut Ui, rect: Rect, s: &mut Settings, t: &Theme, mac: bool) -> PaneOut {
    let mut out = PaneOut::default();
    ui.painter().rect_filled(rect, 0.0, t.code.bg);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    egui::ScrollArea::vertical().id_salt("settings-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let width = COL.min(ui.available_width() - 64.0).max(240.0);
        let left = ui.max_rect().left() + 40.0;
        let rows = Rows { t, left, width };
        ui.add_space(24.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        widgets::text_at(ui, r, left, "Settings", egui::FontId::proportional(t.look.ui_size + 5.0), t.ui.text_strong);

        let mut ch = false;
        let mut idx = false;
        let id = Id::unique("settings");

        rows.heading(ui, "Appearance");
        let c = rows.row(ui, "Theme", "", 150.0);
        if let Some(i) = widgets::segmented(ui, c, &["Dark", "Light"], usize::from(s.mode == Mode::Light), t, id.with("theme")) {
            s.mode = if i == 1 { Mode::Light } else { Mode::Dark };
            ch = true;
        }
        let c = rows.row(ui, "Show menu bar", "", 40.0);
        ch |= switch(ui, c, &mut s.menu_bar, t, id.with("menubar"));
        let c = rows.row(ui, "Accent", "", 5.0 * 26.0);
        for (i, a) in Accent::ALL.into_iter().enumerate() {
            let center = pos2(c.left() + 13.0 + 26.0 * i as f32, c.center().y);
            let hit = Rect::from_center_size(center, vec2(24.0, 24.0));
            let resp = ui.interact(hit, id.with(("accent", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(a.name());
            let p = ui.painter();
            if s.look.accent == a {
                p.circle_stroke(center, 10.0, Stroke::new(1.5, t.ui.text_strong));
            } else if resp.hovered() {
                p.circle_stroke(center, 10.0, Stroke::new(1.0, t.ui.border));
            }
            p.circle_filled(center, 7.0, a.color(t.mode));
            if resp.clicked() && s.look.accent != a {
                s.look.accent = a;
                ch = true;
            }
        }
        let pt = |v: f32| format!("{v:.0} pt");
        let pt_half = |v: f32| if v.fract() == 0.0 { format!("{v:.0} pt") } else { format!("{v:.1} pt") };
        let c = rows.row(ui, "Interface font size", "", 120.0);
        ch |= stepper(ui, c, &mut s.look.ui_size, 1.0, Look::UI_SIZE, &pt, t, id.with("ui_size"));
        let c = rows.row(ui, "Editor font size", "", 120.0);
        ch |= stepper(ui, c, &mut s.look.code_size, 0.5, Look::CODE_SIZE, &pt_half, t, id.with("code_size"));
        let c = rows.row(ui, "Line height", "", 120.0);
        ch |= stepper(ui, c, &mut s.look.line_height, 0.1, Look::LINE_HEIGHT, &|v| format!("{v:.1}"), t, id.with("line_height"));
        let c = rows.row(ui, "Zoom", &format!("Also {} / {}", commands::shortcut(commands::Cmd::ZoomIn, mac), commands::shortcut(commands::Cmd::ZoomOut, mac)), 120.0);
        ch |= stepper(ui, c, &mut s.zoom, 0.1, ZOOM, &|v| format!("{:.0}%", v * 100.0), t, id.with("zoom"));

        rows.heading(ui, "Editor");
        let c = rows.row(ui, "Tab width", "", 120.0);
        let mut tw = s.tab_width as f32;
        if stepper(ui, c, &mut tw, 1.0, 1.0..=16.0, &|v| format!("{v:.0}"), t, id.with("tab_width")) {
            s.tab_width = tw as u32;
            ch = true;
        }
        let c = rows.row(ui, "Insert spaces for tabs", "", 40.0);
        ch |= switch(ui, c, &mut s.soft_tabs, t, id.with("soft_tabs"));
        let c = rows.row(ui, "Word wrap", "Auto wraps prose (Markdown, plain text), not code", 180.0);
        let cur = match s.wrap {
            Wrap::Auto => 0,
            Wrap::On => 1,
            Wrap::Off => 2,
        };
        if let Some(i) = widgets::segmented(ui, c, &["Auto", "On", "Off"], cur, t, id.with("wrap")) {
            s.wrap = [Wrap::Auto, Wrap::On, Wrap::Off][i];
            ch = true;
        }
        for (label, desc, v, k) in [
            ("Line numbers", "", &mut s.line_numbers, "ln"),
            ("Highlight current line", "", &mut s.highlight_line, "hl"),
            ("Show whitespace", "", &mut s.show_whitespace, "ws"),
            ("Auto-indent", "Enter keeps the indentation; Markdown lists continue", &mut s.auto_indent, "autoindent"),
            ("Code hints", "Callers, callees and sinks above each function", &mut s.code_hints, "hints"),
            ("Save when the window loses focus", "", &mut s.autosave, "autosave"),
            ("Reopen tabs with the folder", "", &mut s.restore_tabs, "restore"),
            ("Open preview with Markdown files", "Editor and preview side by side", &mut s.md_preview, "mdpreview"),
        ] {
            let c = rows.row(ui, label, desc, 40.0);
            ch |= switch(ui, c, v, t, id.with(k));
        }

        rows.heading(ui, "Files and index");
        for (label, desc, v, k) in [
            ("Respect .gitignore", "Ignored files stay in the tree, greyed, and are not searched", &mut s.respect_gitignore, "gi"),
            ("Index hidden files", "Dot-files and dot-directories (.git is always skipped)", &mut s.index_hidden, "hidden"),
        ] {
            let c = rows.row(ui, label, desc, 40.0);
            if switch(ui, c, v, t, id.with(k)) {
                ch = true;
                idx = true;
            }
        }
        let c = rows.row(ui, "Largest indexed file", "Bigger files are listed, not searched", 120.0);
        let mut mb = s.max_file_kb as f32 / 1024.0;
        let step = if mb < 1.0 { 0.25 } else { 1.0 };
        if stepper(ui, c, &mut mb, step, 0.25..=64.0, &|v| if v < 1.0 { format!("{:.0} KB", v * 1024.0) } else { format!("{v:.0} MB") }, t, id.with("maxfile")) {
            s.max_file_kb = (mb * 1024.0).round() as u32;
            ch = true;
            idx = true;
        }

        rows.heading(ui, "Keyboard shortcuts");
        for d in commands::ALL {
            let keys: Vec<String> = commands::chords(d, mac).iter().map(|c| commands::label(*c, mac)).collect();
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), t.row_h()), Sense::hover());
            let row = Rect::from_min_max(pos2(left, r.top()), pos2(left + width, r.bottom()));
            widgets::text_at(ui, row, left, d.title, t.font_ui(), t.ui.text);
            let k = if keys.is_empty() { "—".to_owned() } else { keys.join("   ") };
            widgets::key_hint(ui, row, row.right(), &k, t);
        }
        ui.add_space(24.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), t.row_h() + 4.0), Sense::hover());
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(left, r.top()), pos2(left + width, r.bottom()))));
        if widgets::quiet_button(&mut b, "Reset to defaults", t).clicked() {
            let d = Settings::default();
            idx |= d.index_opts() != s.index_opts();
            *s = d;
            ch = true;
        }
        ui.add_space(40.0);
        out.changed = ch;
        out.reindex = idx;
    });
    out
}
