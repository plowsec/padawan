//! Command registry + keymap + menu layout. One table feeds the palette, shortcut handling, the menu bar
//! and the settings list.
//!
//! Chords: `CMD` = Cmd on macOS, Ctrl elsewhere; `CTRL` = the literal Control key (macOS only; elsewhere
//! it is the same key as `CMD`). Matching is exact on shift/alt. Shortcuts are consumed before the editor runs,
//! except `pass` ones (undo, clipboard…): those stay for the focused widget and are only labels.

use egui::{Key, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cmd {
    Palette,
    GoToFile,
    OpenFolder,
    CloseFolder,
    /// Untitled buffer.
    NewFile,
    /// Inline name field in the tree.
    NewFileInFolder,
    NewFolder,
    NewWindow,
    Save,
    SaveAs,
    SaveAll,
    CloseTab,
    NextTab,
    PrevTab,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    ToggleComment,
    Indent,
    Outdent,
    DuplicateLine,
    DeleteLine,
    MoveLineUp,
    MoveLineDown,
    Find,
    Replace,
    FindNext,
    FindPrev,
    SearchFolder,
    ReplaceFolder,
    GoToLine,
    GoToSymbol,
    GoToDef,
    FindCallers,
    Back,
    Forward,
    ToggleSidebar,
    ShowFiles,
    ToggleMenuBar,
    MdPreview,
    ToggleWrap,
    ToggleHints,
    ToggleWhitespace,
    ToggleLineNumbers,
    ToggleTheme,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Settings,
    Shortcuts,
    About,
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
    /// Label only: the key stays in the input for the focused widget (editor, text field).
    pub pass: bool,
}

macro_rules! def {
    ($cmd:ident, $title:expr, [$($m:expr),*], [$($o:expr),*]) => {
        Def { cmd: Cmd::$cmd, title: $title, mac: &[$($m),*], other: &[$($o),*], pass: false }
    };
    ($cmd:ident, $title:expr, [$($m:expr),*]) => {
        Def { cmd: Cmd::$cmd, title: $title, mac: &[$($m),*], other: &[$($m),*], pass: false }
    };
}

macro_rules! pass {
    ($cmd:ident, $title:expr, [$($m:expr),*], [$($o:expr),*]) => {
        Def { cmd: Cmd::$cmd, title: $title, mac: &[$($m),*], other: &[$($o),*], pass: true }
    };
    ($cmd:ident, $title:expr, [$($m:expr),*]) => {
        Def { cmd: Cmd::$cmd, title: $title, mac: &[$($m),*], other: &[$($m),*], pass: true }
    };
}

use Key as K;

/// Every command, in palette order for an empty query (before recently used ones move up).
pub const ALL: &[Def] = &[
    def!(Palette, "Command Palette", [c(CMD, K::P), c(CTRL, K::P), c(CMD | SHIFT, K::P)], [c(CMD, K::P), c(CMD | SHIFT, K::P)]),
    def!(GoToFile, "Go to File…", [c(CMD, K::T)]),
    def!(OpenFolder, "Open Folder…", [c(CMD, K::O)]),
    def!(CloseFolder, "Close Folder", []),
    def!(NewFile, "New File", [c(CMD, K::N)]),
    def!(NewFileInFolder, "New File in Folder…", []),
    def!(NewFolder, "New Folder…", []),
    def!(NewWindow, "New Window", [c(CMD | SHIFT, K::N)]),
    def!(Save, "Save", [c(CMD, K::S)]),
    def!(SaveAs, "Save As…", [c(CMD | SHIFT, K::S)]),
    def!(SaveAll, "Save All", [c(CMD | ALT, K::S)]),
    def!(CloseTab, "Close Tab", [c(CMD, K::W)]),
    def!(NextTab, "Next Tab", [c(CTRL, K::Tab)]),
    def!(PrevTab, "Previous Tab", [c(CTRL | SHIFT, K::Tab)]),
    pass!(Undo, "Undo", [c(CMD, K::Z)]),
    pass!(Redo, "Redo", [c(CMD | SHIFT, K::Z)], [c(CMD, K::Y), c(CMD | SHIFT, K::Z)]),
    pass!(Cut, "Cut", [c(CMD, K::X)]),
    pass!(Copy, "Copy", [c(CMD, K::C)]),
    pass!(Paste, "Paste", [c(CMD, K::V)]),
    pass!(SelectAll, "Select All", [c(CMD, K::A)]),
    pass!(ToggleComment, "Toggle Comment", [c(CMD, K::Slash)]),
    def!(Indent, "Indent", [c(CMD, K::CloseBracket)]),
    def!(Outdent, "Outdent", [c(CMD, K::OpenBracket)]),
    def!(DuplicateLine, "Duplicate Line", [c(SHIFT | ALT, K::ArrowDown)]),
    def!(DeleteLine, "Delete Line", [c(CMD | SHIFT, K::K)]),
    def!(MoveLineUp, "Move Line Up", [c(ALT, K::ArrowUp)]),
    def!(MoveLineDown, "Move Line Down", [c(ALT, K::ArrowDown)]),
    def!(Find, "Find in File", [c(CMD, K::F)]),
    def!(Replace, "Replace in File", [c(CMD | ALT, K::F)]),
    def!(FindNext, "Find Next", [c(CMD, K::G), c(0, K::F3)], [c(0, K::F3)]),
    def!(FindPrev, "Find Previous", [c(CMD | SHIFT, K::G), c(SHIFT, K::F3)], [c(SHIFT, K::F3)]),
    def!(SearchFolder, "Find in Folder", [c(CMD | SHIFT, K::F)]),
    def!(ReplaceFolder, "Replace in Folder", [c(CMD | SHIFT, K::H)]),
    def!(GoToLine, "Go to Line…", [c(CTRL, K::G)]),
    def!(GoToSymbol, "Go to Symbol in File…", [c(CMD | SHIFT, K::O)]),
    def!(GoToDef, "Go to Definition", [c(0, K::F12)]),
    def!(FindCallers, "Find Callers", [c(SHIFT, K::F12)]),
    def!(Back, "Back", [c(CTRL, K::Minus)], [c(ALT, K::ArrowLeft)]),
    def!(Forward, "Forward", [c(CTRL | SHIFT, K::Minus)], [c(ALT, K::ArrowRight)]),
    def!(ToggleSidebar, "Toggle Sidebar", [c(CMD, K::B)]),
    def!(ShowFiles, "Find File by Name", [c(CMD | SHIFT, K::E)]),
    def!(ToggleMenuBar, "Toggle Menu Bar", []),
    def!(MdPreview, "Toggle Markdown Preview", [c(CMD | SHIFT, K::V)]),
    def!(ToggleWrap, "Toggle Word Wrap", [c(ALT, K::Z)]),
    def!(ToggleHints, "Toggle Code Hints", []),
    def!(ToggleWhitespace, "Toggle Whitespace", []),
    def!(ToggleLineNumbers, "Toggle Line Numbers", []),
    def!(ToggleTheme, "Toggle Dark / Light Theme", []),
    def!(ZoomIn, "Zoom In", [c(CMD, K::Equals), c(CMD | SHIFT, K::Equals), c(CMD, K::Plus)]),
    def!(ZoomOut, "Zoom Out", [c(CMD, K::Minus)]),
    def!(ZoomReset, "Reset Zoom", [c(CMD, K::Num0)]),
    def!(Settings, "Settings", [c(CMD, K::Comma)]),
    def!(Shortcuts, "Keyboard Shortcuts", []),
    def!(About, "About Margin", []),
    def!(RebuildIndex, "Rebuild Index", []),
    def!(RevertFile, "Revert File", []),
    def!(CopyPath, "Copy Path", []),
    def!(CopyRelPath, "Copy Relative Path", []),
    def!(Quit, "Quit", [c(CMD, K::Q)]),
];

/// A menu bar entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Cmd(Cmd),
    Sep,
    /// `Open Recent ▸`: recent folders.
    Recent,
}

