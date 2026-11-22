# Dependencies

Versions vetted on 2026-11-20 (first release ≥ 90 days, download thresholds, known owners, repository link, 7-day cooldown). `Cargo.lock` was seeded from an already vetted lockfile; nothing newer was pulled.

## Direct

| Crate | Locked | Used by | Why |
|---|---|---|---|
| eframe / egui | 0.37.0 | gui, editor, theme | window + immediate-mode UI (wgpu, no webview) |
| wgpu, egui-wgpu | 30.0.2 / 0.37.0 | margin-shot (dev) | offscreen render for tests and screenshots; already pulled in by eframe |
| regex | 1.13.1 | engine | content search (linear time) |
| memchr | 2.8.3 | engine | substring search |
| rayon | 1.12.0 | engine | parallel walk, index, search |
| flate2 | 1.1.x | margin-shot (dev) | PNG encoder; already in the lock through eframe |

Worker process (the one that reads hostile files) links: `regex`, `memchr`, `rayon` and their dependencies (`aho-corasick`, `regex-automata`, `regex-syntax`, `rayon-core`, `crossbeam-*`, `either`). Nothing else third-party runs its own logic there.

## Written here instead of pulled in

| Not used | Instead |
|---|---|
| tree-sitter + grammars (C) | `margin-code`: lexers and outlines in safe Rust |
| ignore / globset / walkdir | `margin-engine`: walker, gitignore globs |
| nucleo / fuzzy-matcher | `margin-engine::fuzzy` |
| notify (fs watcher) | re-scan on window focus and after writes |
| serde, bincode, toml | `margin-proto::wire` (bounds-checked binary codec); `key = value` settings |
| clap | `apps/margin/src/args.rs` |
| rfd (native file dialog) | folder picker overlay |
| dirs | `apps/margin/src/dirs.rs` |
| syntect | `margin-code` |
| tokio / async | threads + channels |
| libc, objc sandbox FFI | `/usr/bin/sandbox-exec` with a profile |
| tempfile | `margin-testutil::TempDir` |

## Adding one

Ask first. Check age, downloads, owners, repository, build scripts. Then `[workspace.dependencies]`, a row here, commit `Cargo.lock`. Build with `--locked`.
