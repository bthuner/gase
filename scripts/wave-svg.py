#!/usr/bin/env python3
"""Draw a stretch of a WAV file (as recorded by `gase --wav`) as an SVG.

    wave-svg.py IN.wav OUT.svg START_SECONDS DURATION_SECONDS [--envelope]

Plots the mono mix (L+R)/2. By default every sample is a point of one line
(for a zoomed-in view of a few milliseconds); with --envelope each column
shows the minimum and maximum of its samples (for seconds of audio). The
SVG is plain black on transparent, meant to be used as a CSS mask so the
page can colour it. Standard library only.
"""
import struct
import sys
import wave

WIDTH, HEIGHT = 1200, 240


def main():
    path, out, start, duration = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])
    envelope = "--envelope" in sys.argv[5:]
    w = wave.open(path)
    rate, channels = w.getframerate(), w.getnchannels()
    w.setpos(int(start * rate))
    count = int(duration * rate)
    raw = w.readframes(count)
    values = struct.unpack("<%dh" % (len(raw) // 2), raw)
    mono = [sum(values[i:i + channels]) / channels for i in range(0, len(values), channels)]
    peak = max(1, max(abs(v) for v in mono))
    mid = HEIGHT / 2
    scale = (HEIGHT / 2 - 4) / peak

    if envelope:
        columns = WIDTH // 3
        parts = []
        for c in range(columns):
            chunk = mono[c * len(mono) // columns:(c + 1) * len(mono) // columns] or [0]
            top, bottom = mid - max(chunk) * scale, mid - min(chunk) * scale
            x = c * 3 + 1
            parts.append(f"M{x} {top:.1f}V{max(bottom, top + 1):.1f}")
        body = f'<path d="{"".join(parts)}" stroke="#000" stroke-width="2" fill="none"/>'
    else:
        step = WIDTH / max(1, len(mono) - 1)
        points = " ".join(f"{i * step:.1f},{mid - v * scale:.1f}" for i, v in enumerate(mono))
        body = (f'<polyline points="{points}" stroke="#000" stroke-width="2.5" '
                'fill="none" stroke-linejoin="round" vector-effect="non-scaling-stroke"/>')
    svg = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {HEIGHT}" '
           f'preserveAspectRatio="none">{body}</svg>\n')
    open(out, "w").write(svg)
    print(f"{out}: {len(mono)} samples from {start}s, {len(svg)} bytes")


if __name__ == "__main__":
    main()
