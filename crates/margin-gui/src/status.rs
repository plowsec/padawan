//! Status bar: one thin row. Left: relative path + notes. Right: position, language, index, sandbox badge.

use egui::{pos2, Rect, Sense, Ui};
use margin_proto::{IndexPhase, IndexStatus};
use margin_theme::Theme;

use crate::search_panel::thousands;
use crate::widgets;

#[derive(Default)]
pub struct StatusInfo {
    pub path: String,
    pub notes: Vec<&'static str>,
    /// 1-based line, 1-based column in characters.
    pub pos: Option<(u32, u32)>,
    pub lang: Option<&'static str>,
    pub index: Option<IndexStatus>,
    pub gui_sandbox: String,
    pub engine_sandbox: String,
}

pub fn height(t: &Theme) -> f32 {
    (t.row_h() - 2.0).round()
}

pub fn sandboxed(gui: &str, engine: &str) -> bool {
    gui != "none" && engine != "none" && !gui.is_empty() && !engine.is_empty()
}

pub fn index_text(s: &IndexStatus) -> String {
    match s.phase {
        IndexPhase::Scanning => "Scanning…".into(),
        IndexPhase::Indexing => format!("Indexing {} / {}", thousands(u64::from(s.files_done)), thousands(u64::from(s.files_total))),
        IndexPhase::Ready => format!("{} {}", thousands(u64::from(s.files_total)), if s.files_total == 1 { "file" } else { "files" }),
    }
}

pub fn show(ui: &mut Ui, rect: Rect, t: &Theme, info: &StatusInfo) {
    let p = ui.painter().clone();
    p.rect_filled(rect, 0.0, t.ui.panel_bg);
    p.hline(rect.x_range(), rect.top() + 0.5, t.border());
    let font = t.font_small();
    let mut right = rect.right() - 12.0;
    let gap = 18.0;
    // Sandbox badge.
    let ok = sandboxed(&info.gui_sandbox, &info.engine_sandbox);
    let (label, color) = if ok { ("sandboxed", t.ui.ok) } else { ("no sandbox", t.ui.warning) };
    let w = widgets::text_right(ui, rect, right, label, font.clone(), color);
    let br = Rect::from_min_max(pos2(right - w - 4.0, rect.top()), pos2(right + 4.0, rect.bottom()));
    ui.interact(br, egui::Id::unique("status-sandbox"), Sense::hover())
        .on_hover_text(format!("Window: {}\nFolder worker: {}", crate::sanitize::sanitize(&info.gui_sandbox, 40), crate::sanitize::sanitize(&info.engine_sandbox, 40)));
    right -= w + gap;
    if let Some(s) = &info.index {
        let txt = index_text(s);
        let w = widgets::text_right(ui, rect, right, &txt, font.clone(), t.ui.text_muted);
        if s.phase == IndexPhase::Indexing && s.files_total > 0 {
            let frac = (s.files_done as f32 / s.files_total as f32).clamp(0.0, 1.0);
            let y = rect.bottom() - 3.0;
            p.hline(right - w..=right, y, egui::Stroke::new(1.0, t.ui.border));
            p.hline(right - w..=right - w + w * frac, y, egui::Stroke::new(1.0, t.ui.accent));
        }
        right -= w + gap;
    }
    if let Some(l) = info.lang {
        right -= widgets::text_right(ui, rect, right, l, font.clone(), t.ui.text_muted) + gap;
    }
    if let Some((l, c)) = info.pos {
        right -= widgets::text_right(ui, rect, right, &format!("Ln {l}, Col {c}"), t.font_mono_ui(), t.ui.text_muted) + gap;
    }
    let left = Rect::from_min_max(rect.min, pos2(right.max(rect.left() + 40.0), rect.bottom()));
    let mut x = rect.left() + 12.0;
    if !info.path.is_empty() {
        x += widgets::text_at(ui, left, x, &info.path, font.clone(), t.ui.text_muted) + 14.0;
    }
    for n in &info.notes {
        x += widgets::text_at(ui, left, x, n, font.clone(), t.ui.warning) + 14.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_texts_and_badge() {
        let mut s = IndexStatus { phase: IndexPhase::Indexing, files_done: 1234, files_total: 10_000, bytes: 0, symbols: 0, skipped: 0, from_cache: 0, elapsed_ms: 0 };
        assert_eq!(index_text(&s), "Indexing 1,234 / 10,000");
        s.phase = IndexPhase::Ready;
        s.files_total = 12_345;
        assert_eq!(index_text(&s), "12,345 files");
        assert!(sandboxed("seatbelt", "seatbelt"));
        assert!(!sandboxed("seatbelt", "none"));
        assert!(!sandboxed("none", "seatbelt"));
    }
}
