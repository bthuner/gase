#!/bin/sh
# Measure emulation speed over the test ROMs and print a table of medians.
#
#   benchmarks/run.sh                       # build release, 5 rounds of 3000 frames
#   ROUNDS=9 FRAMES=1000 benchmarks/run.sh  # tune the run
#   benchmarks/run.sh old/gase new/gase     # A/B compare two existing binaries
#
# Each round runs every (binary, ROM) pair once, so a burst of noise from
# other processes on the machine hits all of them alike instead of skewing
# one; the median over the rounds then discards the outliers. The emulator
# runs headless with `--bench`, which times only the emulation loop (no
# window, no audio device, no frame pacing).
#
# The ROMs are the ones fetched by scripts/fetch-test-roms.sh. For a
# noise-free number use benchmarks/callgrind.sh instead (see
# benchmarks/README.md).
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROMS="${GASE_TEST_ROMS:-$ROOT/target/test-roms}"
ROUNDS="${ROUNDS:-5}"
FRAMES="${FRAMES:-3000}"

if [ $# -eq 0 ]; then
    cargo build --release -p gase --quiet --manifest-path "$ROOT/Cargo.toml"
    set -- "${CARGO_TARGET_DIR:-$ROOT/target}/release/gase"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# name  path (relative to $ROMS)
cat > "$TMP/roms" <<EOF
240p-suite      240p-test-suite/240pSuite-1.23.bin
testpattern     genmd-imgrom-testpattern/testpattern.bin
airstriker      airstriker/Airstriker.md
right2repair    right2repair-ggj2020/rom.bin
the-spiral      resistance-the-spiral/rom.bin
EOF

round=1
while [ "$round" -le "$ROUNDS" ]; do
    printf 'round %s/%s\r' "$round" "$ROUNDS" >&2
    b=0
    for bin in "$@"; do
        b=$((b + 1))
        while read -r name rom; do
            [ -f "$ROMS/$rom" ] || continue
            # The bench line reads "N frames in T: F fps (Xx real time)".
            "$bin" --headless --bench --frames "$FRAMES" "$ROMS/$rom" 2>&1 \
                | awk '/ fps /{for (i = 1; i < NF; i++) if ($(i + 1) == "fps") print $i}' \
                >> "$TMP/$b.$name"
        done < "$TMP/roms"
    done
    round=$((round + 1))
done
printf '\n' >&2

median() { sort -n "$1" | awk '{v[NR] = $1} END {print (NR % 2) ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2}'; }

printf '%-14s' "ROM"
b=0
for bin in "$@"; do b=$((b + 1)); printf '%14s' "fps #$b"; done
[ $# -gt 1 ] && printf '%10s' "speedup"
printf '\n'
while read -r name rom; do
    [ -f "$TMP/1.$name" ] || continue
    printf '%-14s' "$name"
    b=0
    for bin in "$@"; do
        b=$((b + 1))
        printf '%14s' "$(median "$TMP/$b.$name")"
    done
    if [ $# -gt 1 ]; then
        printf '%9.2fx' "$(echo "$(median "$TMP/$b.$name") $(median "$TMP/1.$name")" | awk '{print $1 / $2}')"
    fi
    printf '\n'
done < "$TMP/roms"
b=0
for bin in "$@"; do b=$((b + 1)); echo "#$b = $bin"; done
echo "median of $ROUNDS rounds of $FRAMES frames"
