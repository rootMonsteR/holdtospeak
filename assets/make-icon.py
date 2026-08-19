"""Procedural HoldToSpeak app icon.

Brief: futuristic, and instantly readable at 16 px in a system tray.
Everything decorative (brackets, rim) is allowed to disappear at small sizes; the
waveform silhouette is what must survive, so it carries the contrast.
"""
import math, struct, zlib, os

OUT = os.path.dirname(os.path.abspath(__file__))
SS = 4  # supersample factor


def clamp(v, lo=0.0, hi=1.0):
    return lo if v < lo else hi if v > hi else v


def blend(dst, i, r, g, b, a):
    if a <= 0:
        return
    d = dst[i]
    ia = 1.0 - a
    d[0] = r * a + d[0] * ia
    d[1] = g * a + d[1] * ia
    d[2] = b * a + d[2] * ia
    d[3] = a + d[3] * ia


def rr_cov(x, y, n, inset, rad):
    lo, hi = inset, n - inset
    cx = min(max(x, lo + rad), hi - rad)
    cy = min(max(y, lo + rad), hi - rad)
    return clamp((rad - math.hypot(x - cx, y - cy)) + 0.5)


def bar_cov(x, y, x0, x1, y0, y1, rad):
    cx = min(max(x, x0 + rad), x1 - rad)
    cy = min(max(y, y0 + rad), y1 - rad)
    return clamp((rad - math.hypot(x - cx, y - cy)) + 0.5)


def box_blur(mask, n, r):
    out = list(mask)
    tmp = [0.0] * (n * n)
    for _ in range(2):
        for yy in range(n):
            acc = 0.0
            row = yy * n
            for xx in range(-r, n):
                if xx + r < n:
                    acc += out[row + xx + r]
                if xx - r - 1 >= 0:
                    acc -= out[row + xx - r - 1]
                if xx >= 0:
                    tmp[row + xx] = acc / (2 * r + 1)
        for xx in range(n):
            acc = 0.0
            for yy in range(-r, n):
                if yy + r < n:
                    acc += tmp[(yy + r) * n + xx]
                if yy - r - 1 >= 0:
                    acc -= tmp[(yy - r - 1) * n + xx]
                if yy >= 0:
                    out[yy * n + xx] = acc / (2 * r + 1)
    return out


# Asymmetric on purpose: a symmetric equaliser reads as a generic media icon.
#
# Small sizes get FEWER, FATTER bars. Five bars at 16 px works out under 1.5 px each, which
# antialiases into an unreadable smear — so the tray variant is a simplified drawing of the same
# idea rather than a shrunk copy of the big one. Standard icon practice, and the only way the
# silhouette survives.
BARS_LARGE = [0.34, 0.62, 1.00, 0.72, 0.44]
BARS_SMALL = [0.52, 1.00, 0.68]


