#!/bin/sh
# Count the instructions the emulator executes per test ROM, with valgrind's
# callgrind, and print a table. Unlike frames per second this number does
# not depend on what else the machine is doing, so it shows changes of a
# percent or less reliably.
#
#   benchmarks/callgrind.sh                  # build release, 300 frames per ROM
#   FRAMES=600 benchmarks/callgrind.sh       # longer runs
#   benchmarks/callgrind.sh path/to/gase     # an existing binary
#
# The profiles are kept in $OUT (default target/callgrind/) for a closer
# look: `callgrind_annotate --inclusive=yes target/callgrind/airstriker.out`
# lists the most expensive functions, `--auto=yes` annotates source lines.
# The release profile keeps line tables (debug = "line-tables-only") so the
# annotations point at real source lines. The ROMs run in parallel.
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROMS="${GASE_TEST_ROMS:-$ROOT/target/test-roms}"
FRAMES="${FRAMES:-300}"
OUT="${OUT:-$ROOT/target/callgrind}"

if [ $# -eq 0 ]; then
    cargo build --release -p gase --quiet --manifest-path "$ROOT/Cargo.toml"
    set -- "${CARGO_TARGET_DIR:-$ROOT/target}/release/gase"
fi
BIN="$1"
mkdir -p "$OUT"

ROMLIST="240p-suite:240p-test-suite/240pSuite-1.23.bin
testpattern:genmd-imgrom-testpattern/testpattern.bin
airstriker:airstriker/Airstriker.md
right2repair:right2repair-ggj2020/rom.bin
the-spiral:resistance-the-spiral/rom.bin"

for entry in $ROMLIST; do
    name="${entry%%:*}"
    rom="$ROMS/${entry#*:}"
    [ -f "$rom" ] || continue
    valgrind --tool=callgrind --callgrind-out-file="$OUT/$name.out" \
        "$BIN" --headless --frames "$FRAMES" "$rom" > /dev/null 2> "$OUT/$name.log" &
done
wait

printf '%-14s %16s\n' "ROM" "instructions"
for entry in $ROMLIST; do
    name="${entry%%:*}"
    [ -f "$OUT/$name.log" ] || continue
    ir=$(sed -n 's/.*Collected : \([0-9]*\).*/\1/p' "$OUT/$name.log")
    printf '%-14s %16s\n' "$name" "$ir"
done
echo "$FRAMES frames per ROM; profiles in $OUT"
