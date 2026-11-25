//! Shared painters. Glyphs (chevron, ×, dots) are drawn with lines and circles, never an icon font.
//!
//! | Fn | Draws |
//! |---|---|
//! | `galley` / `text_at` | one line of text, truncated with `…`, vertically centred |
//! | `job` | runs of differently coloured text (highlights) on one line |
//! | `row_bg` | hover / selected row background |
//! | `chevron`, `close_x`, `dot` | glyphs |
//! | `field` | text input: `input_bg`, 1 px border, accent on focus |
//! | `segmented` | two-or-more segment switch |
//! | `text_toggle` | small `Aa` / `W` / `.*` toggle |
//! | `primary_button`, `quiet_button` | buttons |
//! | `key_hint` | shortcut text, small mono muted |
//! | `virtual_rows` | fixed-height virtualised list in a scroll area |
//! | `menu_popup`, `menu_step`, `menu_keys` | popup menu: menu bar dropdowns and every context menu |
//! | `text_toggle_off` | a disabled toggle |

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, FontId, Galley, Id, Painter, Pos2, Rect, Response, Sense, Stroke, Ui};
use margin_theme::Theme;

/// Install the theme. Cheap when unchanged: called every frame.
pub fn apply_theme(ctx: &egui::Context, t: &Theme) {
    t.apply(ctx);
}

