//! Look of the app: two palettes (Graphite dark, Paper light), one accent, fonts, egui style.
//!
//! Design rules: flat surfaces, 1 px borders, small radii, no gradients, no glow, one accent colour used
//! sparingly (focus, selection, the active item). Colour carries meaning, not decoration.
//! Widgets read colours from `Theme`, never hard-code them.

pub mod fonts;

use egui::{Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke, TextStyle};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

/// Accent presets. No purple on purpose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Accent {
    #[default]
    Amber,
    Teal,
    Blue,
    Green,
    Rose,
}

impl Accent {
    pub const ALL: [Accent; 5] = [Accent::Amber, Accent::Teal, Accent::Blue, Accent::Green, Accent::Rose];

    pub fn name(self) -> &'static str {
        match self {
            Accent::Amber => "amber",
            Accent::Teal => "teal",
            Accent::Blue => "blue",
            Accent::Green => "green",
            Accent::Rose => "rose",
        }
    }

    pub fn from_name(s: &str) -> Option<Accent> {
        Accent::ALL.into_iter().find(|a| a.name() == s)
    }

    pub fn color(self, mode: Mode) -> Color32 {
        let (d, l) = match self {
            Accent::Amber => (0xe0a850, 0xa5630a),
            Accent::Teal => (0x56b6a4, 0x12766a),
            Accent::Blue => (0x6aa3d8, 0x2c68a8),
            Accent::Green => (0x86b46c, 0x3f7a2a),
            Accent::Rose => (0xd98478, 0xa8433a),
        };
        hex(if mode == Mode::Dark { d } else { l })
    }
}

/// What the settings change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// UI font size, pt.
    pub ui_size: f32,
    /// Editor font size, pt.
    pub code_size: f32,
    /// Editor line height as a multiple of the font size (1.2 – 2.0).
    pub line_height: f32,
    pub accent: Accent,
}

impl Default for Look {
    fn default() -> Self {
        Self { ui_size: 13.0, code_size: 13.0, line_height: 1.5, accent: Accent::Amber }
    }
}

impl Look {
    pub const UI_SIZE: std::ops::RangeInclusive<f32> = 10.0..=20.0;
    pub const CODE_SIZE: std::ops::RangeInclusive<f32> = 8.0..=32.0;
    pub const LINE_HEIGHT: std::ops::RangeInclusive<f32> = 1.2..=2.0;

    pub fn clamped(mut self) -> Self {
        self.ui_size = self.ui_size.clamp(*Self::UI_SIZE.start(), *Self::UI_SIZE.end());
        self.code_size = self.code_size.clamp(*Self::CODE_SIZE.start(), *Self::CODE_SIZE.end());
        self.line_height = self.line_height.clamp(*Self::LINE_HEIGHT.start(), *Self::LINE_HEIGHT.end());
        self
    }
}

/// Chrome colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiColors {
    /// Sidebar, tab strip, status bar.
    pub panel_bg: Color32,
    /// Palette, menus, dialogs.
    pub overlay_bg: Color32,
    /// Text fields.
    pub input_bg: Color32,
    /// 1 px separators and outlines.
    pub border: Color32,
    pub row_hover: Color32,
    pub row_selected: Color32,
    pub text: Color32,
    /// Titles, the active item.
    pub text_strong: Color32,
    /// Secondary text: paths, counts, hints.
    pub text_muted: Color32,
    /// Disabled, placeholders, ignored files.
    pub text_faint: Color32,
    pub accent: Color32,
    /// Text on an accent-filled surface.
    pub on_accent: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub ok: Color32,
    /// Dim layer under a modal overlay.
    pub scrim: Color32,
}

/// Editor surface colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodeColors {
    pub bg: Color32,
    pub text: Color32,
    pub gutter_text: Color32,
    pub gutter_text_active: Color32,
    pub current_line: Color32,
    pub selection: Color32,
    /// Selection when the editor has no focus.
    pub selection_inactive: Color32,
    pub cursor: Color32,
    /// Every find match.
    pub find_match: Color32,
    /// The current find match.
    pub find_current: Color32,
    /// Other occurrences of the word under the cursor.
    pub word_highlight: Color32,
    /// Hint line above a function: text, hovered segment, sink segment.
    pub lens_text: Color32,
    pub lens_hover: Color32,
    pub lens_sink: Color32,
    /// Indent guides, whitespace dots, the wrap marker.
    pub guide: Color32,
    /// Background behind a control or bidi-override character made visible (Trojan Source).
    pub hazard_bg: Color32,
    pub hazard_text: Color32,
    pub scrollbar: Color32,
}

