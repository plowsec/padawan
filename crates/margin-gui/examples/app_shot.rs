//! Render the key states of the app to PNG with the fake engine, and the app icon (`icon-<size>.png`).
//!
//! `cargo run -p margin-gui --example app_shot -- <abs out dir> [scale] [width height] [ui_size]`

#[path = "../tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};

use std::sync::Arc;

use common::{Script, H};
use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use margin_gui::hints::{SEG_CALLERS, SEG_SINKS};
use margin_proto::{CallPath, FlowArg, Hop, RelPath, Site};

const PARSER_C: &str = r#"/* Tiny HTTP header parser. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAX_HEADER 256

struct header {
    char name[MAX_HEADER];
    char value[MAX_HEADER];
};

static int read_line(FILE *in, char *buf, size_t len) {
    if (!fgets(buf, (int)len, in)) {
        return -1;
    }
    buf[strcspn(buf, "\r\n")] = 0;
    return (int)strlen(buf);
}

int parse_header(const char *line, struct header *out) {
    const char *colon = strchr(line, ':');
    if (!colon) {
        return -1;
    }
    size_t n = (size_t)(colon - line);
    memcpy(out->name, line, n);
    out->name[n] = 0;
    strcpy(out->value, colon + 2);
    return 0;
}

int run_hook(const struct header *h) {
    char cmd[512];
    sprintf(cmd, "notify %s", h->value);
    return system(cmd);
}

int main(int argc, char **argv) {
    char line[1024];
    struct header h;
    while (read_line(stdin, line, sizeof line) > 0) {
        if (parse_header(line, &h) == 0) {
            printf("%s = %s\n", h.name, h.value);
            run_hook(&h);
        }
    }
    return 0;
}
"#;

const NOTES_MD: &str = "# Notes\n\nHow the parser handles **untrusted** headers.\n\n## Checklist\n\n- [x] reject lines without a colon\n- [ ] bound `memcpy` by `MAX_HEADER`\n- [ ] drop `system()` in `run_hook`\n\n## Example\n\n```c\nstruct header h;\nparse_header(\"Host: a\", &h);\n```\n\n> `strcpy` copies without a bound. See [the parser](../src/parser.c).\n\n| Sink | Kind |\n|---|---|\n| memcpy | memory |\n| system | exec |\n";

fn site(path: &str, line: u32, text: &str, col: u32, len: u32, ctx: &str) -> Site {
    Site { path: RelPath::new(path).unwrap(), line, col, len, text: text.into(), text_col: col, context: ctx.into() }
}

fn arg(text: &str, from: &[&str]) -> FlowArg {
    FlowArg { text: text.into(), from: from.iter().map(|s| s.to_string()).collect() }
}

/// Paths from parse_header (as the engine would send them).
fn paths() -> Vec<CallPath> {
    let p = "src/parser.c";
    vec![
        CallPath {
            sink: "memcpy".into(),
            kind: "memory".into(),
            hops: vec![Hop { site: site(p, 38, "    memcpy(out->name, line, n);", 4, 6, "parse_header"), args: vec![arg("out->name", &["out"]), arg("line", &["line"]), arg("n", &["colon", "line"])] }],
            flows: true,
        },
        CallPath {
            sink: "strcpy".into(),
            kind: "memory".into(),
            hops: vec![Hop { site: site(p, 40, "    strcpy(out->value, colon + 2);", 4, 6, "parse_header"), args: vec![arg("out->value", &["out"]), arg("colon + 2", &["colon"])] }],
            flows: true,
        },
        CallPath {
            sink: "system".into(),
            kind: "exec".into(),
            hops: vec![
                Hop { site: site(p, 55, "            run_hook(&h);", 12, 8, "main"), args: vec![arg("&h", &["h"])] },
                Hop { site: site(p, 46, "    sprintf(cmd, \"notify %s\", h->value);", 4, 7, "run_hook"), args: vec![arg("cmd", &[]), arg("\"notify %s\"", &[]), arg("h->value", &["h"])] },
                Hop { site: site(p, 47, "    return system(cmd);", 11, 6, "run_hook"), args: vec![arg("cmd", &["cmd"])] },
            ],
            flows: false,
        },
    ]
}

