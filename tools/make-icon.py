#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Generate assets/puhuri.png (the app icon).

The icon is a message bubble in the project's accent green on the same purple
as the other projects, with a lightning bolt cut out of it ("sähke" is a
telegram, a short message). The SVG next to this script is the same drawing:
that is the file the launcher reads, the PNG is what the click manifest
points at.

    python3 tools/make-icon.py

Copyright (C) 2026 Reijo Kinnunen - SPDX-License-Identifier: AGPL-3.0-or-later
"""

import os

from PIL import Image, ImageDraw

SIZE = 256
BACKGROUND = (119, 33, 111, 255)   # #77216F
GREEN = (62, 179, 79, 255)         # #3EB34F

BUBBLE = [40, 62, 216, 170]        # left, top, right, bottom
BUBBLE_RADIUS = 26
TAIL = [(74, 168), (74, 206), (118, 168)]
# Three wind lines, shorter downwards: a gust of wind ("puhuri") inside the
# message bubble. Drawn in the background colour, cut out of the green.
WIND = [
    (96, 92, 158, 92),
    (84, 116, 150, 116),
    (104, 140, 140, 140),
]


def main():
    image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)

    draw.rounded_rectangle([0, 0, SIZE - 1, SIZE - 1], radius=56, fill=BACKGROUND)
    draw.rounded_rectangle(BUBBLE, radius=BUBBLE_RADIUS, fill=GREEN)
    draw.polygon(TAIL, fill=GREEN)
    for left, top, right, bottom in WIND:
        draw.rounded_rectangle(
            [left, top, right, bottom], radius=(bottom - top) // 2, fill=BACKGROUND
        )

    target = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "app",
        "assets",
        "puhuri.png",
    )
    image.save(target, "PNG", optimize=True)
    print("wrote %s (%d bytes)" % (target, os.path.getsize(target)))


if __name__ == "__main__":
    main()
