#!/usr/bin/env bash
# Download the SingleStepTests Z80 JSON vectors (https://github.com/SingleStepTests/z80)
# into target/test-vectors/z80/ (or $1). They are large (~1.5 GB) and not committed.
#
# Run the suite afterwards with:
#   cargo test -p gase-z80 --profile fast-test -- --ignored singlestep
#
# The upstream directory listing lives on github.com, which we may not be able to
# reach, so we instead try every plausible opcode file name on
# raw.githubusercontent.com and skip the ones that do not exist (e.g. there is no
# "ed 00.json": unassigned ED opcodes are not part of the suite).
set -euo pipefail

BASE="${Z80_TESTS_URL:-https://raw.githubusercontent.com/SingleStepTests/z80/main/v1}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${1:-$ROOT/target/test-vectors/z80}"
JOBS="${JOBS:-16}"
mkdir -p "$DEST"

names() {
    for i in $(seq 0 255); do
        local h
        h=$(printf '%02x' "$i")
        case "$h" in
            cb | dd | ed | fd) ;;
            *) echo "$h" ;;
        esac
        echo "cb $h"
        echo "ed $h"
        for p in dd fd; do
            [ "$h" != cb ] && echo "$p $h"
            echo "$p cb __ $h"
        done
    done
}

fetch_one() {
    local name="$1" dest="$2" base="$3"
    local file="$dest/$name.json"
    [ -s "$file" ] && return 0
    local url="$base/${name// /%20}.json"
    local code
    code=$(curl -sS --retry 3 -o "$file.part" -w '%{http_code}' "$url") || code=000
    if [ "$code" = 200 ]; then
        mv "$file.part" "$file"
        echo "fetched $name"
    else
        rm -f "$file.part"
        [ "$code" = 404 ] || echo "FAILED $name (HTTP $code)" >&2
    fi
}
export -f fetch_one

names | xargs -P "$JOBS" -I{} bash -c 'fetch_one "$1" "$2" "$3"' _ {} "$DEST" "$BASE"
echo "$(find "$DEST" -name '*.json' | wc -l) vector files in $DEST"

# ZEXDOC / ZEXALL (Frank Cringle's instruction exercisers, CP/M .com programs),
# from a mirror on raw.githubusercontent.com. Run them with:
#   cargo test -p gase-z80 --profile fast-test -- --ignored zex
ZEX_BASE="${ZEX_URL:-https://raw.githubusercontent.com/floooh/chips-test/master/tests/roms}"
for prog in zexdoc zexall; do
    if [ ! -s "$DEST/$prog.com" ]; then
        curl -sS --retry 3 -f -o "$DEST/$prog.com" "$ZEX_BASE/$prog.com" && echo "fetched $prog.com" ||
            echo "FAILED $prog.com" >&2
    fi
done