use MenuItem::{Cmd as M, Sep};

/// The menu bar: `(title, items)`. Titles and shortcuts come from `ALL`.
pub const MENUS: &[(&str, &[MenuItem])] = &[
    (
        "File",
        &[
            M(Cmd::NewFile),
            M(Cmd::NewWindow),
            Sep,
            M(Cmd::OpenFolder),
            MenuItem::Recent,
            Sep,
            M(Cmd::Save),
            M(Cmd::SaveAs),
            M(Cmd::SaveAll),
            M(Cmd::RevertFile),
            Sep,
            M(Cmd::CloseTab),
            M(Cmd::CloseFolder),
            Sep,
            M(Cmd::Quit),
        ],
    ),
    (
        "Edit",
        &[
            M(Cmd::Undo),
            M(Cmd::Redo),
            Sep,
            M(Cmd::Cut),
            M(Cmd::Copy),
            M(Cmd::Paste),
            M(Cmd::SelectAll),
            Sep,
            M(Cmd::Find),
            M(Cmd::Replace),
            M(Cmd::SearchFolder),
            M(Cmd::ReplaceFolder),
            Sep,
            M(Cmd::ToggleComment),
            M(Cmd::Indent),
            M(Cmd::Outdent),
            M(Cmd::DuplicateLine),
            M(Cmd::DeleteLine),
            M(Cmd::MoveLineUp),
            M(Cmd::MoveLineDown),
        ],
    ),
    (
        "View",
        &[
            M(Cmd::Palette),
            Sep,
            M(Cmd::ToggleSidebar),
            M(Cmd::ToggleMenuBar),
            M(Cmd::MdPreview),
            Sep,
            M(Cmd::ToggleWrap),
            M(Cmd::ToggleWhitespace),
            M(Cmd::ToggleLineNumbers),
            M(Cmd::ToggleHints),
            Sep,
            M(Cmd::ZoomIn),
            M(Cmd::ZoomOut),
            M(Cmd::ZoomReset),
            Sep,
            M(Cmd::ToggleTheme),
            M(Cmd::Settings),
        ],
    ),
    (
        "Go",
        &[
            M(Cmd::GoToFile),
            M(Cmd::GoToSymbol),
            M(Cmd::GoToLine),
            Sep,
            M(Cmd::GoToDef),
            M(Cmd::FindCallers),
            Sep,
            M(Cmd::Back),
            M(Cmd::Forward),
            Sep,
            M(Cmd::NextTab),
            M(Cmd::PrevTab),
        ],
    ),
    ("Help", &[M(Cmd::Shortcuts), M(Cmd::About)]),
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
        K::Slash => "/".to_owned(),
        K::OpenBracket => "[".to_owned(),
        K::CloseBracket => "]".to_owned(),
        K::ArrowLeft => if mac { "←" } else { "Left" }.to_owned(),
        K::ArrowRight => if mac { "→" } else { "Right" }.to_owned(),
        K::ArrowUp => if mac { "↑" } else { "Up" }.to_owned(),
        K::ArrowDown => if mac { "↓" } else { "Down" }.to_owned(),
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
/// commands are taken (overlays open); others stay in the input. `pass` commands are never taken.
pub fn take(ctx: &egui::Context, mac: bool, only: Option<&[Cmd]>) -> Vec<Cmd> {
    let mut out = Vec::new();
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            let egui::Event::Key { key, pressed, modifiers, .. } = e else { return true };
            let hit = ALL.iter().find(|d| !d.pass && only.is_none_or(|o| o.contains(&d.cmd)) && chords(d, mac).iter().any(|c| matches(*c, *key, *modifiers, mac)));
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
    fn menus_use_known_commands_once() {
        let mut seen = HashSet::new();
        for (title, items) in MENUS {
            assert!(!title.is_empty() && !items.is_empty());
            assert_ne!(items.first(), Some(&MenuItem::Sep));
            assert_ne!(items.last(), Some(&MenuItem::Sep));
            for it in *items {
                if let MenuItem::Cmd(c) = it {
                    assert!(seen.insert(*c), "{c:?} in two menus");
                    assert_eq!(def(*c).cmd, *c, "{c:?} has no definition");
                }
            }
        }
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
        assert_eq!(shortcut(Cmd::ToggleComment, true), "⌘/");
        assert_eq!(shortcut(Cmd::MoveLineUp, false), "Alt+Up");
    }

    #[test]
    fn pass_keys_stay_in_the_input() {
        let ctx = egui::Context::default();
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::Key { key: K::C, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
        raw.events.push(egui::Event::Key { key: K::S, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
        let mut got = Vec::new();
        let mut left = 0;
        let mut out = ctx.run_ui(raw, |ui| {
            got = take(ui.ctx(), false, None);
            left = ui.input(|i| i.events.len());
        });
        out.textures_delta.clear();
        assert_eq!(got, vec![Cmd::Save]);
        assert_eq!(left, 1, "cmd+C is left for the focused widget");
    }
}
