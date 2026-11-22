//! Settings model and `settings.conf`.
//!
//! Format (also `state.conf`): `key = value` per line, `#` comments, blank lines ignored.
//! Unknown keys ignored, bad values → default, numbers clamped, file capped at `MAX_CONF` bytes.
//!
//! | Key | Values | Default |
//! |---|---|---|
//! | `theme` | `dark` `light` | dark |
//! | `accent` | `amber` `teal` `blue` `green` `rose` | amber |
//! | `ui_size` / `code_size` | pt (10–20 / 8–32) | 13 / 13 |
//! | `line_height` | 1.2–2.0 | 1.5 |
//! | `zoom` | 0.5–3.0 | 1.0 |
//! | `tab_width` | 1–16 | 4 |
//! | `soft_tabs`, `line_numbers`, `highlight_line`, `show_whitespace`, `code_hints`, `restore_tabs` | bool | on, on, on, off, on, on |
//! | `wrap` | `auto` `on` `off` | auto |
//! | `autosave` | bool (on focus loss) | off |
//! | `respect_gitignore`, `index_hidden` | bool | on, on |
//! | `max_file_kb` | 16–65536 | 2048 |

use margin_editor::Wrap;
use margin_proto::IndexOpts;
use margin_theme::{Accent, Look, Mode};

/// Largest config file read.
pub const MAX_CONF: usize = 256 << 10;
pub const ZOOM: std::ops::RangeInclusive<f32> = 0.5..=3.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub mode: Mode,
    pub look: Look,
    pub zoom: f32,
    pub tab_width: u32,
    pub soft_tabs: bool,
    pub wrap: Wrap,
    pub line_numbers: bool,
    pub highlight_line: bool,
    pub show_whitespace: bool,
    pub code_hints: bool,
    pub autosave: bool,
    pub restore_tabs: bool,
    pub respect_gitignore: bool,
    pub index_hidden: bool,
    pub max_file_kb: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Dark,
            look: Look::default(),
            zoom: 1.0,
            tab_width: 4,
            soft_tabs: true,
            wrap: Wrap::Auto,
            line_numbers: true,
            highlight_line: true,
            show_whitespace: false,
            code_hints: true,
            autosave: false,
            restore_tabs: true,
            respect_gitignore: true,
            index_hidden: true,
            max_file_kb: 2048,
        }
    }
}

/// `key = value` pairs in file order. Never panics; caps size and line count.
pub fn parse_conf(text: &str) -> Vec<(String, String)> {
    let text = &text[..crate::sanitize::floor_char(text, MAX_CONF)];
    text.lines()
        .take(10_000)
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_owned(), v.trim().to_owned()))
        })
        .collect()
}

fn bool_of(v: &str) -> Option<bool> {
    match v {
        "true" | "on" | "yes" | "1" => Some(true),
        "false" | "off" | "no" | "0" => Some(false),
        _ => None,
    }
}

fn num(v: &str, r: std::ops::RangeInclusive<f32>) -> Option<f32> {
    let x: f32 = v.parse().ok()?;
    x.is_finite().then(|| x.clamp(*r.start(), *r.end()))
}

