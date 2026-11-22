# Security

An opened folder is hostile: names, contents, symlinks, special files, `.gitignore`, sizes.

## Processes

```
launcher (no sandbox, reads no file content)
 ├─ gui     Seatbelt: no network, no exec, no user files
 └─ worker  Seatbelt deny-default: the folder + its cache dir     (one per open folder)
```

| Process | Network | Exec / fork | Reads | Writes |
|---|---|---|---|---|
| launcher | not used | starts the two others | directory names (folder picker) | creates config + cache dirs |
| gui | denied | denied | system files, own binary, `~/.config/margin` | `~/.config/margin`, per-user temp (Metal shader cache) |
| worker | denied | denied | system libraries, own binary, the folder, its cache dir | the folder, its cache dir |

- Profiles: `apps/margin/src/sandbox.rs`. Paths go in as `-D` parameters, never into the profile text.
- A sandboxed process cannot sandbox its children again, so the launcher stays outside. It relays frames by length and never decodes engine messages.
- `--no-sandbox`: one process, no OS sandbox. The status bar shows `no sandbox`.
- Linux / Windows: no OS sandbox yet. Same code paths, in-process checks only.

Check the profiles on your machine:

```bash
cargo build --offline -p margin --example sandbox_probe
```

Then run `target/debug/examples/sandbox_probe` under `sandbox-exec` with a profile from `sandbox.rs`; it prints what was refused.

## Folders that cannot be opened

`/`, the home directory or anything above it, `~/Library`, `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.kube`, `~/.docker`, `~/.config`, `~/.local`, `~/.cargo`, `~/.rustup`, `~/.claude`, Margin's own config and cache dirs. Symlinks are resolved first (`sandbox::check_root`).

Why: the worker can write in the folder; a write there would mean code execution or stolen credentials later.

## What never happens

| Never | Because |
|---|---|
| Run anything from the folder | no tasks, hooks, extensions, language servers, build scripts |
| Read settings from the folder | config lives in `~/.config/margin` only; `.gitignore` is the one file interpreted, as glob patterns |
| Open a URL, start a program | the GUI has no code path for it and the sandbox denies it |
| Follow a symlink out of the folder | `margin-engine` jail: canonical path must stay inside; directory symlinks are not traversed |
| Open a FIFO, socket or device | `O_NOFOLLOW \| O_NONBLOCK`, then the open handle must be a regular file |
| Recursive delete | files, symlinks and empty directories only |
| Parse hostile bytes with C code | no tree-sitter, no libgit2, no webview. Lexers and outlines are safe Rust (`margin-code`) |
| `unsafe` in this workspace | `unsafe_code = "forbid"` |
| Trust a peer process | every message is bounds-checked on decode (`margin-proto::wire`); a bad frame ends the link |

## Display

| Threat | Handling |
|---|---|
| Bidi overrides, zero-width and control characters in code (Trojan Source) | drawn as visible `U+XXXX` boxes in the editor |
| Same in file names, snippets, error text | replaced by a placeholder before display (`margin-gui::sanitize`) |
| Huge files, long lines, deep trees | caps in `margin-proto::limits`; the editor lays out visible rows only |
| Regex denial of service | Rust `regex` (linear time) with size limits |
| Poisoned index cache | cache lives outside the folder, is parsed as untrusted, and each entry is checked against size + mtime |

## Known limits

| Limit | Detail |
|---|---|
| Launcher is not sandboxed | it parses only frames from the GUI and its own arguments |
| Check-then-open race in the jail | no `openat` without `unsafe`; the worker sandbox is the backstop |
| GUI compromise + launcher | a subverted GUI could ask for a worker on any folder the policy above allows |
| Renderer | text shaping (egui) and Metal (wgpu) run in the GUI process on hostile text |
| Call graph | by name, from token heuristics: hints are leads, not proofs |
