#!/usr/bin/env python3
"""Generate the 6 face+glasses logos.

Geometry — all coordinates in a 1024x1024 viewBox:
  - dark squircle background fills the canvas
  - head: white-stroked circle, fixed at (512, 512)
  - glasses: two lens-circles + a bridge, translated as a unit by (dx, dy)
    to point the gaze in one of six directions
  - lens interiors are filled with the background color so the glasses
    read in front of the face outline instead of intersecting it
"""
from pathlib import Path

OUT = Path(__file__).parent

CANVAS = 1024
CENTER = CANVAS // 2
BG = "#13161c"
FG = "#ffffff"

HEAD_R = 259
HEAD_STROKE = 46

LENS_R = 70
LENS_FILL_R = 61
LENS_DX = 105         # half-distance between the two lens centers
BRIDGE_HALF = 35      # bridge runs from (-35, 0) to (+35, 0) before translation
GLASSES_STROKE = 38
MASK_R = 94

DIRECTIONS = {
    "straight":   (0,    0),
    "up-left":    (-90,  -85),
    "up-right":   ( 90,  -85),
    "down":       (0,     100),
    "down-left":  (-90,   85),
    "down-right": ( 90,   85),
}

def svg(direction: str, dx: int, dy: int, *, with_squircle: bool = True) -> str:
    gx, gy = CENTER + dx, CENTER + dy
    bg_layer = (
        f'  <rect x="0" y="0" width="{CANVAS}" height="{CANVAS}" '
        f'rx="230" ry="230" fill="{BG}"/>\n'
        if with_squircle else ""
    )
    return f'''<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{CANVAS}" height="{CANVAS}" viewBox="0 0 {CANVAS} {CANVAS}">
  <!-- {direction} -->
{bg_layer}  <g fill="none" stroke="{FG}" stroke-linecap="round">
    <defs>
      <mask id="head-mask" maskUnits="userSpaceOnUse" x="0" y="0" width="{CANVAS}" height="{CANVAS}">
        <rect x="0" y="0" width="{CANVAS}" height="{CANVAS}" fill="white"/>
        <circle cx="{gx - LENS_DX}" cy="{gy}" r="{MASK_R}" fill="black"/>
        <circle cx="{gx + LENS_DX}" cy="{gy}" r="{MASK_R}" fill="black"/>
      </mask>
    </defs>
    <!-- head -->
    <circle cx="{CENTER}" cy="{CENTER}" r="{HEAD_R}" stroke-width="{HEAD_STROKE}" mask="url(#head-mask)"/>
    <!-- glasses (translated as a group) -->
    <g stroke-width="{GLASSES_STROKE}">
      <circle cx="{gx - LENS_DX}" cy="{gy}" r="{LENS_FILL_R}" fill="{BG}" stroke="none"/>
      <circle cx="{gx + LENS_DX}" cy="{gy}" r="{LENS_FILL_R}" fill="{BG}" stroke="none"/>
      <circle cx="{gx - LENS_DX}" cy="{gy}" r="{LENS_R}"/>
      <circle cx="{gx + LENS_DX}" cy="{gy}" r="{LENS_R}"/>
      <line x1="{gx - BRIDGE_HALF}" y1="{gy}" x2="{gx + BRIDGE_HALF}" y2="{gy}"/>
    </g>
  </g>
</svg>
'''

if __name__ == "__main__":
    for name, (dx, dy) in DIRECTIONS.items():
        (OUT / f"{name}.svg").write_text(svg(name, dx, dy))
    # Bare (no squircle) variant of "straight" for embedding inside the pill.
    (OUT / "straight-bare.svg").write_text(
        svg("straight-bare", 0, 0, with_squircle=False)
    )
    print(f"wrote {len(DIRECTIONS) + 1} svgs to {OUT}")
