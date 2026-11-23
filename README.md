# Margin

Small, fast editor for notes, files and reading code. Rust, native GUI (egui/wgpu), no webview, no extensions. Opened folders are treated as hostile.

## Run

Builds (unsigned): [releases](https://github.com/plowsec/padawan/releases). Check against `SHA256SUMS`. macOS: `xattr -d com.apple.quarantine margin` after unpacking.

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
| macOS arm64 | built and tested; both sandboxes on (`margin --check-sandbox`) |
| macOS x86_64 | built in CI, never run |
| Linux x86_64 / arm64, Windows x86_64 | built in CI; CI starts it, opens a folder, indexes, draws (`MARGIN_SMOKE=1`). Not used by hand. No OS sandbox (in-process checks only) |

Linux needs X11 or Wayland, `libxkbcommon`, a Vulkan or OpenGL driver, glibc 2.35+.

## Keys

`cmd` = Cmd on macOS, Ctrl elsewhere. Every action is in the palette and the menu bar, with its shortcut.

| Key | Does |
|---|---|
| `ctrl+p` / `cmd+p` | palette: actions and files. `>` actions only, `:42` go to line, `@` functions of the file |
| `cmd+t` | go to file |
| `cmd+o`, `cmd+shift+n` | open folder, new window |
| `cmd+n` | new untitled tab (no file until you save) |
| `cmd+s`, `cmd+shift+s`, `cmd+alt+s` | save, save as, save all |
| `cmd+w` | close tab |
| `cmd+shift+e` | sidebar search: file names |
| `cmd+shift+f`, `cmd+shift+h` | search in folder, replace in folder (opens a search tab) |
| `cmd+f`, `cmd+alt+f` | find, replace in file |
| `cmd+g` / `f3`, with `shift` | find next, previous (`f3` only off macOS) |
| `f12` / cmd+click, `shift+f12` | go to definition, callers |
| `ctrl+g`, `cmd+shift+o` | go to line, go to function in file |
| `ctrl+-`, `ctrl+shift+-` | back, forward (`alt+←` / `alt+→` off macOS) |
| `ctrl+tab`, `ctrl+shift+tab` | next, previous tab |
| `alt+↑` / `alt+↓`, `shift+alt+↓`, `cmd+shift+k` | move line, duplicate line, delete line |
| `cmd+]`, `cmd+[`, `cmd+/` | indent, outdent, toggle comment |
| `cmd+shift+v` | Markdown preview |
| `cmd+b`, `alt+z` | sidebar, word wrap |
| `cmd+,` | settings |
| `cmd+=` `cmd+-` `cmd+0` | zoom |

Right-click: editor, file tree, tabs, search results.

## Search

One box at the top of the sidebar.

| Mode | Typing | Result |
|---|---|---|
| Names | `todo`, `*.md`, `src/**/*.c` | fuzzy or glob on file names; the tree becomes the list of matches |
| Content | text, or a regex with `.*` on | Enter opens a `Search: …` tab; several can be open |

| Filter | Example | Rule |
|---|---|---|
| include | `*.rs, src/**` | a file must match one |
| exclude | `target/**, *.min.js` | a file must match none |
| `!` in include | `*.c, !tests/**` | same as an exclude |

Replace (search tab): `Replace All` or per file. Regex: `$1`, `${name}`. Files are rewritten on disk after a confirmation; not undoable. Tabs with unsaved edits are skipped.

## Notes and Markdown

| What | How |
|---|---|
| Quick note | `cmd+n`. Kept in `~/.config/margin/untitled/` until saved or discarded; restored at start |
| Save it | `cmd+s` asks for a path in the open folder |
| Preview | `cmd+shift+v`: editor left, rendered right, live |
| Links in the preview | `#heading` and files of the folder open; URLs are copied, never opened |
| Images, HTML | never loaded, never interpreted |

## Hints above functions

`3 callers · 5 callees · sinks: memcpy, system · reaches: exec`

| Segment | Means | Click |
|---|---|---|
| callers | call sites of that name in the folder | list them |
| callees | distinct functions it calls | list the calls |
| sinks | dangerous calls made directly (exec, memory, eval, deserialize, sql, file, format, network, html) | call paths |
| reaches | sinks reachable through calls | call paths |

Call paths: one group per sink call, one row per call on the way, and under each call `argument ← inputs`.

| Shown | Means |
|---|---|
| `n ← len` | parameter `len` of the enclosing function reaches argument `n`, directly or through local assignments |
| `input reaches sink` | a parameter of the first function is tracked into a sink argument through every call |
| `no tracked input` | nothing was tracked; not a proof of safety |

By name, from token heuristics: leads for a reader, not proofs. Not tracked: return values, globals, fields read back, branches. Languages: C, C++, Rust, Go, Java, JavaScript, TypeScript, Python, C#, PHP, Ruby, Kotlin, Swift, Lua, Shell.

## Layout

| Path | What |
|---|---|
| `apps/margin` | the binary: launcher, sandbox profiles, worker and GUI roles |
| `crates/margin-proto` | messages GUI ⇄ engine, bounds-checked codec, limits |
| `crates/margin-engine` | everything that touches the folder: jail, walk, index, search, call graph, cache |
| `crates/margin-code` | lexers, outlines, sink tables. Safe Rust, no dependency |
| `crates/margin-editor` | the editor widget |
| `crates/margin-md` | Markdown parser and preview. Safe Rust, written here |
| `crates/margin-gui` | window: sidebar, tabs, menus, palette, search tabs, call paths, settings |
| `crates/margin-theme` | palettes, fonts, egui style |
| `crates/margin-shot`, `margin-testutil` | headless render to PNG; `TempDir` for tests |
| `docs/security.md` | threat model, sandbox, known limits |
| `docs/deps.md` | dependencies and what was written here instead |

## Files

| What | Where |
|---|---|
| Settings, recent folders | `~/.config/margin/settings.conf`, `state.conf` |
| Unsaved untitled tabs | `~/.config/margin/untitled/` |
| Index cache (per folder) | `~/Library/Caches/margin/roots/<hash>/` (Linux: `~/.cache/margin`) |
| Windows | `%APPDATA%\margin` (settings, untitled), `%LOCALAPPDATA%\margin` (cache) |

Nothing is written into an opened folder except the files you save.

## Dev

```bash
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
cargo run -p margin-gui --example app_shot -- /abs/out/dir     # screenshots of the main states
scripts/sign.sh                                                # macOS: codesign target/release/margin (ad hoc; MARGIN_SIGN_ID="<identity>" for yours)
MARGIN_SMOKE=1 target/debug/margin /abs/folder                 # start, index, print `smoke: ok …`, exit
MARGIN_SMOKE=clipboard target/debug/margin /abs/folder         # same + copy/paste round trip (overwrites the clipboard)
MARGIN_SMOKE=newwindow target/debug/margin /abs/folder         # same + a second window
git tag vX.Y.Z && git push origin main vX.Y.Z                  # release: CI builds all platforms and publishes (.github/workflows/build.yml)
scripts/third-party.py <target-triple>                         # crates in the binary + licence texts (shipped as THIRD-PARTY.txt)
```

| Rule | Detail |
|---|---|
| Unsafe | forbidden workspace-wide |
| stdout | IPC channel of the gui and worker roles: no `println!` (clippy denies it) |
| Deletes | `remove_dir_all` banned by clippy; tests use `margin_testutil::TempDir` |
| UI thread | no I/O, no parsing of big buffers; everything visible is virtualised |
| New dependency | ask first (`docs/deps.md`) |