fn right_click(a: &mut H, pos: Pos2) {
    a.shot.move_to([pos.x, pos.y]);
    a.frames(1);
    a.shot.event(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, modifiers: Modifiers::NONE });
    a.frames(1);
    a.shot.event(Event::PointerButton { pos, button: PointerButton::Secondary, pressed: false, modifiers: Modifiers::NONE });
    a.frames(2);
}

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("README.md", "# parser-c\n\nA tiny header parser.\n"),
        ("Makefile", "all:\n\tcc -o parser src/parser.c\n"),
        ("src/parser.c", PARSER_C),
        ("src/net/socket.c", "int open_socket(int port) {\n    return port;\n}\n"),
        ("src/net/socket.h", "int open_socket(int port);\n"),
        ("src/util.h", "#pragma once\n"),
        ("tests/test_parser.c", "int test_parse(void) {\n    struct header h;\n    return parse_header(\"a: b\", &h);\n}\n"),
        ("docs/notes.md", NOTES_MD),
        ("target/out.o", "\0\0\0"),
        ("build.log", "log\n"),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| common::shots_dir().to_string_lossy().into_owned()));
    if !out.is_absolute() {
        eprintln!("out dir must be absolute");
        std::process::exit(2);
    }
    let _ = std::fs::create_dir_all(&out);
    let scale: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let w: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1280.0);
    let h: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(800.0);
    let ui_size: Option<f32> = args.get(5).and_then(|s| s.parse().ok());
    let suffix = if args.len() > 2 { format!("-{w}x{h}@{scale}") } else { String::new() };
    let mut conf = String::new();
    if let Some(s) = ui_size {
        conf = format!("ui_size = {s}\n");
    }
    for mode in ["dark", "light"] {
        let settings = format!("{conf}theme = {mode}\n");
        shots(&out, mode, &suffix, scale, [w, h], &settings);
    }
    for size in [16, 32, 48, 256] {
        let png = margin_shot::png::encode(size, size, &margin_theme::icon::rgba(size, 0.03)).expect("icon png");
        std::fs::write(out.join(format!("icon-{size}.png")), png).expect("write icon");
    }
}

