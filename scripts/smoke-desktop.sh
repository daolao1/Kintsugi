#!/usr/bin/env bash
#
# The desktop shell's smoke test: not "it compiled", but "it mounted a game
# and drew it". The window itself needs a screen, so CI drives the headless
# twin the shell keeps for exactly this — the same renderer, the same seam,
# writing the frames the window would show.
#
# The game is synthesized fresh, like scripts/smoke.sh does — the repository
# carries no game bytes — so pass the CLI beside the shell:
#
#   scripts/smoke-desktop.sh target/release/kintsugi-desktop target/release/kintsugi
#   scripts/smoke-desktop.sh target/x86_64-pc-windows-msvc/release/kintsugi-desktop.exe \
#       target/x86_64-pc-windows-msvc/release/kintsugi.exe "$TEMP/smoke"
#
# With no CLI argument it plays the local demo-game/ directory instead, which
# `kintsugi demo --dir demo-game` writes for development.
#
# It works in a temporary directory and removes nothing outside it.

set -euo pipefail

exe=${1:?usage: smoke-desktop.sh path/to/kintsugi-desktop [path/to/kintsugi] [workdir]}
cli=${2:-}
work=${3:-$(mktemp -d "${TMPDIR:-/tmp}/kintsugi-desktop-smoke-XXXXXX")}

mkdir -p "$work"

if [ -n "$cli" ]; then
    game="$work/game"
    "$cli" demo --dir "$game" > /dev/null
else
    game="demo-game"
fi

"$exe" "$game" --dump-frames "$work/frames" --max-frames 3

for frame in frame-0001.png frame-0002.png frame-0003.png; do
    if [ ! -s "$work/frames/$frame" ]; then
        echo "smoke: $frame was not written" >&2
        exit 1
    fi
done

echo "desktop smoke: three frames drawn and written"
