#!/usr/bin/env sh
# Download the 68000 single-instruction test vectors used by the gase-m68k
# test suite:
#
# * Tom Harte's vectors (https://github.com/TomHarte/ProcessorTests,
#   680x0/68000/v1), unpacked into target/test-vectors/m68000/ for
#   `crates/m68k/tests/tomharte.rs` (~1 M tests, ~1.1 GB unpacked);
# * the SingleStepTests/m68000 vectors, generated from MAME's microcode-level
#   68000, into target/test-vectors/m68000-mame/ for
#   `crates/m68k/tests/mame.rs` (~300 k tests, binary format, ~150 MB).
#   Where the two sets disagree, the MAME set follows the real microcode.
#
#     scripts/fetch-m68k-tests.sh
#     cargo test -p gase-m68k --profile fast-test -- --ignored
#
# Nothing here is committed (target/ is gitignored). Re-running skips
# files that are already present.
set -eu

BASE="https://raw.githubusercontent.com/TomHarte/ProcessorTests/main/680x0/68000/v1"
MAME_BASE="https://raw.githubusercontent.com/SingleStepTests/m68000/main/v1"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${GASE_M68K_TESTS:-$ROOT/target/test-vectors/m68000}"
MAME_DEST="${GASE_M68K_MAME_TESTS:-$ROOT/target/test-vectors/m68000-mame}"
mkdir -p "$DEST" "$MAME_DEST"

# One file per operation (sized variants carry a .b/.w/.l suffix).
sized() { for s in b w l; do echo "$1.$s"; done; }
NAMES="
ABCD SBCD NBCD
$(sized ADD) $(sized ADDX) ADDA.w ADDA.l
$(sized SUB) $(sized SUBX) SUBA.w SUBA.l
$(sized CMP) CMPA.w CMPA.l
$(sized AND) $(sized OR) $(sized EOR)
ANDItoCCR ANDItoSR ORItoCCR ORItoSR EORItoCCR EORItoSR
$(sized NEG) $(sized NEGX) $(sized NOT) $(sized CLR) $(sized TST)
$(sized ASL) $(sized ASR) $(sized LSL) $(sized LSR)
$(sized ROL) $(sized ROR) $(sized ROXL) $(sized ROXR)
BCHG BCLR BSET BTST
Bcc BSR DBcc Scc JMP JSR RTS RTR RTE
CHK DIVS DIVU MULS MULU
EXG EXT.w EXT.l SWAP LEA PEA LINK UNLINK
$(sized MOVE) MOVE.q MOVEA.w MOVEA.l MOVEM.w MOVEM.l MOVEP.w MOVEP.l
MOVEfromSR MOVEtoSR MOVEtoCCR MOVEfromUSP MOVEtoUSP
NOP RESET TAS TRAP TRAPV
"

for name in $NAMES; do
    out="$DEST/$name.json"
    if [ -s "$out" ]; then
        continue
    fi
    echo "fetching $name"
    if curl -fsSL --retry 3 "$BASE/$name.json.gz" -o "$out.gz"; then
        gunzip -f "$out.gz"
    else
        echo "  warning: $name.json.gz not found upstream" >&2
        rm -f "$out.gz"
    fi
done

for name in $NAMES; do
    out="$MAME_DEST/$name.json.bin"
    if [ -s "$out" ]; then
        continue
    fi
    echo "fetching $name (MAME)"
    if ! curl -fsSL --retry 3 "$MAME_BASE/$name.json.bin" -o "$out"; then
        echo "  warning: $name.json.bin not found upstream" >&2
        rm -f "$out"
    fi
done

echo "Tom Harte vectors in $DEST: $(ls "$DEST"/*.json | wc -l) files"
echo "MAME vectors in $MAME_DEST: $(ls "$MAME_DEST"/*.json.bin | wc -l) files"
