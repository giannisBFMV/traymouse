"""Draws assets/traymouse.ico (the website logo: purple gradient tile + dark mouse). Needs Pillow.

python tools/make_icon.py
"""
import pathlib

from PIL import Image, ImageDraw

OUT = pathlib.Path(__file__).resolve().parent.parent / "assets"
STOPS = [(0.0, (0x7C, 0x6C, 0xFF)), (0.55, (0x96, 0x8C, 0xFF)), (1.0, (0xD9, 0xD4, 0xFF))]
INK = (0x13, 0x13, 0x16, 255)
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]


def grad(t):
    for (a, ca), (b, cb) in zip(STOPS, STOPS[1:]):
        if t <= b:
            f = (t - a) / (b - a)
            return tuple(round(x + (y - x) * f) for x, y in zip(ca, cb))
    return STOPS[-1][1]


def draw(n=1024):
    """Big master drawing; downscaling it gives smooth edges at every size."""
    m = round(n * 0.03)  # small margin so it sits like other Windows icons
    tile = n - 2 * m
    fill = Image.new("RGBA", (n, n))
    px = fill.load()
    for x in range(n):
        c = grad(x / (n - 1)) + (255,)
        for y in range(n):
            px[x, y] = c
    mask = Image.new("L", (n, n), 0)
    ImageDraw.Draw(mask).rounded_rectangle((m, m, m + tile, m + tile), radius=tile * 8 / 28, fill=255)
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    img.paste(fill, (0, 0), mask)

    # mouse glyph: same proportions as the site logo (10x17 inside a 28 tile, 14x24 viewBox)
    gw, gh = tile * 10 / 28, tile * 17 / 28
    k = gw / 14
    x0, y0 = (n - gw) / 2, (n - gh) / 2
    d = ImageDraw.Draw(img)
    sw = 2.2 * k
    d.rounded_rectangle((x0 + 0.3 * k, y0 + 0.3 * k, x0 + 13.7 * k, y0 + 23.7 * k), radius=6.7 * k, outline=INK, width=round(sw))
    d.line((x0 + 7 * k, y0 + 5.2 * k, x0 + 7 * k, y0 + 8.8 * k), fill=INK, width=round(sw))
    for yy in (5.2, 8.8):  # round caps
        r = sw / 2
        d.ellipse((x0 + 7 * k - r, y0 + yy * k - r, x0 + 7 * k + r, y0 + yy * k + r), fill=INK)
    return img


if __name__ == "__main__":
    big = draw()
    frames = [big.resize((s, s), Image.LANCZOS) for s in SIZES]
    frames[-1].save(OUT / "traymouse.ico", sizes=[(s, s) for s in SIZES], append_images=frames[:-1])
    big.resize((256, 256), Image.LANCZOS).save(OUT / "icon.png")
    print("wrote", OUT / "traymouse.ico")
