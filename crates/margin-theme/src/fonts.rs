//! Font stack: system UI / mono fonts when present, else egui's bundled ones; symbol fallbacks.
//!
//! | Role | macOS | Linux | Windows | Bundled |
//! |---|---|---|---|---|
//! | ui (Proportional) | SF (`SFNS.ttf`) | Inter, Noto Sans, DejaVu Sans | Segoe UI | Ubuntu-Light |
//! | code (Monospace) | SF Mono, Menlo | DejaVu Sans Mono, Noto Sans Mono | Cascadia Mono, Consolas | Hack |
//! | symbols (both) | Menlo, Apple Symbols | DejaVu Sans, Noto Symbols | Segoe UI Symbol | egui-icons |
//!
//! Only fixed system paths are read. Never a font from the open folder.
//! Files are read once per process, at start-up, before the first frame.

use std::sync::{Arc, OnceLock};

use egui::epaint::text::{FontData, FontDefinitions};
use egui::{Context, FontFamily};

/// What got loaded: names (file stems) per role, in fallback order.
#[derive(Clone, Debug, Default)]
pub struct FontReport {
    pub ui: Vec<String>,
    pub code: Vec<String>,
    /// Files found but unreadable, as `path: error`.
    pub errors: Vec<String>,
}

impl std::fmt::Display for FontReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ui: {} | code: {}", self.ui.join(" → "), self.code.join(" → "))
    }
}

/// Install the stack into `ctx`. Cheap after the first call (cached bytes).
pub fn install(ctx: &Context) -> FontReport {
    let (defs, report) = stack();
    ctx.set_fonts(defs.clone());
    report.clone()
}

/// Cached definitions + report.
pub fn stack() -> &'static (FontDefinitions, FontReport) {
    static STACK: OnceLock<(FontDefinitions, FontReport)> = OnceLock::new();
    STACK.get_or_init(build)
}

#[cfg(target_os = "macos")]
mod paths {
    pub const UI: &[&str] = &["/System/Library/Fonts/SFNS.ttf"];
    pub const CODE: &[&str] = &["/System/Library/Fonts/SFNSMono.ttf", "/System/Library/Fonts/Menlo.ttc"];
    pub const SYMBOLS: &[&str] = &["/System/Library/Fonts/Menlo.ttc", "/System/Library/Fonts/Apple Symbols.ttf"];
}

#[cfg(all(unix, not(target_os = "macos")))]
mod paths {
    pub const UI: &[&str] = &[
        "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
        "/usr/share/fonts/inter/Inter-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    ];
    pub const CODE: &[&str] = &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
    ];
    pub const SYMBOLS: &[&str] = &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
    ];
}

#[cfg(windows)]
mod paths {
    pub const UI: &[&str] = &["C:\\Windows\\Fonts\\segoeui.ttf"];
    pub const CODE: &[&str] = &["C:\\Windows\\Fonts\\CascadiaMono.ttf", "C:\\Windows\\Fonts\\consola.ttf"];
    pub const SYMBOLS: &[&str] = &["C:\\Windows\\Fonts\\seguisym.ttf"];
}

#[cfg(not(any(unix, windows)))]
mod paths {
    pub const UI: &[&str] = &[];
    pub const CODE: &[&str] = &[];
    pub const SYMBOLS: &[&str] = &[];
}

fn stem(path: &str) -> String {
    std::path::Path::new(path).file_stem().map_or_else(|| path.to_owned(), |s| s.to_string_lossy().into_owned())
}

/// Load one candidate into `defs` under its stem; returns the name.
fn load(defs: &mut FontDefinitions, path: &str, report: &mut FontReport) -> Option<String> {
    let name = stem(path);
    if defs.font_data.contains_key(&name) {
        return Some(name);
    }
    match std::fs::read(path) {
        Ok(bytes) => {
            defs.font_data.insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
            Some(name)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            report.errors.push(format!("{path}: {e}"));
            None
        }
    }
}

fn build() -> (FontDefinitions, FontReport) {
    let mut defs = FontDefinitions::default();
    let mut report = FontReport::default();
    let ui = paths::UI.iter().find_map(|p| load(&mut defs, p, &mut report));
    let code = paths::CODE.iter().find_map(|p| load(&mut defs, p, &mut report));
    let mut symbols: Vec<String> = Vec::new();
    for p in paths::SYMBOLS {
        if let Some(n) = load(&mut defs, p, &mut report) {
            if !symbols.contains(&n) && Some(&n) != code.as_ref() {
                symbols.push(n);
            }
        }
    }
    let bundled = |defs: &FontDefinitions, f: &FontFamily| defs.families.get(f).cloned().unwrap_or_default();
    let prop_tail = bundled(&defs, &FontFamily::Proportional);
    let mono_tail = bundled(&defs, &FontFamily::Monospace);

    let mut prop: Vec<String> = ui.into_iter().collect();
    prop.extend(symbols.iter().cloned());
    prop.extend(code.iter().cloned());
    let mut mono: Vec<String> = code.into_iter().collect();
    mono.extend(symbols.iter().cloned());
    for (chain, tail) in [(&mut prop, prop_tail), (&mut mono, mono_tail)] {
        for n in tail {
            if !chain.contains(&n) {
                chain.push(n);
            }
        }
    }
    report.ui = prop.clone();
    report.code = mono.clone();
    defs.families.insert(FontFamily::Proportional, prop);
    defs.families.insert(FontFamily::Monospace, mono);
    (defs, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_builds_and_has_both_families() {
        let (defs, report) = stack();
        assert!(!defs.families[&FontFamily::Proportional].is_empty());
        assert!(!defs.families[&FontFamily::Monospace].is_empty());
        if cfg!(target_os = "macos") {
            assert_eq!(report.ui.first().map(String::as_str), Some("SFNS"), "{report}");
        }
    }
}
