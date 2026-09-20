"""生成 VideoFlow 应用图标（纯标准库，无第三方依赖）。

用法：python scripts/gen_icon.py [输出尺寸]
输出：scripts/_icon_source.png —— 供 `npx tauri icon` 生成各平台图标集。
"""

import math
import struct
import sys
import zlib

SIZE = int(sys.argv[1]) if len(sys.argv) > 1 else 1024
SS = 4  # 超采样倍率，用于抗锯齿

# 品牌色：深蓝 -> 亮蓝 对角渐变
C_TOP = (0x6F, 0xC4, 0xFF)
C_BOTTOM = (0x0B, 0x5E, 0x9F)


def in_rounded_rect(px, py, w, h, r):
    cx, cy = w / 2.0, h / 2.0
    dx = abs(px - cx) - (w / 2.0 - r)
    dy = abs(py - cy) - (h / 2.0 - r)
    dx = max(dx, 0.0)
    dy = max(dy, 0.0)
    return math.hypot(dx, dy) <= r


def in_triangle(px, py, ax, ay, bx, by, cx, cy):
    d1 = (px - bx) * (ay - by) - (ax - bx) * (py - by)
    d2 = (px - cx) * (by - cy) - (bx - cx) * (py - cy)
    d3 = (px - ax) * (cy - ay) - (cx - ax) * (py - ay)
    has_neg = d1 < 0 or d2 < 0 or d3 < 0
    has_pos = d1 > 0 or d2 > 0 or d3 > 0
    return not (has_neg and has_pos)


def sample(px, py):
    """返回该采样点的 (r, g, b, a)。"""
    # 外圈留白，让图标在任务栏里不顶边
    pad = SIZE * 0.055
    inner = SIZE - pad * 2
    radius = inner * 0.235

    if not in_rounded_rect(px - pad, py - pad, inner, inner, radius):
        return (0, 0, 0, 0)

    # 对角渐变
    t = (px / SIZE) * 0.35 + (py / SIZE) * 0.65
    t = min(max(t, 0.0), 1.0)
    r = C_TOP[0] + (C_BOTTOM[0] - C_TOP[0]) * t
    g = C_TOP[1] + (C_BOTTOM[1] - C_TOP[1]) * t
    b = C_TOP[2] + (C_BOTTOM[2] - C_TOP[2]) * t

    # 左上角柔光
    glow_x, glow_y = SIZE * 0.26, SIZE * 0.20
    dist = math.hypot(px - glow_x, py - glow_y)
    glow = max(0.0, 1.0 - dist / (SIZE * 0.62)) ** 2 * 0.30
    r += (255 - r) * glow
    g += (255 - g) * glow
    b += (255 - b) * glow

    # 中央播放三角
    s = inner * 0.34
    ccx, ccy = SIZE / 2.0, SIZE / 2.0
    ax, ay = ccx - s * 0.52, ccy - s * 0.62
    bx, by = ccx - s * 0.52, ccy + s * 0.62
    tx, ty = ccx + s * 0.72, ccy

    if in_triangle(px, py, ax, ay, bx, by, tx, ty):
        return (255, 255, 255, 255)

    return (int(r), int(g), int(b), 255)


def build_png():
    rows = []
    step = 1.0 / SS
    for y in range(SIZE):
        row = bytearray()
        row.append(0)  # filter type 0
        for x in range(SIZE):
            acc_r = acc_g = acc_b = acc_a = 0
            for sy in range(SS):
                for sx in range(SS):
                    px = x + (sx + 0.5) * step
                    py = y + (sy + 0.5) * step
                    sr, sg, sb, sa = sample(px, py)
                    acc_r += sr * sa
                    acc_g += sg * sa
                    acc_b += sb * sa
                    acc_a += sa
            n = SS * SS
            if acc_a == 0:
                row.extend((0, 0, 0, 0))
            else:
                row.extend(
                    (
                        min(255, acc_r // acc_a),
                        min(255, acc_g // acc_a),
                        min(255, acc_b // acc_a),
                        min(255, acc_a // n),
                    )
                )
        rows.append(bytes(row))
    return b"".join(rows)


def chunk(tag, data):
    return (
        struct.pack(">I", len(data))
        + tag
        + data
        + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    )


def main():
    raw = build_png()
    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    out = "scripts/_icon_source.png"
    with open(out, "wb") as f:
        f.write(png)
    print(f"wrote {out} ({SIZE}x{SIZE}, {len(png)} bytes)")


if __name__ == "__main__":
    main()