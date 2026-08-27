#!/usr/bin/env python
"""Erzeugt assets/diktier.ico (Phase 5, Paket F).

Kreis in der Tray-Farbe fuer `idle` (46, 204, 64) mit weissem Mikrofon.
Gezeichnet wird 8-fach ueberabgetastet und dann herunterskaliert — bei 16 px
ist das der Unterschied zwischen Symbol und Matsch.

    python scripts/make-icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

IDLE = (46, 204, 64, 255)
WHITE = (255, 255, 255, 255)
SIZES = (16, 32, 48, 256)
SS = 8  # Ueberabtastung


def render(size: int) -> Image.Image:
    n = size * SS
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    def box(x0, y0, x1, y1):
        return [x0 * n, y0 * n, x1 * n, y1 * n]

    d.ellipse(box(0.02, 0.02, 0.98, 0.98), fill=IDLE)

    lw = max(1, round(0.055 * n))
    d.rounded_rectangle(box(0.385, 0.20, 0.615, 0.56), radius=0.115 * n, fill=WHITE)
    d.arc(box(0.30, 0.30, 0.70, 0.68), start=0, end=180, fill=WHITE, width=lw)
    d.rounded_rectangle(box(0.472, 0.62, 0.528, 0.79), radius=0.028 * n, fill=WHITE)
    d.rounded_rectangle(box(0.355, 0.76, 0.645, 0.815), radius=0.028 * n, fill=WHITE)

    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    out = Path(__file__).resolve().parent.parent / "assets" / "diktier.ico"
    out.parent.mkdir(parents=True, exist_ok=True)
    frames = [render(s) for s in SIZES]
    frames[-1].save(out, format="ICO", sizes=[(s, s) for s in SIZES], append_images=frames[:-1])
    print(f"OK: {out} ({out.stat().st_size} Bytes, {', '.join(str(s) for s in SIZES)})")


if __name__ == "__main__":
    main()