/// One colour per `margin_code::TokenKind` (the editor maps them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntaxColors {
    pub keyword: Color32,
    pub type_: Color32,
    pub function: Color32,
    pub string: Color32,
    pub escape: Color32,
    pub number: Color32,
    pub comment: Color32,
    pub operator: Color32,
    pub punct: Color32,
    pub constant: Color32,
    pub attribute: Color32,
    pub heading: Color32,
    pub emphasis: Color32,
    pub strong: Color32,
    pub link: Color32,
    pub code_span: Color32,
    pub tag: Color32,
    pub key: Color32,
    pub marker: Color32,
    pub inserted: Color32,
    pub deleted: Color32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub mode: Mode,
    pub look: Look,
    pub ui: UiColors,
    pub code: CodeColors,
    pub syn: SyntaxColors,
}

pub const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// `a` moved towards `b` by `t` (0 = a, 1 = b), per channel in gamma space. Opaque.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

impl Default for Theme {
    fn default() -> Self {
        Theme::new(Mode::Dark, Look::default())
    }
}

impl Theme {
    pub fn new(mode: Mode, look: Look) -> Theme {
        let look = look.clamped();
        let accent = look.accent.color(mode);
        match mode {
            Mode::Dark => {
                let bg = hex(0x1c1d1f);
                let ui = UiColors {
                    panel_bg: hex(0x161718),
                    overlay_bg: hex(0x212326),
                    input_bg: hex(0x111213),
                    border: hex(0x2b2d30),
                    row_hover: hex(0x222427),
                    row_selected: hex(0x2c2f33),
                    text: hex(0xc9c7c1),
                    text_strong: hex(0xecebe6),
                    text_muted: hex(0x8a8882),
                    text_faint: hex(0x5f5e5a),
                    accent,
                    on_accent: hex(0x16130c),
                    error: hex(0xe07264),
                    warning: hex(0xd9a343),
                    ok: hex(0x86b46c),
                    scrim: Color32::from_black_alpha(120),
                };
                let code = CodeColors {
                    bg,
                    text: hex(0xd0cec8),
                    gutter_text: hex(0x55544f),
                    gutter_text_active: hex(0xa3a19a),
                    current_line: hex(0x212225),
                    selection: mix(bg, accent, 0.26),
                    selection_inactive: hex(0x2c2e32),
                    cursor: accent,
                    find_match: mix(bg, hex(0xd9a343), 0.30),
                    find_current: mix(bg, hex(0xd9a343), 0.62),
                    word_highlight: hex(0x2d3035),
                    lens_text: hex(0x76746e),
                    lens_hover: accent,
                    lens_sink: hex(0xd98478),
                    guide: hex(0x2c2d30),
                    hazard_bg: hex(0x5a2620),
                    hazard_text: hex(0xf2b8ae),
                    scrollbar: hex(0x3a3c40),
                };
                let syn = SyntaxColors {
                    keyword: hex(0xd68d5f),
                    type_: hex(0x82b5c9),
                    function: hex(0xdcc480),
                    string: hex(0x9ab57c),
                    escape: hex(0xc9aa6f),
                    number: hex(0xd3a08a),
                    comment: hex(0x6f716d),
                    operator: hex(0xaaa8a2),
                    punct: hex(0x8e8c86),
                    constant: hex(0xd19292),
                    attribute: hex(0x8fada3),
                    heading: hex(0xecebe6),
                    emphasis: hex(0xd9c9a3),
                    strong: hex(0xecebe6),
                    link: hex(0x82b5c9),
                    code_span: hex(0xbfae8c),
                    tag: hex(0xd68d5f),
                    key: hex(0x82b5c9),
                    marker: hex(0x8a8882),
                    inserted: hex(0x9ab57c),
                    deleted: hex(0xd98478),
                };
                Theme { mode, look, ui, code, syn }
            }
            Mode::Light => {
                let bg = hex(0xfcfbf9);
                let ui = UiColors {
                    panel_bg: hex(0xf2f0ec),
                    overlay_bg: hex(0xfcfbf9),
                    input_bg: hex(0xffffff),
                    border: hex(0xdcd9d2),
                    row_hover: hex(0xe9e6e0),
                    row_selected: hex(0xdedad2),
                    text: hex(0x3d3c38),
                    text_strong: hex(0x1b1a18),
                    text_muted: hex(0x77746d),
                    text_faint: hex(0xa5a29a),
                    accent,
                    on_accent: hex(0xffffff),
                    error: hex(0xb3392c),
                    warning: hex(0x9a6a08),
                    ok: hex(0x3f7a2a),
                    scrim: Color32::from_black_alpha(60),
                };
                let code = CodeColors {
                    bg,
                    text: hex(0x33322f),
                    gutter_text: hex(0xb4b1a9),
                    gutter_text_active: hex(0x66635d),
                    current_line: hex(0xf5f3ef),
                    selection: mix(bg, accent, 0.22),
                    selection_inactive: hex(0xe6e3dd),
                    cursor: accent,
                    find_match: mix(bg, hex(0xe0a850), 0.35),
                    find_current: mix(bg, hex(0xe0a850), 0.75),
                    word_highlight: hex(0xe9e6df),
                    lens_text: hex(0x9a978f),
                    lens_hover: accent,
                    lens_sink: hex(0xb3392c),
                    guide: hex(0xe6e3dc),
                    hazard_bg: hex(0xf6d3cd),
                    hazard_text: hex(0x8a2a20),
                    scrollbar: hex(0xcfccc4),
                };
                let syn = SyntaxColors {
                    keyword: hex(0xa8501f),
                    type_: hex(0x2a6c88),
                    function: hex(0x7a5a00),
                    string: hex(0x4c7729),
                    escape: hex(0x8a6a1c),
                    number: hex(0x9a553a),
                    comment: hex(0x928f87),
                    operator: hex(0x5f5d58),
                    punct: hex(0x77746d),
                    constant: hex(0xa0464a),
                    attribute: hex(0x4f7568),
                    heading: hex(0x1b1a18),
                    emphasis: hex(0x6a5320),
                    strong: hex(0x1b1a18),
                    link: hex(0x2a6c88),
                    code_span: hex(0x7a6336),
                    tag: hex(0xa8501f),
                    key: hex(0x2a6c88),
                    marker: hex(0x77746d),
                    inserted: hex(0x4c7729),
                    deleted: hex(0xb3392c),
                };
                Theme { mode, look, ui, code, syn }
            }
        }
    }

