"""Builds assets/demo.gif and assets/states.png for the README.

1. cargo test --release showcase -- --ignored   (renders the widget at 3x into %TEMP%/squeak_showcase)
2. python tools/showcase.py                      (needs Pillow; places renders on a mock Windows 11 taskbar)
"""
import os
import pathlib
import tempfile

from PIL import Image, ImageDraw, ImageFont

S = 3  # renders are 3x, so everything here is drawn at 3x too
SRC = pathlib.Path(tempfile.gettempdir()) / "squeak_showcase"
OUT = pathlib.Path(__file__).resolve().parent.parent / "assets"
FONTS = pathlib.Path(os.environ.get("WINDIR", r"C:\Windows")) / "Fonts"
BAR = (32, 32, 34)
TEXT = (235, 235, 235)


def font(name, px):
    return ImageFont.truetype(str(FONTS / name), px * S)


def load(name):
    raw = (SRC / f"{name}.rgba").read_bytes()
    head, data = raw.split(b"\n", 1)
    w, h = map(int, head.split())
    return Image.frombytes("RGBA", (w, h), data)


def taskbar(widget):
    """Right end of a Windows 11 taskbar: widget, tray chevron, a few tray icons and the clock."""
    w, h = 430 * S, 48 * S
    img = Image.new("RGBA", (w, h), BAR + (255,))
    d = ImageDraw.Draw(img)
    d.line((0, 0, w, 0), fill=(54, 54, 58), width=S)  # top hairline
    icons = font("SegoeIcons.ttf", 13)
    x = w - 12 * S
    clock = font("segoeui.ttf", 11)
    for i, line in enumerate(("4:51 PM", "11/17/2026")):
        d.text((x, h / 2 + (i - 0.5) * 16 * S), line, font=clock, fill=TEXT, anchor="rm")
    x -= 74 * S
    for glyph in ("\ue995", "\ue701"):  # speaker, wifi
        d.text((x, h / 2), glyph, font=icons, fill=TEXT, anchor="rm")
        x -= 24 * S
    d.text((x, h / 2), "ENG", font=font("segoeui.ttf", 11), fill=TEXT, anchor="rm")
    x -= 40 * S
    chevron_right = x
    d.text((x, h / 2), "\ue70e", font=font("SegoeIcons.ttf", 11), fill=TEXT, anchor="rm")
    x = chevron_right - 22 * S  # tray's left edge; the widget sits 2px left of it
    img.alpha_composite(widget, (int(x - 2 * S - widget.width), (h - widget.height) // 2))
    return img


def rounded(img, radius=10 * S, pad=24 * S, bg=(18, 18, 20)):
    """Card with rounded corners so it looks good on both GitHub themes."""
    card = Image.new("RGBA", (img.width + 2 * pad, img.height + 2 * pad), (0, 0, 0, 0))
    mask = Image.new("L", card.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, card.width - 1, card.height - 1), radius, fill=255)
    card.paste(Image.new("RGBA", card.size, bg + (255,)), (0, 0), mask)
    card.alpha_composite(img, (pad, pad))
    return card


def demo_gif():
    seq = [("87_lavender", 1600), ("87_cyan", 900), ("87_mint", 900), ("87_pink", 900), ("87_gold", 900),
           ("18_low", 1600), ("64_charging", 1600), ("12_charging", 1300), ("x_off", 1600)]
    frames = [rounded(taskbar(load(n))).convert("RGB") for n, _ in seq]
    frames[0].save(OUT / "demo.gif", save_all=True, append_images=frames[1:],
                   duration=[ms for _, ms in seq], loop=0, optimize=True)


def states_png():
    rows = [
        ("Battery level", ["87_lavender"]),
        ("Pick any color (right-click > Mouse color)", ["87_cyan", "87_mint", "87_pink", "87_gold", "87_red"]),
        ("Below 25%: mouse and number turn red (works with a red mouse too)", ["18_low", "18_low_red"]),
        ("Plugged in: bolt + current level", ["64_charging", "12_charging"]),
        ("Receiver unplugged or mouse off", ["x_off"]),
    ]
    label = font("segoeui.ttf", 11)
    row_h, gap, left = 48 * S, 14 * S, 20 * S
    width = 520 * S
    img = Image.new("RGBA", (width, len(rows) * (row_h + 22 * S + gap) + gap), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    y = gap
    for title, names in rows:
        d.text((left, y), title, font=label, fill=(170, 170, 178))
        y += 20 * S
        d.rounded_rectangle((left - 8 * S, y, width - left, y + row_h), 8 * S, fill=BAR + (255,))
        x = left
        for n in names:
            r = load(n)
            img.alpha_composite(r, (x, y + (row_h - r.height) // 2))
            x += r.width + 18 * S
        y += row_h + gap
    rounded(img, pad=8 * S).save(OUT / "states.png", optimize=True)


if __name__ == "__main__":
    OUT.mkdir(exist_ok=True)
    demo_gif()
    states_png()
    print("wrote", OUT / "demo.gif", "and", OUT / "states.png")
