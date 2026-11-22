//! Command registry + keymap. One table feeds the palette, shortcut handling and the settings list.
//!
//! Chords: `CMD` = Cmd on macOS, Ctrl elsewhere; `CTRL` = the literal Control key (macOS only; elsewhere
//! it is the same key as `CMD`). Matching is exact on shift/alt. Shortcuts are consumed before the editor runs.

use egui::{Key, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cmd {
    Palette,
    GoToFile,
    OpenFolder,
    CloseFolder,
    NewFile,
    NewFolder,
    Save,
    SaveAll,
    CloseTab,
    NextTab,
    PrevTab,
    Find,
    Replace,
    FindNext,
    FindPrev,
    SearchFolder,
    GoToLine,
    GoToSymbol,
    GoToDef,
    FindCallers,
    Back,
    Forward,
    ToggleSidebar,
    ShowFiles,
    ToggleWrap,
    ToggleHints,
    ToggleWhitespace,
    ToggleLineNumbers,
    ToggleTheme,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Settings,
    RebuildIndex,
    RevertFile,
    CopyPath,
    CopyRelPath,
    Quit,
}

pub const CMD: u8 = 1;
pub const CTRL: u8 = 2;
pub const SHIFT: u8 = 4;
pub const ALT: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: Key,
    pub mods: u8,
}

const fn c(mods: u8, key: Key) -> Chord {
    Chord { key, mods }
}

pub struct Def {
    pub cmd: Cmd,
    pub title: &'static str,
    pub mac: &'static [Chord],
    pub other: &'static [Chord],
}

macro_rules! def {
    ($cmd:ident, $title:expr, [$($m:expr),*], [$($o:expr),*]) => {
        Def { cmd: Cmd::$cmd, title: $title, mac: &[$($m),*], other: &[$($o),*] }
    };
    ($cmd:ident, $title:expr, [$($m:expr),*]) => {
        Def { cmd: Cmd::$cmd, title: $title, mac: &[$($m),*], other: &[$($m),*] }
    };
}

use Key as K;

/// Every command, in palette order for an empty query (before recently used ones move up).
pub const ALL: &[Def] = &[
    def!(Palette, "Command Palette", [c(CMD, K::P), c(CTRL, K::P), c(CMD | SHIFT, K::P)], [c(CMD, K::P), c(CMD | SHIFT, K::P)]),
    def!(GoToFile, "Go to File…", [c(CMD, K::T)]),
    def!(OpenFolder, "Open Folder…", [c(CMD, K::O)]),
    def!(CloseFolder, "Close Folder", []),
    def!(NewFile, "New File…", [c(CMD, K::N)]),
    def!(NewFolder, "New Folder…", []),
    def!(Save, "Save", [c(CMD, K::S)]),
    def!(SaveAll, "Save All", [c(CMD | ALT, K::S)]),
    def!(CloseTab, "Close Tab", [c(CMD, K::W)]),
    def!(NextTab, "Next Tab", [c(CTRL, K::Tab)]),
    def!(PrevTab, "Previous Tab", [c(CTRL | SHIFT, K::Tab)]),
    def!(Find, "Find in File", [c(CMD, K::F)]),
    def!(Replace, "Replace in File", [c(CMD | ALT, K::F)]),
    def!(FindNext, "Find Next", [c(CMD, K::G), c(0, K::F3)], [c(0, K::F3)]),
    def!(FindPrev, "Find Previous", [c(CMD | SHIFT, K::G), c(SHIFT, K::F3)], [c(SHIFT, K::F3)]),
    def!(SearchFolder, "Search in Folder", [c(CMD | SHIFT, K::F)]),
    def!(GoToLine, "Go to Line…", [c(CTRL, K::G)]),
    def!(GoToSymbol, "Go to Symbol in File…", [c(CMD | SHIFT, K::O)]),
    def!(GoToDef, "Go to Definition", [c(0, K::F12)]),
    def!(FindCallers, "Find Callers", [c(SHIFT, K::F12)]),
    def!(Back, "Back", [c(CTRL, K::Minus)], [c(ALT, K::ArrowLeft)]),
    def!(Forward, "Forward", [c(CTRL | SHIFT, K::Minus)], [c(ALT, K::ArrowRight)]),
    def!(ToggleSidebar, "Toggle Sidebar", [c(CMD, K::B)]),
    def!(ShowFiles, "Show Files", [c(CMD | SHIFT, K::E)]),
    def!(ToggleWrap, "Toggle Word Wrap", [c(ALT, K::Z)]),
    def!(ToggleHints, "Toggle Code Hints", []),
    def!(ToggleWhitespace, "Toggle Whitespace", []),
    def!(ToggleLineNumbers, "Toggle Line Numbers", []),
    def!(ToggleTheme, "Toggle Dark / Light Theme", []),
    def!(ZoomIn, "Zoom In", [c(CMD, K::Equals), c(CMD | SHIFT, K::Equals), c(CMD, K::Plus)]),
    def!(ZoomOut, "Zoom Out", [c(CMD, K::Minus)]),
    def!(ZoomReset, "Reset Zoom", [c(CMD, K::Num0)]),
    def!(Settings, "Settings", [c(CMD, K::Comma)]),
    def!(RebuildIndex, "Rebuild Index", []),
    def!(RevertFile, "Revert File", []),
    def!(CopyPath, "Copy Path", []),
    def!(CopyRelPath, "Copy Relative Path", []),
    def!(Quit, "Quit", [c(CMD, K::Q)]),
];

pub fn def(cmd: Cmd) -> &'static Def {
    ALL.iter().find(|d| d.cmd == cmd).unwrap_or(&ALL[0])
}

