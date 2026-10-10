#!/usr/bin/env bash
# Regenerate every picture on the project site (docs/index.html) with the
# real emulator, from the freely licensed test ROMs.
#
#   scripts/fetch-test-roms.sh        # once: ROMs into target/test-roms/
#   scripts/docs-screenshots.sh       # pictures into docs/assets/img/
#
# Uses the headless release build of gase (screenshots, debugger dumps,
# traces, WAV recordings) and a small helper, scripts/docs-shots, for what
# the command line cannot do: scripted button presses and pictures of the
# VDP's individual layers. scripts/png.py stores every picture as a small
# lossless palette PNG.
#
# Environment: GASE_TEST_ROMS (ROM directory, default target/test-roms),
# CARGO_TARGET_DIR (respected), OUT (default docs/assets/img).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
roms="${GASE_TEST_ROMS:-$root/target/test-roms}"
out="${OUT:-$root/docs/assets/img}"
target="${CARGO_TARGET_DIR:-$root/target}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

for rom in 240p-test-suite/240pSuite-1.23.bin genmd-imgrom-testpattern/testpattern.bin \
    airstriker/Airstriker.md right2repair-ggj2020/rom.bin resistance-the-spiral/rom.bin; do
    [[ -f "$roms/$rom" ]] || { echo "missing $roms/$rom (run scripts/fetch-test-roms.sh)" >&2; exit 1; }
done

echo "== building gase (headless) and the docs-shots helper"
cargo build --release --quiet -p gase --no-default-features --manifest-path "$root/Cargo.toml"
cargo build --release --quiet --manifest-path "$root/scripts/docs-shots/Cargo.toml"
gase="$target/release/gase"
shots="$target/release/gase-docs-shots"

mkdir -p "$out"
png() { python3 "$root/scripts/png.py" "$1" "$out/$2" >/dev/null; echo "   $2"; }

air="$roms/airstriker/Airstriker.md"
p240="$roms/240p-test-suite/240pSuite-1.23.bin"
r2r="$roms/right2repair-ggj2020/rom.bin"

echo "== screenshots (gase --headless --screenshot)"
shot() { # name frames rom [extra flags...]
    local name="$1" frames="$2" rom="$3"
    shift 3
    "$gase" --headless --frames "$frames" "$@" --screenshot "$work/$name.png" "$rom" >/dev/null 2>&1
    png "$work/$name.png" "$name.png"
}
shot 240p-menu 300 "$p240"
shot testpattern 120 "$roms/genmd-imgrom-testpattern/testpattern.bin"
shot airstriker-sega 60 "$air"
shot airstriker-error 400 "$air"
shot airstriker-title 600 "$air" --no-address-errors
shot right2repair-title 600 "$r2r"
shot spiral 1200 "$roms/resistance-the-spiral/rom.bin"

echo "== one frame of Right 2 Repair, layer by layer (scripts/docs-shots)"
# Start on the title, Start again to begin, then fight a little.
"$shots" "$r2r" 1200 "$work/r2r" --layers --tiles \
    --press start@600-606 --press start@800-806 --press a@1000-1006 --press right@1100-1300 2>/dev/null
png "$work/r2r.pam" r2r-frame.png
for part in plane-b plane-a sprites tiles cram; do
    png "$work/r2r-$part.pam" "r2r-$part.png"
done

echo "== debugger (gase --dump-debugger)"
"$gase" --headless --frames 300 --dump-debugger "$work/dbg.png" "$p240" >/dev/null 2>&1
png "$work/dbg.png" debugger-240p.png
# Airstriker stopped at its address-error handler (the vector at $00000C).
handler=$(python3 -c "import sys; d=open(sys.argv[1],'rb').read(); print('%X' % int.from_bytes(d[12:16], 'big'))" "$air")
"$gase" --headless --frames 600 --break "$handler" --dump-debugger "$work/dbg-air.png" "$air" \
    >"$work/break.txt" 2>&1
png "$work/dbg-air.png" debugger-airstriker.png

echo "== the instructions before Airstriker's address error (gase --trace)"
"$gase" --headless --frames 359 --trace 20000000 "$air" 2>/dev/null \
    | grep -m1 -B 8 "^0*${handler} " >"$out/airstriker-trace.txt" || true
cat "$out/airstriker-trace.txt"

echo "== the user interface (gase --dump-ui), desktop and phone"
# A throwaway configuration with a few recent games (one of them zipped:
# gase reads .zip files directly) so the screens look lived in. The games
# folder has a fixed name because the file browser shows its path.
lib=/tmp/gase-docs/Games
rm -rf /tmp/gase-docs
mkdir -p "$lib" "$work/cfg"
trap 'rm -rf "$work" /tmp/gase-docs' EXIT
cp "$p240" "$lib/240p Test Suite.bin"
cp "$air" "$lib/Airstriker.md"
cp "$roms/resistance-the-spiral/rom.bin" "$lib/The Spiral.bin"
cp "$r2r" "$work/Right 2 Repair.bin"
python3 -c "import sys, zipfile; z = zipfile.ZipFile(sys.argv[1], 'w', zipfile.ZIP_DEFLATED); z.write(sys.argv[2], 'Right 2 Repair.bin'); z.close()" \
    "$lib/Right 2 Repair.zip" "$work/Right 2 Repair.bin"
{
    echo "browse_dir = $lib"
    for game in "Right 2 Repair.zip" "240p Test Suite.bin" "Airstriker.md" "The Spiral.bin"; do
        echo "recent = $lib/$game"
    done
} >"$work/cfg/settings.cfg"
ui() { # name rom-or-empty [flags...]
    local name="$1" rom="$2"
    shift 2
    GASE_CONFIG_DIR="$work/cfg" "$gase" --headless "$@" --dump-ui "$work/$name.png" ${rom:+"$rom"} >/dev/null 2>&1
    png "$work/$name.png" "ui-$name.png"
}
game="$lib/Right 2 Repair.zip"
ui home ""
ui browser "" --ui-screen browser
ui controls "$game" --frames 600 --ui-screen pause,settings,controls
ui remap "$game" --frames 600 --ui-screen pause,settings,controls,remap-waiting
# A phone: 390 x 844 points at two pixels per point, held both ways.
phone=(--mobile --ui-density 2)
ui phone-home "" "${phone[@]}" --ui-size 780x1688
ui phone-game "$game" "${phone[@]}" --ui-size 780x1688 --frames 1200 --ui-screen game
ui phone-landscape "$game" "${phone[@]}" --ui-size 1688x780 --frames 1200 --ui-screen game

echo "== the browser version (needs node and the playwright package, else skipped)"
# web/tests/smoke.mjs drives the real page in headless Chromium and saves
# screenshots along the way; the home screen is the one on the site.
if node -e "require('playwright')" >/dev/null 2>&1; then
    "$root/web/build.sh" >/dev/null
    node "$root/web/tests/smoke.mjs" "$r2r" "$work/web" >/dev/null
    png "$work/web/1-home.png" web-home.png
else
    echo "   skipped (set NODE_PATH to a node_modules folder with playwright)"
fi

echo "== a waveform of Airstriker's title music (gase --wav)"
"$gase" --headless --frames 700 --no-address-errors --wav "$work/air.wav" "$air" >/dev/null 2>&1
python3 "$root/scripts/wave-svg.py" "$work/air.wav" "$out/wave-zoom.svg" 8.0 0.012
python3 "$root/scripts/wave-svg.py" "$work/air.wav" "$out/wave-overview.svg" 6.0 5.5 --envelope

du -ch "$out"/* | tail -1