fn shots(out: &Path, mode: &str, suffix: &str, scale: f32, size: [f32; 2], settings: &str) {
    let name = |s: &str| out.join(format!("{s}-{mode}{suffix}.png"));
    let state = "recent = /home/me/dev/parser-c\nrecent = /home/me/notes\nrecent = /home/me/dev/website\n";
    let conf = [("settings.conf", settings), ("state.conf", state)];

    // Welcome.
    let mut a = H::build(&files(), Script::default(), None, size, scale, &conf);
    a.frames(3);
    a.png_to(&name("01-welcome"));

    // Folder picker.
    a.key(Key::O, Modifiers::COMMAND);
    a.until("picker listing", |p| p.picker_open);
    a.frames(4);
    a.png_to(&name("06-picker"));

    // Folder with a code file, hints, tree.
    let mut a = H::build(&files(), Script::default(), Some("/home/me/dev/parser-c"), size, scale, &conf);
    a.opened();
    let src = a.probe().tree.iter().position(|(p, _)| p == "src").unwrap();
    let _ = src;
    a.app_open("src/parser.c");
    a.until("hints", |p| p.active_view == "text" && !p.lenses.is_empty());
    a.frames(30);
    a.until("lens numbers", |p| p.lenses.iter().any(|l| l.1.iter().any(|s| s.contains("caller"))) || true);
    a.frames(10);
    a.png_to(&name("02-code"));

    // Palette with actions and files.
    a.key(Key::P, Modifiers::COMMAND);
    a.type_text("pa");
    a.frames(10);
    a.until("palette files", |p| p.palette_files > 0);
    a.frames(3);
    a.png_to(&name("03-palette"));
    a.key(Key::Escape, Modifiers::NONE);

    // Results panel (callers).
    a.app_cmd_callers("parse_header");
    a.until("callers", |p| p.results > 0);
    a.frames(3);
    a.png_to(&name("07-results"));
    a.key(Key::Escape, Modifiers::NONE);

    // Search box in Content mode (sidebar), then Enter: a search tab.
    a.key(Key::F, Modifiers::COMMAND | Modifiers::SHIFT);
    a.type_text("header");
    a.frames(3);
    a.png_to(&name("04-search"));
    a.key(Key::Enter, Modifiers::NONE);
    a.until("search hits", |p| p.search_hits > 0 && p.search_summary.contains("ms"));
    a.frames(3);
    a.png_to(&name("08-search-tab"));

    // Replace row + confirm.
    let tab = a.probe().search_tab.unwrap();
    a.click_id(margin_gui::search_panel::replace_toggle_id(tab));
    a.type_text("hdr");
    a.frames(2);
    a.click_id(margin_gui::search_panel::replace_all_id(tab));
    a.until("confirm", |p| p.modal_title.is_some());
    a.frames(3);
    a.png_to(&name("09-replace-confirm"));
    a.key(Key::Escape, Modifiers::NONE);

    // Settings.
    a.key(Key::Comma, Modifiers::COMMAND);
    a.frames(4);
    a.png_to(&name("05-settings"));

    // Names filter with include / exclude.
    let mut a = H::build(&files(), Script::default(), Some("/home/me/dev/parser-c"), size, scale, &conf);
    a.opened();
    a.app_open("src/parser.c");
    a.key(Key::E, Modifiers::COMMAND | Modifiers::SHIFT);
    a.type_text("*.c");
    a.click_id(margin_gui::side::filters_id());
    a.click_id(margin_gui::side::glob_id(1));
    a.type_text("tests/**");
    a.until("names", |p| p.side_list.len() == 2);
    a.click_id(margin_gui::side::field_id());
    a.key(Key::ArrowDown, Modifiers::NONE);
    a.frames(3);
    a.png_to(&name("10-names-filter"));

    // Call paths (sinks: lens).
    let script = Script { paths_override: Some(Arc::new(paths())), ..Default::default() };
    let mut a = H::build(&files(), script, Some("/home/me/dev/parser-c"), size, scale, &conf);
    a.opened();
    a.app_open("src/parser.c");
    a.until("sinks hint", |p| p.lenses.iter().any(|l| l.1.iter().any(|s| s.starts_with("sinks")) && l.1.iter().any(|s| s.contains("caller"))));
    a.frames(5);
    let p = a.probe();
    let line = p.lenses.iter().find(|l| l.1.iter().any(|s| s.starts_with("sinks"))).unwrap().0;
    a.click_id(p.editor_id.unwrap().with(("lens", line, SEG_SINKS)));
    a.until("paths", |p| p.paths == 3);
    a.key(Key::ArrowDown, Modifiers::NONE);
    a.frames(3);
    a.png_to(&name("11-paths"));
    a.key(Key::Escape, Modifiers::NONE);

    // Menu bar: File open, one item selected.
    a.click_id(margin_gui::menubar::title_id(0));
    a.key(Key::ArrowDown, Modifiers::NONE);
    a.key(Key::ArrowDown, Modifiers::NONE);
    a.frames(2);
    a.png_to(&name("12-menu-open"));
    a.key(Key::Escape, Modifiers::NONE);

    // Tree context menu.
    let i = a.probe().tree.iter().position(|(p, _)| p == "src").unwrap();
    let r = a.rect_of(margin_gui::tree::tree_id().with(("row", i))).unwrap();
    right_click(&mut a, r.center());
    a.key(Key::ArrowDown, Modifiers::NONE);
    a.frames(2);
    a.png_to(&name("13-context-menu"));
    a.key(Key::Escape, Modifiers::NONE);

    // Untitled + Save As.
    a.key(Key::N, Modifiers::COMMAND);
    a.frames(1);
    a.type_text("idea: bound every copy by MAX_HEADER");
    a.key(Key::S, Modifiers::COMMAND);
    a.until("save as", |p| p.modal.as_deref() == Some("save-as"));
    a.frames(1);
    a.type_text("notes/");
    a.frames(3);
    a.png_to(&name("14-untitled-save-as"));
    a.key(Key::Escape, Modifiers::NONE);

    // Markdown preview.
    a.app_open("docs/notes.md");
    a.key(Key::V, Modifiers::COMMAND | Modifiers::SHIFT);
    a.until("preview", |p| p.preview_parsed);
    a.frames(4);
    a.png_to(&name("15-md-preview"));
}

trait Drive {
    fn app_open(&mut self, path: &str);
    fn click_id(&mut self, id: egui::Id);
    fn app_cmd_callers(&mut self, name: &str);
}

impl Drive for H {
    fn click_id(&mut self, id: egui::Id) {
        self.frames(1);
        let r = self.rect_of(id).unwrap_or_else(|| panic!("no widget {id:?}"));
        self.click(r.center());
    }

    /// Through the palette's Go to File, like a user.
    fn app_open(&mut self, path: &str) {
        self.key(Key::T, Modifiers::COMMAND);
        self.type_text(path);
        let want = path.rsplit('/').next().unwrap_or(path).to_owned();
        self.until("file match", |p| p.palette_files > 0);
        self.frames(3);
        self.key(Key::Enter, Modifiers::NONE);
        self.until("tab", move |p| p.active.as_deref() == Some(want.as_str()));
    }

    fn app_cmd_callers(&mut self, name: &str) {
        let _ = name;
        // Click the "callers" hint segment of parse_header.
        let p = self.probe();
        let id = p.editor_id.expect("editor");
        let line = p.lenses.iter().find(|l| l.1.iter().any(|s| s.contains("caller"))).map(|l| l.0).expect("a callers hint");
        let r = self.rect_of(id.with(("lens", line, SEG_CALLERS))).expect("lens rect");
        self.click(r.center());
    }
}