/// One line, truncated at `max_w` with `…`.
pub fn galley(ui: &Ui, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<Galley> {
    let mut j = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    j.wrap = TextWrapping::truncate_at_width(max_w.max(1.0));
    ui.painter().layout_job(j)
}

/// Text left-aligned at `x`, centred in `rect` vertically. Returns its width.
pub fn text_at(ui: &Ui, rect: Rect, x: f32, text: &str, font: FontId, color: Color32) -> f32 {
    let g = galley(ui, text, font, color, rect.right() - x);
    let w = g.size().x;
    let y = rect.center().y - g.size().y / 2.0;
    ui.painter().with_clip_rect(rect).galley(pos2(x, y), g, color);
    w
}

/// Text right-aligned at `right`. Returns its width.
pub fn text_right(ui: &Ui, rect: Rect, right: f32, text: &str, font: FontId, color: Color32) -> f32 {
    let g = galley(ui, text, font, color, rect.width());
    let w = g.size().x;
    let y = rect.center().y - g.size().y / 2.0;
    ui.painter().with_clip_rect(rect).galley(pos2(right - w, y), g, color);
    w
}

/// A line made of runs `(text, font, colour)`, truncated at `max_w`.
pub fn job(ui: &Ui, runs: &[(&str, FontId, Color32)], max_w: f32) -> Arc<Galley> {
    let mut j = LayoutJob::default();
    for (t, f, c) in runs {
        j.append(t, 0.0, TextFormat::simple(f.clone(), *c));
    }
    j.wrap = TextWrapping::truncate_at_width(max_w.max(1.0));
    ui.painter().layout_job(j)
}

pub fn paint_job(ui: &Ui, rect: Rect, x: f32, runs: &[(&str, FontId, Color32)]) -> f32 {
    let g = job(ui, runs, rect.right() - x);
    let w = g.size().x;
    let y = rect.center().y - g.size().y / 2.0;
    ui.painter().with_clip_rect(rect).galley(pos2(x, y), g, Color32::PLACEHOLDER);
    w
}

/// Engine snippet with the match `[col, col+len)` (byte offsets, clamped) highlighted. Leading
/// whitespace dropped; a long prefix is cut to keep the match in view. Every part sanitised.
pub fn snippet(ui: &Ui, rect: Rect, x: f32, text: &str, col: u32, len: u32, t: &Theme) -> f32 {
    use crate::sanitize::{sanitize, split3};
    let (pre, mid, post) = split3(text, col, len);
    let pre = pre.trim_start();
    let n = pre.chars().count();
    // Keep the match in the first third of the row.
    let room = (((rect.right() - x) / (t.font_mono_ui().size * 0.6)) / 3.0).max(8.0) as usize;
    let pre = if n > room {
        let skip = pre.char_indices().nth(n - (room - 1)).map_or(0, |(i, _)| i);
        format!("…{}", sanitize(&pre[skip..], room))
    } else {
        sanitize(pre, room + 1)
    };
    let (mid, post) = (sanitize(mid, 200), sanitize(post, 300));
    let font = t.font_mono_ui();
    let mut j = LayoutJob::default();
    j.append(&pre, 0.0, TextFormat::simple(font.clone(), t.ui.text));
    j.append(&mid, 0.0, TextFormat { background: t.code.find_match, ..TextFormat::simple(font.clone(), t.ui.text_strong) });
    j.append(&post, 0.0, TextFormat::simple(font, t.ui.text));
    j.wrap = TextWrapping::truncate_at_width((rect.right() - x).max(1.0));
    let g = ui.painter().layout_job(j);
    let w = g.size().x;
    let y = rect.center().y - g.size().y / 2.0;
    ui.painter().with_clip_rect(rect).galley(pos2(x, y), g, t.ui.text);
    w
}

pub fn row_bg(p: &Painter, rect: Rect, t: &Theme, hovered: bool, selected: bool) {
    if selected {
        p.rect_filled(rect, 0.0, t.ui.row_selected);
    } else if hovered {
        p.rect_filled(rect, 0.0, t.ui.row_hover);
    }
}

/// `>` pointing right, or `v` when open. `s` = half size.
pub fn chevron(p: &Painter, c: Pos2, open: bool, color: Color32) {
    let s = 3.0;
    let stroke = Stroke::new(1.3, color);
    let pts = if open {
        vec![pos2(c.x - s, c.y - s * 0.5), pos2(c.x, c.y + s * 0.5), pos2(c.x + s, c.y - s * 0.5)]
    } else {
        vec![pos2(c.x - s * 0.5, c.y - s), pos2(c.x + s * 0.5, c.y), pos2(c.x - s * 0.5, c.y + s)]
    };
    p.line(pts, stroke);
}

pub fn close_x(p: &Painter, c: Pos2, s: f32, color: Color32) {
    let st = Stroke::new(1.3, color);
    p.line_segment([pos2(c.x - s, c.y - s), pos2(c.x + s, c.y + s)], st);
    p.line_segment([pos2(c.x - s, c.y + s), pos2(c.x + s, c.y - s)], st);
}

/// Two panes side by side.
pub fn split_icon(p: &Painter, c: Pos2, color: Color32) {
    let r = Rect::from_center_size(c, egui::vec2(13.0, 11.0));
    let st = Stroke::new(1.2, color);
    p.rect_stroke(r, CornerRadius::same(2), st, egui::StrokeKind::Inside);
    p.line_segment([pos2(c.x, r.top() + 1.0), pos2(c.x, r.bottom() - 1.0)], st);
}

pub fn dot(p: &Painter, c: Pos2, r: f32, color: Color32) {
    p.circle_filled(c, r, color);
}

/// Single-line text input in `rect`. Paints its own frame.
pub fn field(ui: &mut Ui, rect: Rect, id: Id, text: &mut String, hint: &str, t: &Theme, font: FontId) -> Response {
    field_pad(ui, rect, id, text, hint, t, font, 0.0)
}

/// `field` leaving `right_pad` pt free inside the box (for toggles).
#[allow(clippy::too_many_arguments)]
pub fn field_pad(ui: &mut Ui, rect: Rect, id: Id, text: &mut String, hint: &str, t: &Theme, font: FontId, right_pad: f32) -> Response {
    let focused = ui.memory(|m| m.has_focus(id));
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(4), t.ui.input_bg);
    p.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, if focused { t.ui.accent } else { t.ui.border }), egui::StrokeKind::Inside);
    let inner = Rect::from_min_max(pos2(rect.left() + 8.0, rect.top()), pos2(rect.right() - 8.0 - right_pad, rect.bottom()));
    let edit = egui::TextEdit::singleline(text)
        .id(id)
        .font(font)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .hint_text(egui::RichText::new(hint).color(t.ui.text_faint))
        .text_color(t.ui.text_strong)
        .desired_width(inner.width())
        .align(egui::Align2::LEFT_CENTER);
    ui.put(inner, edit)
}

/// Frameless input (overlay headers: the overlay is the frame).
pub fn field_bare(ui: &mut Ui, rect: Rect, id: Id, text: &mut String, hint: &str, t: &Theme, font: FontId) -> Response {
    let edit = egui::TextEdit::singleline(text)
        .id(id)
        .font(font)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .hint_text(egui::RichText::new(hint).color(t.ui.text_faint))
        .text_color(t.ui.text_strong)
        .desired_width(rect.width())
        .align(egui::Align2::LEFT_CENTER);
    ui.put(rect, edit)
}

