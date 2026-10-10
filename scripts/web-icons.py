#!/usr/bin/env python3
"""Draw the web app's icons (web/icons/*.png and icon.svg).

The mark is the project site's: three bars in console red, gold and bone
on warm black. No logo of anyone else's. Standard library only: the PNGs
are written by hand (signature, IHDR, one zlib-compressed IDAT, IEND).

    python3 scripts/web-icons.py
"""

import os
import struct
import zlib

BG = (0x0D, 0x0C, 0x0B)
BARS = [  # (x, y, w, h) on a 32-unit grid, colour
    ((6, 6, 20, 4), (0xF0, 0x47, 0x3B)),
    ((6, 14, 20, 4), (0xDC, 0xAE, 0x45)),
    ((6, 22, 12, 4), (0xED, 0xE6, 0xD9)),
]
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "web", "icons")


def png(path, size, content=1.0):
    """A size×size icon; the 32-unit mark is scaled to `content` of it,
    centred (maskable icons keep it inside the central safe circle)."""
    unit = size * content / 32
    offset = size * (1 - content) / 2
    rows = []
    for y in range(size):
        row = bytearray([0])  # filter type: none
        for x in range(size):
            colour = BG
            for (bx, by, bw, bh), c in BARS:
                if (offset + bx * unit <= x + 0.5 < offset + (bx + bw) * unit
                        and offset + by * unit <= y + 0.5 < offset + (by + bh) * unit):
                    colour = c
            row += bytes(colour)
        rows.append(bytes(row))

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    data = b"\x89PNG\r\n\x1a\n"
    data += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
    data += chunk(b"IEND", b"")
    with open(os.path.join(OUT, path), "wb") as f:
        f.write(data)


def svg():
    hexc = lambda c: "#%02x%02x%02x" % c
    rects = "".join(
        f'<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{hexc(c)}"/>'
        for (x, y, w, h), c in BARS
    )
    with open(os.path.join(OUT, "icon.svg"), "w") as f:
        f.write('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">'
                f'<rect width="32" height="32" fill="{hexc(BG)}"/>{rects}</svg>\n')


os.makedirs(OUT, exist_ok=True)
png("icon-192.png", 192)
png("icon-512.png", 512)
# iOS rounds the corners itself and wants no transparency.
png("apple-touch-icon.png", 180)
# Maskable: Android may cut any shape out of the middle 80 %; the mark's
# corners must stay inside that circle.
png("maskable-512.png", 512, content=0.62)
svg()
