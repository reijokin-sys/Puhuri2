#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Generate the Puhuri 2 icon: a wide P cut out of a green speech bubble.

The hand-drawn letter needs no external font. Coordinates use a 256px canvas;
supersampling smooths the final launcher icon without changing its layout.

    python3 tools/make-icon.py

Copyright (C) 2026 Reijo Kinnunen - SPDX-License-Identifier: AGPL-3.0-or-later
"""
from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 256
SCALE = 4
BACKGROUND = (0, 0, 0, 255)         # #000000
GREEN = (62, 179, 79, 255)         # #3EB34F
BUBBLE = [40, 62, 216, 170]
BUBBLE_RADIUS = 26
TAIL = [(74, 168), (74, 206), (118, 168)]


def box(bounds):
    return tuple(value * SCALE for value in bounds)


def main():
    image = Image.new("RGBA", (SIZE * SCALE, SIZE * SCALE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle(box([0, 0, SIZE, SIZE]), radius=56 * SCALE, fill=BACKGROUND)
    draw.rounded_rectangle(box(BUBBLE), radius=BUBBLE_RADIUS * SCALE, fill=GREEN)
    draw.polygon([(x * SCALE, y * SCALE) for x, y in TAIL], fill=GREEN)

    # A slightly wide, bold P, optically centred inside the bubble.
    # Square stem/joins, rounded bowl; the counter preserves the bubble green.
    draw.rounded_rectangle(box([96, 80, 160, 129]), radius=24 * SCALE, fill=BACKGROUND)
    draw.rectangle(box([96, 80, 111, 154]), fill=BACKGROUND)
    draw.rectangle(box([96, 80, 136, 95]), fill=BACKGROUND)
    draw.rectangle(box([96, 114, 136, 129]), fill=BACKGROUND)
    draw.rounded_rectangle(box([111, 95, 145, 114]), radius=9 * SCALE, fill=GREEN)
    draw.rectangle(box([111, 95, 128, 114]), fill=GREEN)

    image = image.resize((SIZE, SIZE), Image.Resampling.LANCZOS)
    target = Path(__file__).resolve().parents[1] / "app/assets/puhuri-black-green.png"
    image.save(target, "PNG", optimize=True)
    print(f"wrote {target} ({target.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
