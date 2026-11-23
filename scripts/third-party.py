#!/usr/bin/env python3
"""Third-party crates linked into the margin binary for one target, with their licence texts.

Usage: scripts/third-party.py <target-triple> > THIRD-PARTY.txt
Reads `cargo metadata --locked` (crates must be fetched). Build and dev dependencies are left out.
"""
import hashlib
import json
import os
import subprocess
import sys

PREFIXES = ("license", "licence", "copying", "notice", "unlicense", "copyright")
FONT_LICENCES = ("ofl.txt", "ufl.txt")  # fonts bundled by egui


def linked(meta):
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"] if p["name"] == "margin")
    seen, todo = set(), [root]
    while todo:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]):
                todo.append(dep["pkg"])
    return seen


def licence_files(crate_dir):
    out = []
    for base, dirs, files in os.walk(crate_dir):
        dirs[:] = sorted(d for d in dirs if d not in ("target", ".git"))
        for f in sorted(files):
            low = f.lower()
            if low.startswith(PREFIXES) or low in FONT_LICENCES:
                out.append(os.path.join(base, f))
    return out


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", sys.argv[1]]
    meta = json.loads(subprocess.check_output(cmd))
    ids = linked(meta)
    pkgs = sorted((p for p in meta["packages"] if p["id"] in ids and p["source"]), key=lambda p: (p["name"], p["version"]))

    texts = {}  # sha256 -> (text, [crate labels])
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")  # Windows: not cp1252, not CRLF
    w = sys.stdout.write
    w(f"Third-party crates in margin ({sys.argv[1]}): {len(pkgs)}\n\n")
    for p in pkgs:
        label = f'{p["name"]} {p["version"]}'
        w(f'{label} | {p.get("license") or "see licence file"} | {p.get("repository") or ""}\n')
        for path in licence_files(os.path.dirname(p["manifest_path"])):
            with open(path, encoding="utf-8", errors="replace") as fh:
                text = fh.read().replace("\r\n", "\n").strip()
            if text:
                key = hashlib.sha256(text.encode()).hexdigest()
                texts.setdefault(key, (text, []))[1].append(label)
    for text, labels in texts.values():
        w("\n" + "=" * 78 + "\n" + ", ".join(dict.fromkeys(labels)) + "\n" + "=" * 78 + "\n\n" + text + "\n")


main()
