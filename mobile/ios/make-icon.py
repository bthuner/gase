#!/usr/bin/env python3
"""Rasterise mobile/icon.svg into the iOS app icon. Standard library only.

    mobile/ios/make-icon.py

iOS wants one opaque 1024 x 1024 PNG (Xcode scales it for every device).
The icon is made of axis-aligned rectangles only, so instead of an SVG
renderer this script reads the <rect> elements and computes, for every
pixel, how much of it each rectangle covers (smooth edges, exact colours).
Run it again after changing icon.svg.
"""
import os
import struct
import xml.etree.ElementTree as ET
import zlib

SIZE = 1024
HERE = os.path.dirname(os.path.abspath(__file__))
SVG = os.path.join(HERE, "..", "icon.svg")
OUT = os.path.join(HERE, "Resources", "Assets.xcassets", "AppIcon.appiconset", "icon-1024.png")


def coverage(start, end, scale):
    """How much of each pixel along one axis lies between start and end."""
    a, b = start * scale, end * scale
    return [max(0.0, min(b, i + 1) - max(a, i)) for i in range(SIZE)]


def main():
    root = ET.parse(SVG).getroot()
    view = [float(v) for v in root.get("viewBox").split()]
    scale = SIZE / view[2]
    pixels = [[(0.0, 0.0, 0.0)] * SIZE for _ in range(SIZE)]
    for rect in root.iter("{http://www.w3.org/2000/svg}rect"):
        x, y = float(rect.get("x", 0)), float(rect.get("y", 0))
        w, h = float(rect.get("width")), float(rect.get("height"))
        colour = rect.get("fill").lstrip("#")
        rgb = tuple(int(colour[i:i + 2], 16) for i in (0, 2, 4))
        cx, cy = coverage(x, x + w, scale), coverage(y, y + h, scale)
        cols = [i for i in range(SIZE) if cx[i] > 0]
        for j in range(SIZE):
            if cy[j] == 0:
                continue
            row = pixels[j]
            for i in cols:
                a = cx[i] * cy[j]
                old = row[i]
                row[i] = tuple(o + (n - o) * a for o, n in zip(old, rgb))
    raw = b"".join(
        b"\0" + bytes(int(round(c)) for p in row for c in p) for row in pixels
    )

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    png = (b"\x89PNG\r\n\x1a\n"
           + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 2, 0, 0, 0))
           + chunk(b"IDAT", zlib.compress(raw, 9))
           + chunk(b"IEND", b""))
    with open(OUT, "wb") as f:
        f.write(png)
    print(f"Wrote {OUT} ({len(png)} bytes)")


if __name__ == "__main__":
    main()
