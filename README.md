# Margin

Small, fast editor for notes, files and reading code. Rust, native GUI (egui/wgpu), no webview, no extensions. Opened folders are treated as hostile.

## Run

```bash
cargo build --release --locked            # binary: target/release/margin
margin                                    # welcome screen
margin /abs/folder                        # open a folder
margin /abs/folder/file.md                # open its folder and the file
margin --check-sandbox                    # try to break out of both sandboxes, print the result
margin --no-sandbox /abs/folder           # one process, no OS sandbox (debugging, non-macOS)
```

## Status

| Platform | State |
|---|---|
| macOS | built and tested; both sandboxes on (`margin --check-sandbox`) |
| Linux, Windows | not built or tested yet; no OS sandbox (in-process checks only) |

## Keys

`cmd` = Cmd on macOS, Ctrl elsewhere.

| Key | Does |
|---|---|
| `ctrl+p` / `cmd+p` | palette: actions and files. `>` actions only, `:42` go to line, `@` functions of the file |
| `cmd+t` | go to file |
| `cmd+o` | open folder |
| `cmd+shift+f` | search in folder (text or regex, globs) |
| `cmd+f`, `cmd+alt+f` | find, replace in file |
| `cmd+g` / `f3`, with `shift` | find next, previous (`f3` only off macOS) |
| `cmd+s`, `cmd+w`, `cmd+n` | save, close tab, new file |
| `f12` / cmd+click | go to definition |
| `shift+f12` | callers of the word under the cursor |
| `ctrl+g`, `cmd+shift+o` | go to line, go to function in file |
| `ctrl+-`, `ctrl+shift+-` | back, forward (`alt+←` / `alt+→` off macOS) |
| `ctrl+tab`, `ctrl+shift+tab` | next, previous tab |
| `cmd+b` | sidebar |
| `cmd+,` | settings (theme, accent, font sizes, line height, wrap, hints, index) |
| `cmd+=` `cmd+-` `cmd+0` | zoom |

Every action is in the palette, with its shortcut.

## Hints above functions

`3 callers · 5 callees · sinks: memcpy, system · reaches: exec`

| Segment | Means | Click |
|---|---|---|
| callers | call sites of that name in the folder | list them |
| callees | distinct functions it calls | list the calls |
| sinks | dangerous calls made directly (exec, memory, eval, deserialize, sql, file, format, network, html) | list them |
| reaches | sinks reachable through calls | list them with the call chain |

By name, from token heuristics: leads for a reader, not proofs. Languages: C, C++, Rust, Go, Java, JavaScript, TypeScript, Python, C#, PHP, Ruby, Kotlin, Swift, Lua, Shell.

## Layout

| Path | What |
|---|---|
| `apps/margin` | the binary: launcher, sandbox profiles, worker and GUI roles |
| `crates/margin-proto` | messages GUI ⇄ engine, bounds-checked codec, limits |
| `crates/margin-engine` | everything that touches the folder: jail, walk, index, search, call graph, cache |
| `crates/margin-code` | lexers, outlines, sink tables. Safe Rust, no dependency |
| `crates/margin-editor` | the editor widget |
| `crates/margin-gui` | window: tree, tabs, palette, search, settings, status bar |
| `crates/margin-theme` | palettes, fonts, egui style |
| `crates/margin-shot`, `margin-testutil` | headless render to PNG; `TempDir` for tests |
| `docs/security.md` | threat model, sandbox, known limits |
| `docs/deps.md` | dependencies and what was written here instead |

## Files

| What | Where |
|---|---|
| Settings, recent folders | `~/.config/margin/settings.conf`, `state.conf` |
| Index cache (per folder) | `~/Library/Caches/margin/roots/<hash>/` (Linux: `~/.cache/margin`) |

Nothing is written into an opened folder except the files you save.

## Dev

```bash
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
cargo run -p margin-gui --example app_shot -- /abs/out/dir     # screenshots of the main states
MARGIN_SMOKE=1 target/debug/margin /abs/folder                 # start, index, print `smoke: ok …`, exit
```

| Rule | Detail |
|---|---|
| Unsafe | forbidden workspace-wide |
| stdout | IPC channel of the gui and worker roles: no `println!` (clippy denies it) |
| Deletes | `remove_dir_all` banned by clippy; tests use `margin_testutil::TempDir` |
| UI thread | no I/O, no parsing of big buffers; everything visible is virtualised |
| New dependency | ask first (`docs/deps.md`) |