/// Put the text cursor of field `id` at the end of `text`.
pub fn cursor_to_end(ctx: &egui::Context, id: Id, text: &str) {
    let mut st = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
    let end = egui::text::CCursor::new(text.chars().count());
    st.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
    st.store(ctx, id);
}

/// Segment switch; returns the clicked index.
pub fn segmented(ui: &mut Ui, rect: Rect, labels: &[&str], selected: usize, t: &Theme, id: Id) -> Option<usize> {
    let p = ui.painter().clone();
    p.rect_stroke(rect, CornerRadius::same(4), t.border(), egui::StrokeKind::Inside);
    let w = rect.width() / labels.len().max(1) as f32;
    let mut out = None;
    for (i, l) in labels.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + w * i as f32, rect.top()), vec2(w, rect.height()));
        let resp = ui.interact(r, id.with(i), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if i == selected {
            p.rect_filled(r.shrink(2.0), CornerRadius::same(3), t.ui.row_selected);
        } else if resp.hovered() {
            p.rect_filled(r.shrink(2.0), CornerRadius::same(3), t.ui.row_hover);
        }
        let color = if i == selected { t.ui.text_strong } else { t.ui.text_muted };
        p.text(r.center(), Align2::CENTER_CENTER, *l, t.font_small(), color);
        if resp.clicked() {
            out = Some(i);
        }
    }
    out
}

/// Small text toggle (`Aa`, `W`, `.*`). Flips `on` when clicked.
pub fn text_toggle(ui: &mut Ui, rect: Rect, label: &str, on: &mut bool, tip: &str, t: &Theme, id: Id) -> bool {
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip);
    let p = ui.painter();
    if *on {
        p.rect_filled(rect, CornerRadius::same(3), margin_theme::mix(t.ui.input_bg, t.ui.accent, 0.22));
        p.rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.0, margin_theme::mix(t.ui.input_bg, t.ui.accent, 0.6)), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(3), t.ui.row_hover);
    }
    let color = if *on { t.ui.text_strong } else { t.ui.text_muted };
    p.text(rect.center(), Align2::CENTER_CENTER, label, t.font_mono_ui(), color);
    if resp.clicked() {
        *on = !*on;
        return true;
    }
    false
}

/// `text_toggle` drawn disabled (no interaction).
pub fn text_toggle_off(ui: &Ui, rect: Rect, label: &str, t: &Theme) {
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, t.font_mono_ui(), t.ui.text_faint);
}

fn button(ui: &mut Ui, label: &str, t: &Theme, primary: bool) -> Response {
    let font = t.font_ui();
    let g = ui.painter().layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let size = vec2(g.size().x + 24.0, (t.row_h() + 2.0).round());
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let p = ui.painter();
    let (bg, fg, stroke) = if primary {
        let bg = if resp.hovered() { margin_theme::mix(t.ui.accent, t.ui.text_strong, 0.12) } else { t.ui.accent };
        (bg, t.ui.on_accent, Stroke::NONE)
    } else {
        let bg = if resp.hovered() { t.ui.row_selected } else { t.ui.row_hover };
        (bg, t.ui.text_strong, t.border())
    };
    p.rect_filled(rect, CornerRadius::same(5), bg);
    if stroke != Stroke::NONE {
        p.rect_stroke(rect, CornerRadius::same(5), stroke, egui::StrokeKind::Inside);
    }
    if resp.has_focus() {
        p.rect_stroke(rect.expand(2.0), CornerRadius::same(6), Stroke::new(1.0, t.ui.accent), egui::StrokeKind::Outside);
    }
    p.galley(rect.center() - g.size() / 2.0, g, fg);
    resp
}

pub fn primary_button(ui: &mut Ui, label: &str, t: &Theme) -> Response {
    button(ui, label, t, true)
}

pub fn quiet_button(ui: &mut Ui, label: &str, t: &Theme) -> Response {
    button(ui, label, t, false)
}

/// Small link-like text button (banners): text only, underline-free, accent on hover.
pub fn text_button(ui: &mut Ui, label: &str, t: &Theme) -> Response {
    let g = ui.painter().layout_no_wrap(label.to_owned(), t.font_small(), Color32::PLACEHOLDER);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 14.0, g.size().y + 6.0), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    let p = ui.painter();
    p.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, if resp.hovered() { t.ui.text_muted } else { t.ui.border }), egui::StrokeKind::Inside);
    p.galley(rect.center() - g.size() / 2.0, g, t.ui.text_strong);
    resp
}

