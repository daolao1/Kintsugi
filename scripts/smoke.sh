#!/usr/bin/env bash
#
# The whole repair loop, end to end, through a *built* binary.
#
# This is the check that a platform is actually maintained: not that it
# compiles, but that a person on it can synthesize a game, translate its script,
# install the repair into a copy, and play the copy. CI runs exactly this file
# on macOS, Windows, and Linux, so a green run of this script locally means what
# a green job means.
#
#   scripts/smoke.sh target/release/kintsugi
#   scripts/smoke.sh target/x86_64-pc-windows-msvc/release/kintsugi.exe "$TEMP/smoke"
#
# It works in a temporary directory and removes nothing outside it.

set -euo pipefail

exe=${1:?usage: smoke.sh path/to/kintsugi [workdir]}
work=${2:-}

if [ ! -x "$exe" ] && [ ! -f "$exe" ]; then
  echo "smoke: no binary at $exe (build one first)" >&2
  exit 2
fi
if [ -z "$work" ]; then
  work=$(mktemp -d "${TMPDIR:-/tmp}/kintsugi-smoke-XXXXXX")
fi
mkdir -p "$work"

# No colour: this script asserts on the words the binary prints, and a repair
# report decorated with escape codes is a bad thing to assert against.
export NO_COLOR=1

demo="$work/demo"
patch="$work/story.en.bdt"
copy="$work/repaired"
rm -rf "$demo" "$copy"

"$exe" version
"$exe" demo --dir "$demo" | tee "$work/demo.txt"
grep -q "bluegale" "$work/demo.txt"
grep -q "修复完成" "$work/demo.txt"
test -f "$demo/title-x4-anime4k.png"

"$exe" translate "$demo" --mock --no-play --write-script "$patch" > "$work/translate.txt"
"$exe" install "$demo" --script "$patch" --into "$copy" | tee "$work/install.txt"
grep -q "the original game folder is untouched" "$work/install.txt"
grep -q "installed story.bdt into the copy" "$work/install.txt"
test -f "$copy/.kintsugi-install"
cmp "$patch" "$copy/story.bdt"

"$exe" play "$copy" --auto | tee "$work/play.txt" > /dev/null
grep -q "mock: " "$work/play.txt"

# The exit-code contract, which scripts in the wild branch on: a mistyped
# command line is the caller's fault (2), not the game's (1).
code=0
"$exe" --faktur 4 > /dev/null 2>&1 || code=$?
if [ "$code" -ne 2 ]; then
  echo "smoke: a mistyped flag exited $code, not 2" >&2
  exit 1
fi

# A flag that exists but belongs to another command is a mistake too, not
# something to accept and ignore.
code=0
"$exe" detect "$demo" --factor 4 > /dev/null 2>&1 || code=$?
if [ "$code" -ne 2 ]; then
  echo "smoke: 'detect --factor 4' exited $code, not 2" >&2
  exit 1
fi

# Asking for help anywhere is answered, and answered successfully.
"$exe" translate --help | grep -q "Usage:"

echo "smoke: the whole repair loop works through $exe"