pub fn wrap_name(w: Wrap) -> &'static str {
    match w {
        Wrap::Auto => "auto",
        Wrap::On => "on",
        Wrap::Off => "off",
    }
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for (k, v) in parse_conf(text) {
            let v = v.as_str();
            match k.as_str() {
                "theme" => {
                    s.mode = match v {
                        "light" => Mode::Light,
                        _ => Mode::Dark,
                    }
                }
                "accent" => s.look.accent = Accent::from_name(v).unwrap_or_default(),
                "ui_size" => s.look.ui_size = num(v, Look::UI_SIZE).unwrap_or(Look::default().ui_size),
                "code_size" => s.look.code_size = num(v, Look::CODE_SIZE).unwrap_or(Look::default().code_size),
                "line_height" => s.look.line_height = num(v, Look::LINE_HEIGHT).unwrap_or(Look::default().line_height),
                "zoom" => s.zoom = num(v, ZOOM).unwrap_or(1.0),
                "tab_width" => s.tab_width = num(v, 1.0..=16.0).map_or(4, |x| x.round() as u32),
                "wrap" => {
                    s.wrap = match v {
                        "on" => Wrap::On,
                        "off" => Wrap::Off,
                        _ => Wrap::Auto,
                    }
                }
                "max_file_kb" => s.max_file_kb = num(v, 16.0..=65536.0).map_or(2048, |x| x.round() as u32),
                key => {
                    let d = Settings::default();
                    let b = bool_of(v);
                    match key {
                        "soft_tabs" => s.soft_tabs = b.unwrap_or(d.soft_tabs),
                        "line_numbers" => s.line_numbers = b.unwrap_or(d.line_numbers),
                        "highlight_line" => s.highlight_line = b.unwrap_or(d.highlight_line),
                        "show_whitespace" => s.show_whitespace = b.unwrap_or(d.show_whitespace),
                        "code_hints" => s.code_hints = b.unwrap_or(d.code_hints),
                        "autosave" => s.autosave = b.unwrap_or(d.autosave),
                        "restore_tabs" => s.restore_tabs = b.unwrap_or(d.restore_tabs),
                        "respect_gitignore" => s.respect_gitignore = b.unwrap_or(d.respect_gitignore),
                        "index_hidden" => s.index_hidden = b.unwrap_or(d.index_hidden),
                        _ => {}
                    }
                }
            }
        }
        s.look = s.look.clamped();
        s
    }

    pub fn write(&self) -> String {
        let mode = if self.mode == Mode::Light { "light" } else { "dark" };
        let l = &self.look;
        format!(
            "# Margin settings. `key = value`; unknown keys are ignored.\n\
             theme = {mode}\naccent = {}\nui_size = {}\ncode_size = {}\nline_height = {}\nzoom = {}\n\
             tab_width = {}\nsoft_tabs = {}\nwrap = {}\nline_numbers = {}\nhighlight_line = {}\n\
             show_whitespace = {}\ncode_hints = {}\nautosave = {}\nrestore_tabs = {}\n\
             respect_gitignore = {}\nindex_hidden = {}\nmax_file_kb = {}\n",
            l.accent.name(),
            l.ui_size,
            l.code_size,
            l.line_height,
            self.zoom,
            self.tab_width,
            self.soft_tabs,
            wrap_name(self.wrap),
            self.line_numbers,
            self.highlight_line,
            self.show_whitespace,
            self.code_hints,
            self.autosave,
            self.restore_tabs,
            self.respect_gitignore,
            self.index_hidden,
            self.max_file_kb,
        )
    }

    pub fn index_opts(&self) -> IndexOpts {
        IndexOpts { respect_gitignore: self.respect_gitignore, index_hidden: self.index_hidden, max_file_bytes: u64::from(self.max_file_kb) << 10 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut s = Settings::default();
        assert_eq!(Settings::parse(&s.write()), s);
        s.mode = Mode::Light;
        s.look = Look { ui_size: 16.0, code_size: 15.5, line_height: 1.7, accent: Accent::Teal };
        s.zoom = 1.25;
        s.tab_width = 2;
        s.wrap = Wrap::Off;
        s.code_hints = false;
        s.autosave = true;
        s.respect_gitignore = false;
        s.max_file_kb = 512;
        assert_eq!(Settings::parse(&s.write()), s);
        assert_eq!(s.index_opts().max_file_bytes, 512 << 10);
    }

    #[test]
    fn hostile_files() {
        let d = Settings::default();
        assert_eq!(Settings::parse(""), d);
        assert_eq!(Settings::parse("\0\0\u{202e}=\n==\n=x\nui_size\n"), d);
        let s = Settings::parse("ui_size = 999\ncode_size = -4\nline_height = NaN\nzoom = inf\ntab_width = 1e9\naccent = purple\ntheme = light\nwrap = maybe\nautosave = yes\nnope = 1\n# ui_size = 11\n");
        assert_eq!(s.look.ui_size, 20.0);
        assert_eq!(s.look.code_size, 8.0);
        assert_eq!(s.look.line_height, d.look.line_height);
        assert_eq!(s.zoom, 1.0);
        assert_eq!(s.tab_width, 16);
        assert_eq!(s.look.accent, Accent::Amber);
        assert_eq!(s.mode, Mode::Light);
        assert_eq!(s.wrap, Wrap::Auto);
        assert!(s.autosave);
        // Huge file: capped, no panic, multi-byte cut safe.
        let big = format!("ui_size = 11\n{}", "é".repeat(MAX_CONF));
        assert_eq!(Settings::parse(&big).look.ui_size, 11.0);
        let lines = "x = y\n".repeat(50_000);
        assert_eq!(parse_conf(&lines).len(), 10_000);
        assert_eq!(parse_conf("a = b = c"), vec![("a".to_owned(), "b = c".to_owned())]);
    }
}