/// Shortcut text, right-aligned at `right`.
pub fn key_hint(ui: &Ui, rect: Rect, right: f32, text: &str, t: &Theme) -> f32 {
    text_right(ui, rect, right, text, FontId::monospace((t.look.ui_size - 2.0).max(9.0)), t.ui.text_muted)
}

/// Fixed-height virtual list filling `ui`. Calls `row(ui, index, rect)` for visible rows only.
/// `reveal`: scroll that row into view this frame.
pub fn virtual_rows(ui: &mut Ui, id: Id, n: usize, row_h: f32, reveal: Option<usize>, mut row: impl FnMut(&mut Ui, usize, Rect)) {
    egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show_viewport(ui, |ui, vp| {
        let w = ui.available_width();
        ui.set_height(row_h * n as f32);
        let top = ui.max_rect().top();
        let left = ui.max_rect().left();
        if let Some(i) = reveal.filter(|&i| i < n) {
            let r = Rect::from_min_size(pos2(left, top + row_h * i as f32), vec2(w, row_h));
            let visible = Rect::from_min_max(pos2(left, top + vp.min.y), pos2(left + w, top + vp.max.y));
            if !visible.contains_rect(r) {
                ui.scroll_to_rect(r, None);
            }
        }
        let first = (vp.min.y / row_h).floor().max(0.0) as usize;
        let last = ((vp.max.y / row_h).ceil().max(0.0) as usize + 1).min(n);
        for i in first..last {
            let r = Rect::from_min_size(pos2(left, top + row_h * i as f32), vec2(w, row_h));
            row(ui, i, r);
        }
    });
}

// ---- menus (menu bar dropdowns and every context menu) ----

/// One row of a popup menu.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuEntry {
    pub label: String,
    /// Right-aligned, muted mono.
    pub shortcut: String,
    pub enabled: bool,
    /// Separator line (other fields ignored).
    pub sep: bool,
    /// Opens a submenu (`▸`).
    pub submenu: bool,
}

impl MenuEntry {
    pub fn item(label: impl Into<String>, shortcut: impl Into<String>, enabled: bool) -> MenuEntry {
        MenuEntry { label: label.into(), shortcut: shortcut.into(), enabled, sep: false, submenu: false }
    }

    pub fn sep() -> MenuEntry {
        MenuEntry { sep: true, ..Default::default() }
    }

    fn pickable(&self) -> bool {
        !self.sep && self.enabled
    }
}

pub struct MenuResp {
    pub rect: Rect,
    pub clicked: Option<usize>,
    pub hovered: Option<usize>,
    /// Rect of each entry (submenu placement).
    pub rows: Vec<Rect>,
}

const MENU_SEP_H: f32 = 9.0;
const MENU_PAD: f32 = 4.0;

/// Size of a popup for `entries`.
pub fn menu_size(ctx: &egui::Context, entries: &[MenuEntry], t: &Theme) -> egui::Vec2 {
    let mut w: f32 = 180.0;
    let mut h = 2.0 * MENU_PAD;
    for e in entries {
        if e.sep {
            h += MENU_SEP_H;
            continue;
        }
        h += t.row_h();
        let lw = ctx.fonts_mut(|f| f.layout_no_wrap(e.label.clone(), t.font_ui(), Color32::PLACEHOLDER).size().x).min(360.0);
        let sw = if e.shortcut.is_empty() { 0.0 } else { ctx.fonts_mut(|f| f.layout_no_wrap(e.shortcut.clone(), key_font(t), Color32::PLACEHOLDER).size().x) + 32.0 };
        w = w.max(lw + sw + 24.0 + if e.submenu { 20.0 } else { 0.0 } + 2.0 * MENU_PAD);
    }
    vec2(w.round(), h.round())
}

fn key_font(t: &Theme) -> FontId {
    FontId::monospace((t.look.ui_size - 2.0).max(9.0))
}

/// Next pickable entry from `from` in direction `d` (wraps). `None` when nothing is pickable.
pub fn menu_step(entries: &[MenuEntry], from: Option<usize>, d: isize) -> Option<usize> {
    let n = entries.len() as isize;
    if n == 0 {
        return None;
    }
    let mut i = from.map_or(if d > 0 { -1 } else { n }, |x| x as isize);
    for _ in 0..n {
        i = (i + d).rem_euclid(n);
        if entries[i as usize].pickable() {
            return Some(i as usize);
        }
    }
    None
}

