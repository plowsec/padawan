#!/bin/sh
# Sign a built binary (macOS). Usage: scripts/sign.sh [path]   default: target/release/margin
#
# | Env | Default | Meaning |
# |---|---|---|
# | MARGIN_SIGN_ID | - | codesign identity; `-` = ad hoc. List yours: security find-identity -v -p codesigning |
# | MARGIN_SIGN_TIMESTAMP | 0 | 1 = ask Apple's timestamp server (needed for notarization) |
#
# Hardened runtime is on. No entitlements: the app needs none.
set -eu
bin=${1:-target/release/margin}
id=${MARGIN_SIGN_ID:--}
[ -f "$bin" ] || { echo "sign: no such file: $bin" >&2; exit 1; }
if [ "${MARGIN_SIGN_TIMESTAMP:-0}" = 1 ]; then ts=--timestamp; else ts=--timestamp=none; fi
codesign --force --sign "$id" --identifier io.github.plowsec.margin --options runtime "$ts" "$bin"
codesign --verify --strict --verbose=2 "$bin"
codesign -dv "$bin" 2>&1 | grep -E "^(Identifier|Authority|TeamIdentifier|Signature|CodeDirectory)" || true
