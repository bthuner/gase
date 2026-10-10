"""Regenerate the test fixtures of gase-zip with Python's zlib and zipfile.

Run from anywhere with `python3 -I crates/zip/tests/fixtures/make_fixtures.py`.
The output is deterministic (fixed seeds and timestamps), so re-running it
should leave the committed files unchanged.

Fixtures are produced by an independent, widely used implementation (zlib),
which is the point: gase-zip must read what real tools write.
"""

import io
import os
import random
import zipfile
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
DATE = (2024, 1, 2, 3, 4, 6)


def corpus():
    """8 KiB mixing text, structured binary and noise."""
    rng = random.Random(1951)
    words = ("the sega mega drive genesis cartridge rom zip deflate huffman "
             "code length distance literal block window").split()
    text = " ".join(rng.choice(words) for _ in range(600)).encode()[:3072]
    binary = bytearray()
    for i in range(1536):  # 68000-ish big-endian words
        binary += (((i * 0x0102) ^ (i >> 3)) & 0xFFFF).to_bytes(2, "big")
    noise = bytes(rng.randrange(256) for _ in range(2048))
    return text + bytes(binary) + noise


def long_distance():
    """Two copies of 1 KiB of noise 31 KiB apart: needs the whole window."""
    rng = random.Random(32768)
    noise = bytes(rng.randrange(256) for _ in range(1024))
    return noise + bytes(30 * 1024) + noise


def rom(size):
    """A fake ROM whose bytes are easy to recompute in Rust."""
    return bytes(((i * 7) ^ (i >> 8)) & 0xFF for i in range(size))


def deflate(data, level, strategy=zlib.Z_DEFAULT_STRATEGY):
    c = zlib.compressobj(level, zlib.DEFLATED, -15, 9, strategy)
    return c.compress(data) + c.flush()


def write_deflate_fixtures():
    lines = ["# name size crc32 (of the decompressed data)"]
    cases = [
        ("corpus", corpus()),
        ("long_distance", long_distance()),
        ("empty", b""),
    ]
    for name, data in cases:
        variants = [(f"level{level}", deflate(data, level)) for level in (0, 1, 6, 9)]
        variants += [
            ("fixed", deflate(data, 9, zlib.Z_FIXED)),
            ("huffman_only", deflate(data, 9, zlib.Z_HUFFMAN_ONLY)),
            ("rle", deflate(data, 9, zlib.Z_RLE)),
        ]
        if name == "long_distance":
            # Stored and literal-only versions would be 32 KiB and test
            # nothing the corpus does not already.
            variants = [v for v in variants if v[0] not in ("level0", "huffman_only")]
        for variant, packed in variants:
            filename = f"{name}.{variant}.deflate"
            with open(os.path.join(HERE, filename), "wb") as f:
                f.write(packed)
            lines.append(f"{filename} {len(data)} {zlib.crc32(data):08x}")
    with open(os.path.join(HERE, "deflate.txt"), "w") as f:
        f.write("\n".join(lines) + "\n")


def add(zf, name, data, method=zipfile.ZIP_DEFLATED):
    info = zipfile.ZipInfo(name, DATE)
    info.compress_type = method
    zf.writestr(info, data)


class Unseekable(io.RawIOBase):
    """A stream zipfile cannot seek in, so it writes data descriptors."""

    def __init__(self):
        self.buf = bytearray()

    def writable(self):
        return True

    def write(self, b):
        self.buf += b
        return len(b)


def write_zip_fixtures():
    # Several files: the ROM must be found among them.
    with zipfile.ZipFile(os.path.join(HERE, "several.zip"), "w") as zf:
        zf.comment = b"gase-zip test archive"
        add(zf, "readme.txt", b"Some text that is not a ROM. " * 20)
        add(zf, "Game (USA)/", b"", zipfile.ZIP_STORED)
        add(zf, "Game (USA)/Game (USA).MD", rom(3000))
        add(zf, "Game (USA)/small.bin", rom(600), zipfile.ZIP_STORED)
        add(zf, "__MACOSX/Game (USA)/._Game (USA).MD", rom(4000))
        add(zf, "Game (USA)/._hidden.bin", rom(4000))
        add(zf, "Game (USA)/screenshot.png", rom(5000))
    # No ROM at all.
    with zipfile.ZipFile(os.path.join(HERE, "no_rom.zip"), "w") as zf:
        add(zf, "readme.txt", b"Nothing to see here.\n")
        add(zf, "notes.doc", rom(100), zipfile.ZIP_STORED)
    # No files at all.
    with zipfile.ZipFile(os.path.join(HERE, "empty.zip"), "w"):
        pass
    # Written to an unseekable stream: sizes follow the data
    # (general-purpose flag bit 3), the central directory has them too.
    stream = Unseekable()
    with zipfile.ZipFile(stream, "w") as zf:
        info = zipfile.ZipInfo("game.gen", DATE)
        info.compress_type = zipfile.ZIP_DEFLATED
        with zf.open(info, "w") as f:
            f.write(rom(5000))
    with open(os.path.join(HERE, "streamed.zip"), "wb") as f:
        f.write(stream.buf)


if __name__ == "__main__":
    write_deflate_fixtures()
    write_zip_fixtures()
