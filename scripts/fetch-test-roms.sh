#!/bin/sh
# Download the freely licensed test ROMs used by `crates/core/tests/test_roms.rs`
# into target/test-roms/ (never committed). Each file is checked against a
# SHA-256 so the regression hashes stay meaningful.
#
# Licenses:
#   240p Test Suite 1.23   GPL-2.0-or-later (Artemio Urbina)
#   genmd-imgrom pattern   MIT (Sebastian Tomczak)
#   Airstriker             freeware, distributed by Gym Retro for testing
#   Right 2 Repair         Global Game Jam 2020 entry (non-commercial)
#   The Spiral             GPL-3.0 (Resistance)
set -eu

DEST="${GASE_TEST_ROMS:-$(dirname "$0")/../target/test-roms}"
RAW=https://raw.githubusercontent.com

fetch() { # dir file url sha256
    mkdir -p "$DEST/$1"
    out="$DEST/$1/$2"
    if [ ! -f "$out" ]; then
        echo "fetching $1/$2"
        curl -fsSL -o "$out.part" "$3"
        mv "$out.part" "$out"
    fi
    echo "$4  $out" | sha256sum -c --quiet - || { echo "checksum mismatch: $out" >&2; exit 1; }
}

fetch 240p-test-suite 240pSuite-1.23.bin \
    "$RAW/Apaczer/miyoo_tools/master/test_ROMS/MD/240pTestSuite-MD/240pSuite-1.23.bin" \
    e6cdc5f7efe91378a77026ce40565265bb26a1f5748cca8896660d13cf9755db
fetch genmd-imgrom-testpattern testpattern.bin \
    "$RAW/little-scale/genmd-imgrom/main/examples/testpattern.bin" \
    c3f0d164af79c5056535b1094b5cbeb8c7bcec1d5f0fa2d662bcd328548048e0
fetch airstriker Airstriker.md \
    "$RAW/openai/retro/master/retro/data/stable/Airstriker-Genesis/rom.md" \
    fed6d65a38576628db9061a7c4173ea50fbffebabad256e0654cbcd32a8c651f
fetch right2repair-ggj2020 rom.bin \
    "$RAW/theshaneobrien/Global_Game_Jam_2020/master/out/rom.bin" \
    6a7f1a12da753bfcdc0e8fc018ba41a77d6b489a09ea13bf5a88420f2f19558a
fetch resistance-the-spiral rom.bin \
    "$RAW/ResistanceVault/demo-The-Spiral/main/src/out/rom.bin" \
    a9a153f7a53e3d9294e595fa7d5f4fa5e5c689e31fb7114510f79c066450371c
echo "test ROMs ready in $DEST"