pub fn chords(d: &Def, mac: bool) -> &'static [Chord] {
    if mac {
        d.mac
    } else {
        d.other
    }
}

/// Does `m` (from an egui key event) press exactly `ch`'s modifiers?
pub fn matches(ch: Chord, key: Key, m: Modifiers, mac: bool) -> bool {
    if ch.key != key || m.shift != (ch.mods & SHIFT != 0) || m.alt != (ch.mods & ALT != 0) {
        return false;
    }
    let want_cmd = ch.mods & CMD != 0;
    let want_ctrl = ch.mods & CTRL != 0;
    if mac {
        // `command` is Cmd on macOS; `ctrl` the Control key.
        let cmd = m.command || m.mac_cmd;
        cmd == want_cmd && m.ctrl == want_ctrl
    } else {
        (m.command || m.ctrl) == (want_cmd || want_ctrl)
    }
}

/// Shortcut label: `⌘⇧P` on macOS, `Ctrl+Shift+P` elsewhere.
pub fn label(ch: Chord, mac: bool) -> String {
    let key = match ch.key {
        K::Equals | K::Plus => "=".to_owned(),
        K::Minus => "-".to_owned(),
        K::Comma => ",".to_owned(),
        K::Num0 => "0".to_owned(),
        K::ArrowLeft => if mac { "←" } else { "Left" }.to_owned(),
        K::ArrowRight => if mac { "→" } else { "Right" }.to_owned(),
        K::Tab => "Tab".to_owned(),
        k => k.name().to_owned(),
    };
    let m = ch.mods;
    if mac {
        let mut s = String::new();
        for (bit, g) in [(CTRL, "⌃"), (ALT, "⌥"), (SHIFT, "⇧"), (CMD, "⌘")] {
            if m & bit != 0 {
                s += g;
            }
        }
        if ch.key == K::Tab && !s.is_empty() {
            s += " ";
        }
        s + &key
    } else {
        let mut parts: Vec<&str> = Vec::new();
        if m & (CMD | CTRL) != 0 {
            parts.push("Ctrl");
        }
        if m & ALT != 0 {
            parts.push("Alt");
        }
        if m & SHIFT != 0 {
            parts.push("Shift");
        }
        parts.push(&key);
        parts.join("+")
    }
}

/// First shortcut of `cmd`, formatted; empty when none.
pub fn shortcut(cmd: Cmd, mac: bool) -> String {
    chords(def(cmd), mac).first().map(|c| label(*c, mac)).unwrap_or_default()
}

/// Take shortcut key presses out of this frame's input, in order. `only`: when set, only these
/// commands are taken (overlays open); others stay in the input.
pub fn take(ctx: &egui::Context, mac: bool, only: Option<&[Cmd]>) -> Vec<Cmd> {
    let mut out = Vec::new();
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            let egui::Event::Key { key, pressed, modifiers, .. } = e else { return true };
            let hit = ALL.iter().find(|d| only.is_none_or(|o| o.contains(&d.cmd)) && chords(d, mac).iter().any(|c| matches(*c, *key, *modifiers, mac)));
            match hit {
                Some(d) => {
                    if *pressed {
                        out.push(d.cmd);
                    }
                    false
                }
                None => true,
            }
        });
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn titles_and_unique_shortcuts() {
        let mut seen_cmd = HashSet::new();
        for mac in [true, false] {
            let mut seen = HashSet::new();
            for d in ALL {
                assert!(!d.title.is_empty());
                seen_cmd.insert(d.cmd);
                for ch in chords(d, mac) {
                    // Off macOS CTRL and CMD are one key.
                    let norm = if mac { *ch } else { Chord { key: ch.key, mods: if ch.mods & (CMD | CTRL) != 0 { (ch.mods & !(CTRL)) | CMD } else { ch.mods } } };
                    assert!(seen.insert(norm), "duplicate shortcut {} (mac={mac}) on {}", label(*ch, mac), d.title);
                }
            }
        }
        assert_eq!(seen_cmd.len(), ALL.len(), "a command is listed twice");
    }

    #[test]
    fn matching_is_exact() {
        let cmd_p = Modifiers::COMMAND;
        let mac_cmd = Modifiers { mac_cmd: true, command: true, ..Default::default() };
        assert!(matches(c(CMD, K::P), K::P, cmd_p, true));
        assert!(matches(c(CMD, K::P), K::P, mac_cmd, true));
        assert!(!matches(c(CMD, K::P), K::P, Modifiers::CTRL, true));
        assert!(matches(c(CTRL, K::P), K::P, Modifiers::CTRL, true));
        assert!(!matches(c(CMD, K::P), K::P, Modifiers::COMMAND | Modifiers::SHIFT, true));
        // Linux: ctrl+P sets both ctrl and command.
        let lin = Modifiers { ctrl: true, command: true, ..Default::default() };
        assert!(matches(c(CMD, K::P), K::P, lin, false));
        assert!(matches(c(CTRL, K::Tab), K::Tab, lin, false));
        assert!(!matches(c(0, K::F12), K::F12, Modifiers::SHIFT, false));
    }

    #[test]
    fn labels() {
        assert_eq!(shortcut(Cmd::Palette, true), "⌘P");
        assert_eq!(shortcut(Cmd::Palette, false), "Ctrl+P");
        assert_eq!(shortcut(Cmd::SaveAll, true), "⌥⌘S");
        assert_eq!(shortcut(Cmd::SaveAll, false), "Ctrl+Alt+S");
        assert_eq!(shortcut(Cmd::Back, false), "Alt+Left");
        assert_eq!(shortcut(Cmd::CloseFolder, true), "");
    }
}