/// Draw a popup menu with its top-left at `pos` (kept on screen). Entry `i` has widget id `id.with(i)`.
pub fn menu_popup(ctx: &egui::Context, id: Id, pos: Pos2, entries: &[MenuEntry], sel: Option<usize>, t: &Theme) -> MenuResp {
    let size = menu_size(ctx, entries, t);
    let screen = ctx.content_rect();
    let x = pos.x.min(screen.right() - size.x - 4.0).max(screen.left() + 4.0);
    let y = if pos.y + size.y > screen.bottom() - 4.0 { (screen.bottom() - 4.0 - size.y).max(screen.top() + 4.0) } else { pos.y };
    let rect = Rect::from_min_size(pos2(x, y).round(), size);
    let mut out = MenuResp { rect, clicked: None, hovered: None, rows: Vec::with_capacity(entries.len()) };
    egui::Area::new(id).order(egui::Order::Foreground).fade_in(false).fixed_pos(rect.min).show(ctx, |ui| {
        let p = ui.painter().clone();
        p.add(ctx.global_style().visuals.popup_shadow.as_shape(rect, 6));
        p.rect_filled(rect, 6.0, t.ui.overlay_bg);
        p.rect_stroke(rect, 6.0, t.border(), egui::StrokeKind::Inside);
        // Swallow clicks between rows.
        ui.interact(rect, id.with("body"), Sense::click());
        let mut y = rect.top() + MENU_PAD;
        for (i, e) in entries.iter().enumerate() {
            if e.sep {
                let r = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), MENU_SEP_H));
                p.hline(r.left() + 8.0..=r.right() - 8.0, (r.center().y).round() + 0.5, t.border());
                out.rows.push(r);
                y += MENU_SEP_H;
                continue;
            }
            let r = Rect::from_min_size(pos2(rect.left() + MENU_PAD, y), vec2(rect.width() - 2.0 * MENU_PAD, t.row_h()));
            out.rows.push(r);
            y += t.row_h();
            let resp = ui.interact(r, id.with(i), Sense::click());
            let resp = if e.enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
            if resp.hovered() {
                out.hovered = Some(i);
            }
            if e.enabled && (sel == Some(i) || resp.hovered()) {
                p.rect_filled(r, 4.0, t.ui.row_selected);
            }
            let color = if e.enabled { t.ui.text_strong } else { t.ui.text_faint };
            let mut right = r.right() - 10.0;
            if e.submenu {
                chevron(&p, pos2(right - 3.0, r.center().y), false, if e.enabled { t.ui.text_muted } else { t.ui.text_faint });
                right -= 16.0;
            }
            let sw = if e.shortcut.is_empty() { 0.0 } else { text_right(ui, r, right, &e.shortcut, key_font(t), if e.enabled { t.ui.text_muted } else { t.ui.text_faint }) + 24.0 };
            text_at(ui, r.with_max_x(right - sw), r.left() + 10.0, &e.label, t.font_ui(), color);
            if resp.clicked() && e.enabled && !e.sep {
                out.clicked = Some(i);
            }
        }
    });
    out
}

/// The focused list used the arrow keys this frame: stop egui moving focus with them (its lock filter only
/// applies from the second focused frame).
pub fn hold_focus(ui: &Ui) {
    ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
}

/// Keys a menu uses this frame (consumed): up, down, left, right, enter, esc.
pub fn menu_keys(ctx: &egui::Context) -> [bool; 6] {
    ctx.input_mut(|i| {
        let m = egui::Modifiers::NONE;
        [
            i.consume_key(m, egui::Key::ArrowUp),
            i.consume_key(m, egui::Key::ArrowDown),
            i.consume_key(m, egui::Key::ArrowLeft),
            i.consume_key(m, egui::Key::ArrowRight),
            i.consume_key(m, egui::Key::Enter) || i.consume_key(m, egui::Key::Space),
            i.consume_key(m, egui::Key::Escape),
        ]
    })
}

/// 1 px hairline, pixel-aligned.
pub fn hline(p: &Painter, x: std::ops::RangeInclusive<f32>, y: f32, t: &Theme) {
    p.hline(x, y, t.border());
}

pub fn vline(p: &Painter, x: f32, y: std::ops::RangeInclusive<f32>, t: &Theme) {
    p.vline(x, y, t.border());
}