    pub fn font_ui(&self) -> FontId {
        FontId::new(self.look.ui_size, FontFamily::Proportional)
    }

    pub fn font_small(&self) -> FontId {
        FontId::new((self.look.ui_size - 1.5).max(9.0), FontFamily::Proportional)
    }

    pub fn font_code(&self) -> FontId {
        FontId::new(self.look.code_size, FontFamily::Monospace)
    }

    /// Monospace at UI size: paths, counts, shortcuts in lists.
    pub fn font_mono_ui(&self) -> FontId {
        FontId::new((self.look.ui_size - 1.0).max(9.0), FontFamily::Monospace)
    }

    /// Height of one list row (tree, results, palette), pt.
    pub fn row_h(&self) -> f32 {
        (self.look.ui_size * 1.75).round()
    }

    /// Height of one editor line, pt.
    pub fn code_row_h(&self) -> f32 {
        (self.look.code_size * self.look.line_height).round()
    }

    pub fn border(&self) -> Stroke {
        Stroke::new(1.0, self.ui.border)
    }

    /// Install into `ctx` (both egui theme slots). Cheap when nothing changed: call every frame.
    pub fn apply(&self, ctx: &egui::Context) {
        let id = egui::Id::unique("margin-theme");
        if ctx.data(|d| d.get_temp::<Theme>(id)) == Some(*self) {
            return;
        }
        let t = *self;
        ctx.all_styles_mut(move |s| t.style_into(s));
        ctx.set_theme(match self.mode {
            Mode::Dark => egui::Theme::Dark,
            Mode::Light => egui::Theme::Light,
        });
        ctx.data_mut(|d| d.insert_temp(id, *self));
    }

