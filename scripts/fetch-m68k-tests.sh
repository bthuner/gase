#!/usr/bin/env sh
# Download Tom Harte's 68000 single-instruction test vectors
# (https://github.com/TomHarte/ProcessorTests, 680x0/68000/v1) and unpack them
# into target/test-vectors/m68000/, where `crates/m68k/tests/tomharte.rs`
# looks for them:
#
#     scripts/fetch-m68k-tests.sh
#     cargo test -p gase-m68k --profile fast-test -- --ignored
#
# The vectors are ~1 M tests (~70 MB compressed, ~1.5 GB unpacked); they are
# never committed (target/ is gitignored). Re-running skips existing files.
set -eu

BASE="https://raw.githubusercontent.com/TomHarte/ProcessorTests/main/680x0/68000/v1"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${GASE_M68K_TESTS:-$ROOT/target/test-vectors/m68000}"
mkdir -p "$DEST"

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

echo "test vectors in $DEST: $(ls "$DEST"/*.json | wc -l) files"