def render(size):
    n = size * SS
    small = size <= 24
    bars = BARS_SMALL if small else BARS_LARGE
    cv = [[0.0, 0.0, 0.0, 0.0] for _ in range(n * n)]
    inset = n * 0.045
    rad = n * 0.235

    # tile: deep navy gradient
    for y in range(n):
        t = y / (n - 1)
        r = 0.075 + (0.027 - 0.075) * t
        g = 0.098 + (0.035 - 0.098) * t
        b = 0.180 + (0.078 - 0.180) * t
        for x in range(n):
            c = rr_cov(x + 0.5, y + 0.5, n, inset, rad)
            if c > 0:
                blend(cv, y * n + x, r, g, b, c)

    # bar geometry
    field_w = n * (0.62 if small else 0.64)
    field_h = n * (0.56 if small else 0.50)
    x_start = (n - field_w) / 2.0
    mid = n * 0.53
    gap = field_w * (0.10 if small else 0.075)
    bw = (field_w - gap * (len(bars) - 1)) / len(bars)
    brad = bw * 0.42

    mask = [0.0] * (n * n)
    geo = []
    for i, h in enumerate(bars):
        bh = field_h * h
        x0 = x_start + i * (bw + gap)
        geo.append((x0, x0 + bw, mid - bh / 2.0, mid + bh / 2.0))
    for (x0, x1, y0, y1) in geo:
        for y in range(max(0, int(y0) - 2), min(n, int(y1) + 3)):
            for x in range(max(0, int(x0) - 2), min(n, int(x1) + 3)):
                c = bar_cov(x + 0.5, y + 0.5, x0, x1, y0, y1, brad)
                if c > mask[y * n + x]:
                    mask[y * n + x] = c

    # glow under the bars
    glow = box_blur(mask, n, max(1, int(n * (0.030 if small else 0.055))))
    for y in range(n):
        for x in range(n):
            g = glow[y * n + x]
            if g <= 0.004:
                continue
            tile = rr_cov(x + 0.5, y + 0.5, n, inset, rad)
            if tile <= 0:
                continue
            strength = 0.28 if small else 0.55
            blend(cv, y * n + x, 0.16, 0.62, 1.0, clamp(g * 1.25) * tile * strength)

    # bars: electric blue base -> cyan tip
    top_y = min(g[2] for g in geo)
    bot_y = max(g[3] for g in geo)
    for y in range(n):
        t = clamp((bot_y - y) / (bot_y - top_y))
        lift = 0.16 if small else 0.0
        r = clamp(0.12 + lift + (0.62 - 0.12) * (t ** 0.8))
        gg = clamp(0.53 + lift + (0.94 - 0.53) * (t ** 0.8))
        for x in range(n):
            m = mask[y * n + x]
            if m > 0:
                blend(cv, y * n + x, r, gg, 1.0, m)

    # HUD corner brackets (decoration, allowed to vanish when small)
    if size >= 48:
        bl = int(n * 0.17)
        bt = max(1, int(n * 0.026))
        off = n * 0.125
        for (ox, oy, dx, dy) in ((off, off, 1, 1), (n - off, n - off, -1, -1)):
            for k in range(bl):
                for w in range(bt):
                    for (px_, py_) in ((int(ox + dx * k), int(oy + dy * w)),
                                       (int(ox + dx * w), int(oy + dy * k))):
                        if 0 <= px_ < n and 0 <= py_ < n:
                            blend(cv, py_ * n + px_, 0.35, 0.80, 1.0, 0.55)

    # thin outer rim
    for y in range(n):
        for x in range(n):
            ring = clamp(rr_cov(x + 0.5, y + 0.5, n, inset, rad)
                         - rr_cov(x + 0.5, y + 0.5, n, inset + n * 0.018, rad))
            if ring > 0.02:
                blend(cv, y * n + x, 0.30, 0.72, 1.0, ring * (0.22 if small else 0.40))

    # downsample (premultiplied average, then un-premultiply)
    out = bytearray(size * size * 4)
    k = SS * SS
    for y in range(size):
        for x in range(size):
            r = g = b = a = 0.0
            for sy in range(SS):
                for sx in range(SS):
                    p = cv[(y * SS + sy) * n + (x * SS + sx)]
                    r += p[0] * p[3]
                    g += p[1] * p[3]
                    b += p[2] * p[3]
                    a += p[3]
            a /= k
            if a > 0:
                r = r / k / a
                g = g / k / a
                b = b / k / a
            i = (y * size + x) * 4
            out[i] = int(clamp(r) * 255 + 0.5)
            out[i + 1] = int(clamp(g) * 255 + 0.5)
            out[i + 2] = int(clamp(b) * 255 + 0.5)
            out[i + 3] = int(clamp(a) * 255 + 0.5)
    return bytes(out)


def chunk(kind, data):
    return (struct.pack(">I", len(data)) + kind + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))


def png_bytes(size, rgba):
    raw = b"".join(b"\x00" + rgba[y * size * 4:(y + 1) * size * 4] for y in range(size))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


def bmp_entry(size, rgba):
    hdr = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0,
                      size * size * 4, 0, 0, 0, 0)
    rows = []
    for y in range(size - 1, -1, -1):
        row = bytearray()
        for x in range(size):
            i = (y * size + x) * 4
            row += bytes((rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]))
        rows.append(bytes(row))
    stride = ((size + 31) // 32) * 4
    return hdr + b"".join(rows) + b"\x00" * (stride * size)


SIZES = [16, 20, 24, 32, 48, 64, 128, 256]


def main():
    entries = []
    for s in SIZES:
        rgba = render(s)
        # PNG compression inside an ICO is only universally understood for 256.
        entries.append((s, png_bytes(s, rgba) if s == 256 else bmp_entry(s, rgba)))
        if s in (16, 32, 48, 256):
            open(os.path.join(OUT, "preview_%d.png" % s), "wb").write(png_bytes(s, rgba))

    head = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    dirblob = b""
    body = b""
    for s, data in entries:
        w = 0 if s >= 256 else s
        dirblob += struct.pack("<BBBBHHII", w, w, 0, 0, 1, 32, len(data), offset)
        body += data
        offset += len(data)
    path = os.path.join(OUT, "HoldToSpeak.ico")
    open(path, "wb").write(head + dirblob + body)
    print("wrote", path, os.path.getsize(path), "bytes,", len(entries), "sizes")


main()