    /// Write this theme into an egui `Style`.
    pub fn style_into(&self, s: &mut egui::Style) {
        let ui = &self.ui;
        s.text_styles = [
            (TextStyle::Small, self.font_small()),
            (TextStyle::Body, self.font_ui()),
            (TextStyle::Button, self.font_ui()),
            (TextStyle::Heading, FontId::new(self.look.ui_size + 3.0, FontFamily::Proportional)),
            (TextStyle::Monospace, self.font_code()),
        ]
        .into();
        s.animation_time = 0.08;
        s.explanation_tooltips = false;
        s.url_in_tooltip = false;
        s.interaction.selectable_labels = false;
        s.interaction.tooltip_delay = 0.5;
        s.interaction.show_tooltips_only_when_still = true;

        let sp = &mut s.spacing;
        sp.item_spacing = egui::vec2(8.0, 4.0);
        sp.button_padding = egui::vec2(10.0, 4.0);
        sp.interact_size = egui::vec2(self.row_h(), self.row_h());
        sp.window_margin = egui::Margin::same(10);
        sp.menu_margin = egui::Margin::same(4);
        sp.indent = 14.0;
        sp.icon_width = 14.0;
        sp.icon_width_inner = 8.0;
        sp.icon_spacing = 6.0;
        sp.combo_height = 280.0;
        sp.scroll = egui::style::ScrollStyle::thin();

        let v = &mut s.visuals;
        v.dark_mode = self.mode == Mode::Dark;
        v.override_text_color = None;
        v.panel_fill = ui.panel_bg;
        v.window_fill = ui.overlay_bg;
        v.window_stroke = Stroke::new(1.0, ui.border);
        v.window_corner_radius = CornerRadius::same(6);
        v.menu_corner_radius = CornerRadius::same(5);
        let shadow = if self.mode == Mode::Dark { 130 } else { 45 };
        v.window_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(shadow) };
        v.popup_shadow = Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(shadow * 3 / 4) };
        v.window_highlight_topmost = false;
        v.faint_bg_color = ui.row_hover;
        v.extreme_bg_color = ui.input_bg;
        v.text_edit_bg_color = Some(ui.input_bg);
        v.code_bg_color = ui.row_hover;
        v.warn_fg_color = ui.warning;
        v.error_fg_color = ui.error;
        v.hyperlink_color = self.syn.link;
        v.selection.bg_fill = self.code.selection;
        v.selection.stroke = Stroke::new(1.0, ui.accent);
        v.text_cursor.stroke = Stroke::new(1.5, ui.accent);
        v.striped = false;
        v.button_frame = true;
        v.collapsing_header_frame = false;
        v.indent_has_left_vline = false;
        v.image_loading_spinners = false;

        let r = CornerRadius::same(4);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = ui.panel_bg;
        w.noninteractive.weak_bg_fill = ui.panel_bg;
        w.noninteractive.bg_stroke = Stroke::new(1.0, ui.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, ui.text);
        w.noninteractive.corner_radius = CornerRadius::ZERO;
        w.noninteractive.expansion = 0.0;
        for (wv, bg, stroke, fg) in [
            (&mut w.inactive, ui.row_hover, Stroke::new(1.0, ui.border), ui.text),
            (&mut w.hovered, ui.row_selected, Stroke::new(1.0, ui.border), ui.text_strong),
            (&mut w.active, ui.row_selected, Stroke::new(1.0, ui.accent), ui.text_strong),
            (&mut w.open, ui.row_selected, Stroke::new(1.0, ui.border), ui.text_strong),
        ] {
            wv.bg_fill = bg;
            wv.weak_bg_fill = bg;
            wv.bg_stroke = stroke;
            wv.fg_stroke = Stroke::new(1.0, fg);
            wv.corner_radius = r;
            wv.expansion = 0.0;
        }
    }
}

/// WCAG relative luminance contrast ratio of two opaque colours (1 – 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    fn lum(c: Color32) -> f32 {
        let f = |v: u8| {
            let x = f32::from(v) / 255.0;
            if x <= 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_readable_in_both_modes_and_every_accent() {
        for mode in [Mode::Dark, Mode::Light] {
            for accent in Accent::ALL {
                let t = Theme::new(mode, Look { accent, ..Look::default() });
                let who = format!("{mode:?}/{}", accent.name());
                for (name, fg, bg, min) in [
                    ("ui text", t.ui.text, t.ui.panel_bg, 7.0),
                    ("ui muted", t.ui.text_muted, t.ui.panel_bg, 4.0),
                    ("code text", t.code.text, t.code.bg, 7.0),
                    ("code on selection", t.code.text, t.code.selection, 4.5),
                    ("comment", t.syn.comment, t.code.bg, 3.0),
                    ("keyword", t.syn.keyword, t.code.bg, 4.5),
                    ("string", t.syn.string, t.code.bg, 4.5),
                    ("type", t.syn.type_, t.code.bg, 4.5),
                    ("function", t.syn.function, t.code.bg, 4.5),
                    ("number", t.syn.number, t.code.bg, 4.5),
                    ("constant", t.syn.constant, t.code.bg, 4.5),
                    ("accent", t.ui.accent, t.ui.panel_bg, 4.0),
                    ("on accent", t.ui.on_accent, t.ui.accent, 4.5),
                    ("lens", t.code.lens_text, t.code.bg, 2.8),
                    ("hazard", t.code.hazard_text, t.code.hazard_bg, 4.5),
                ] {
                    let c = contrast(fg, bg);
                    assert!(c >= min, "{who}: {name} contrast {c:.2} < {min}");
                }
            }
        }
    }

    #[test]
    fn no_purple() {
        // Hue between blue and magenta with real saturation = purple. None of the accents may be.
        for mode in [Mode::Dark, Mode::Light] {
            for a in Accent::ALL {
                let c = a.color(mode);
                let (r, g, b) = (i32::from(c.r()), i32::from(c.g()), i32::from(c.b()));
                assert!(!(b > g + 25 && r > g + 25), "{} is purple", a.name());
            }
        }
    }

    #[test]
    fn look_is_clamped() {
        let l = Look { ui_size: 99.0, code_size: 1.0, line_height: 9.0, accent: Accent::Teal }.clamped();
        assert_eq!((l.ui_size, l.code_size, l.line_height), (20.0, 8.0, 2.0));
        assert_eq!(Accent::from_name("teal"), Some(Accent::Teal));
        assert_eq!(Accent::from_name("purple"), None);
    }
}
