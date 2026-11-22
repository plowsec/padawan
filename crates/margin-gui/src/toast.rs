//! Toasts: bottom right, above the status bar. Notices fade after 4 s; errors stay until clicked.
//! At most 3 visible; text sanitised and capped.

use egui::{pos2, vec2, CursorIcon, Id, Order, Rect, Sense};
use margin_theme::Theme;

pub const LIFETIME: f64 = 4.0;
const MAX_SHOWN: usize = 3;
const MAX_QUEUE: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub error: bool,
    /// egui time it was shown first.
    pub at: Option<f64>,
}

#[derive(Default)]
pub struct Toasts {
    pub list: Vec<Toast>,
}

impl Toasts {
    pub fn notice(&mut self, text: &str) {
        self.push(text, false);
    }

    pub fn error(&mut self, text: &str) {
        self.push(text, true);
    }

    fn push(&mut self, text: &str, error: bool) {
        let text = crate::sanitize::sanitize(text, 240);
        if self.list.iter().any(|t| t.text == text) {
            return;
        }
        self.list.push(Toast { text, error, at: None });
        if self.list.len() > MAX_QUEUE {
            self.list.remove(0);
        }
    }

    /// Draw above `bottom` (status bar top), right-aligned at `right`.
    pub fn show(&mut self, ctx: &egui::Context, t: &Theme, right: f32, bottom: f32, now: f64) {
        self.list.retain(|x| x.error || x.at.is_none_or(|a| now - a < LIFETIME));
        let mut y = bottom - 10.0;
        let mut remove = None;
        let mut next_wake: Option<f64> = None;
        let n = self.list.len();
        for i in (n.saturating_sub(MAX_SHOWN)..n).rev() {
            let toast = &mut self.list[i];
            let at = *toast.at.get_or_insert(now);
            if !toast.error {
                let left = LIFETIME - (now - at);
                next_wake = Some(next_wake.map_or(left, |w: f64| w.min(left)));
            }
            let font = t.font_small();
            let g = ctx.fonts_mut(|f| f.layout(toast.text.clone(), font.clone(), t.ui.text_strong, 340.0));
            let size = vec2(g.size().x.max(120.0) + 24.0, g.size().y + 16.0);
            let rect = Rect::from_min_size(pos2(right - 12.0 - size.x, y - size.y), size);
            y = rect.top() - 8.0;
            let err = toast.error;
            egui::Area::new(Id::unique(("toast", i))).order(Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
                let resp = ui.interact(rect, Id::unique(("toast-hit", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                let p = ui.painter();
                p.add(ctx.global_style().visuals.popup_shadow.as_shape(rect, 5));
                p.rect_filled(rect, 5.0, t.ui.overlay_bg);
                p.rect_stroke(rect, 5.0, t.border(), egui::StrokeKind::Inside);
                if err {
                    p.rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(2.0, rect.height() - 12.0)), 1.0, t.ui.error);
                }
                p.galley(rect.min + vec2(12.0, 8.0), g, t.ui.text_strong);
                if resp.clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.list.remove(i);
            ctx.request_repaint();
        }
        if let Some(w) = next_wake {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(w.max(0.0) + 0.05));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_cap_sanitise() {
        let mut t = Toasts::default();
        t.error("bad\u{202e}thing\n");
        t.error("bad\u{202e}thing\n");
        assert_eq!(t.list.len(), 1);
        assert_eq!(t.list[0].text, "bad·thing·");
        for i in 0..40 {
            t.notice(&format!("n{i}"));
        }
        assert_eq!(t.list.len(), MAX_QUEUE);
        t.notice(&"x".repeat(10_000));
        assert!(t.list.last().unwrap().text.chars().count() <= 240);
    }
}
