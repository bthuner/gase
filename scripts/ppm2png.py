#!/usr/bin/env python3
"""Convert binary PPM files (as dumped by the test-ROM suite) to PNG, stdlib only."""
import struct, sys, zlib

def convert(path):
    data = open(path, "rb").read()
    header, _, rest = data.partition(b"\n")
    _, w, h, _ = header.split()
    w, h = int(w), int(h)
    raw = b"".join(b"\0" + rest[y * w * 3:(y + 1) * w * 3] for y in range(h))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
    open(path[:-4] + ".png", "wb").write(png)

for p in sys.argv[1:]:
    convert(p)
