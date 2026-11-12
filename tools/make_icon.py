"""Render the WinFX icon (same design as the tray icon in system.rs: a white mic
capsule on a teal disc) into a multi-size .ico. No dependencies.

    python tools/make_icon.py crates/app/res/winfx.ico
"""
import struct
import sys

SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
SS = 4  # supersampling per axis


def coverage(fx, fy):
    """(alpha, white) for a point in the 32-unit design space centred on 0."""
    disc = 15.5 - (fx * fx + fy * fy) ** 0.5
    if disc <= 0:
        return 0.0, False
    cy = min(max(fy, -6.0), 0.0)
    cap = 4.5 - (fx * fx + (fy - cy) ** 2) ** 0.5
    stand = (abs(fx) <= 1.0 and 5.0 <= fy <= 9.0) or (abs(fx) <= 5.0 and 9.0 <= fy <= 10.5)
    return 1.0, cap > 0 or stand


def render(n):
    px = []
    for y in range(n):
        row = []
        for x in range(n):
            a = r = g = b = 0.0
            for sy in range(SS):
                for sx in range(SS):
                    fx = ((x + (sx + 0.5) / SS) / n) * 32.0 - 16.0
                    fy = ((y + (sy + 0.5) / SS) / n) * 32.0 - 16.0
                    cov, white = coverage(fx, fy)
                    if cov:
                        cr, cg, cb = (245, 250, 250) if white else (20, 150, 140)
                        a += 1
                        r += cr
                        g += cg
                        b += cb
            k = SS * SS
            if a:
                row.append((round(b / a), round(g / a), round(r / a), round(255 * a / k)))
            else:
                row.append((0, 0, 0, 0))
        px.append(row)
    return px


def bmp_entry(n):
    px = render(n)
    header = struct.pack('<IiiHHIIiiII', 40, n, n * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    data = bytearray()
    for row in reversed(px):  # bottom-up
        for bgra in row:
            data += bytes(bgra)
    mask_row = ((n + 31) // 32) * 4
    data += bytes(mask_row * n)  # AND mask: all opaque; alpha does the work
    return header + bytes(data)


def main(out):
    entries = [bmp_entry(n) for n in SIZES]
    ico = struct.pack('<HHH', 0, 1, len(SIZES))
    offset = 6 + 16 * len(SIZES)
    dirs = b''
    for n, e in zip(SIZES, entries):
        dirs += struct.pack('<BBBBHHII', n % 256, n % 256, 0, 0, 1, 32, len(e), offset)
        offset += len(e)
    with open(out, 'wb') as f:
        f.write(ico + dirs + b''.join(entries))
    print(f'wrote {out}')


if __name__ == '__main__':
    main(sys.argv[1])
