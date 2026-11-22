//! Render the key states of the app to PNG with the fake engine.
//!
//! `cargo run -p margin-gui --example app_shot -- <abs out dir> [scale] [width height] [ui_size]`

#[path = "../tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};

use common::{Script, H};
use egui::{Key, Modifiers};

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

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("README.md", "# parser-c\n\nA tiny header parser.\n"),
        ("Makefile", "all:\n\tcc -o parser src/parser.c\n"),
        ("src/parser.c", PARSER_C),
        ("src/net/socket.c", "int open_socket(int port) {\n    return port;\n}\n"),
        ("src/net/socket.h", "int open_socket(int port);\n"),
        ("src/util.h", "#pragma once\n"),
        ("tests/test_parser.c", "int test_parse(void) {\n    struct header h;\n    return parse_header(\"a: b\", &h);\n}\n"),
        ("docs/notes.md", "Notes\n"),
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

    // Search.
    a.key(Key::F, Modifiers::COMMAND | Modifiers::SHIFT);
    a.type_text("header");
    a.until("search hits", |p| p.search_hits > 0);
    a.frames(3);
    a.png_to(&name("04-search"));

    // Settings.
    a.key(Key::Comma, Modifiers::COMMAND);
    a.frames(4);
    a.png_to(&name("05-settings"));
}

trait Drive {
    fn app_open(&mut self, path: &str);
    fn app_cmd_callers(&mut self, name: &str);
}

impl Drive for H {
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
        let r = self.rect_of(id.with(("lens", line, 0usize))).expect("lens rect");
        self.click(r.center());
    }
}
