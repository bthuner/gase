#!/usr/bin/env python3
"""Make small, lossless PNGs for the project site. Standard library only.

    png.py IN [OUT]

IN is a PNG (8-bit RGB or RGBA, as gase writes them) or a PAM file (as
scripts/docs-shots writes them). OUT defaults to IN with a .png suffix.
Emulator pictures have few colours, so they are stored as palette images
(with a transparency table when needed), with the deflate level and the row
filters that give the smallest file. Pixels are never changed.
"""
import struct
import sys
import zlib


def read_png(data):
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos, idat, header = 8, b"", None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos:pos + 4])
        kind = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
    width, height, depth, colour, _, _, interlace = header
    assert depth == 8 and colour in (2, 6) and not interlace, "unsupported PNG"
    channels = 3 if colour == 2 else 4
    raw = zlib.decompress(idat)
    stride = width * channels
    rows, previous = [], bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        kind, line = raw[start], bytearray(raw[start + 1:start + 1 + stride])
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = previous[i]
            c = previous[i - channels] if i >= channels else 0
            if kind == 1:
                line[i] = (line[i] + a) & 255
            elif kind == 2:
                line[i] = (line[i] + b) & 255
            elif kind == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif kind == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 255
        rows.append(bytes(line))
        previous = line
    pixels = []
    for line in rows:
        for x in range(width):
            px = line[x * channels:(x + 1) * channels]
            pixels.append(tuple(px) + ((255,) if channels == 3 else ()))
    return width, height, pixels


def read_pam(data):
    header, _, body = data.partition(b"ENDHDR\n")
    fields = dict(line.split(b" ", 1) for line in header.splitlines()[1:] if b" " in line)
    width, height = int(fields[b"WIDTH"]), int(fields[b"HEIGHT"])
    assert int(fields[b"DEPTH"]) == 4
    pixels = [tuple(body[i:i + 4]) for i in range(0, width * height * 4, 4)]
    return width, height, pixels


def chunk(kind, body):
    return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))


def encode(width, height, pixels):
    # Every fully transparent pixel is the same colour.
    pixels = [p if p[3] else (0, 0, 0, 0) for p in pixels]
    colours = sorted(set(pixels), key=lambda p: (p[3], p))  # transparent first
    if len(colours) <= 256:
        index = {c: i for i, c in enumerate(colours)}
        depth = 1 if len(colours) <= 2 else 2 if len(colours) <= 4 else 4 if len(colours) <= 16 else 8
        per_byte = 8 // depth
        rows = []
        for y in range(height):
            line = bytearray()
            for x0 in range(0, width, per_byte):
                byte = 0
                for k in range(per_byte):
                    value = index[pixels[y * width + x0 + k]] if x0 + k < width else 0
                    byte |= value << (8 - depth * (k + 1))
                line.append(byte)
            rows.append(bytes(line))
        ihdr = struct.pack(">IIBBBBB", width, height, depth, 3, 0, 0, 0)
        extra = chunk(b"PLTE", b"".join(bytes(c[:3]) for c in colours))
        alphas = [c[3] for c in colours]
        if any(a < 255 for a in alphas):
            last = max(i for i, a in enumerate(alphas) if a < 255)
            extra += chunk(b"tRNS", bytes(alphas[:last + 1]))
        bpp = 1
    else:
        alpha = any(p[3] < 255 for p in pixels)
        channels = 4 if alpha else 3
        rows = [b"".join(bytes(p[:channels]) for p in pixels[y * width:(y + 1) * width]) for y in range(height)]
        ihdr = struct.pack(">IIBBBBB", width, height, 8, 6 if alpha else 2, 0, 0, 0)
        extra = b""
        bpp = channels
    best = None
    for strategy in ("none", "sub", "up", "adaptive"):
        raw = bytearray()
        previous = bytes(len(rows[0]))
        for line in rows:
            candidates = {
                0: line,
                1: bytes((line[i] - (line[i - bpp] if i >= bpp else 0)) & 255 for i in range(len(line))),
                2: bytes((line[i] - previous[i]) & 255 for i in range(len(line))),
            }
            if strategy == "none":
                kind = 0
            elif strategy == "sub":
                kind = 1
            elif strategy == "up":
                kind = 2
            else:
                kind = min(candidates, key=lambda k: sum(b if b < 128 else 256 - b for b in candidates[k]))
            raw.append(kind)
            raw += candidates[kind]
            previous = line
        packed = zlib.compress(bytes(raw), 9)
        if best is None or len(packed) < len(best):
            best = packed
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + extra + chunk(b"IDAT", best) + chunk(b"IEND", b"")


def main():
    source = sys.argv[1]
    target = sys.argv[2] if len(sys.argv) > 2 else source.rsplit(".", 1)[0] + ".png"
    data = open(source, "rb").read()
    width, height, pixels = read_pam(data) if data.startswith(b"P7") else read_png(data)
    out = encode(width, height, pixels)
    open(target, "wb").write(out)
    print(f"{target}: {width}x{height}, {len(out)} bytes")


if __name__ == "__main__":
    main()
