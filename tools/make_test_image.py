#!/usr/bin/env python3
"""Generate the simulated demo photo for the hosted emulator's camera.

Writes ../test-images/simphoto.gray8 (raw 256x240 gray8, the format
BAO_CAMERA_IMAGE expects) plus a .png preview. The scene is photo-like
on purpose - gradients, silhouettes, text, and salt-and-pepper sensor
noise for the capture despeckle filter to clean.

Requires Pillow (available in the tools/baogram-host venv).
"""

import pathlib
import random

from PIL import Image, ImageDraw

W, H = 256, 240
OUT = pathlib.Path(__file__).resolve().parents[2] / "test-images"


def main() -> None:
    img = Image.new("L", (W, H))
    d = ImageDraw.Draw(img)
    for y in range(H):  # dusk sky gradient
        d.line([(0, y), (W, y)], fill=210 - y * 110 // H)
    d.ellipse([28, 26, 66, 64], fill=250)  # sun
    for x0 in (110, 150, 190):  # birds
        d.arc([x0, 40, x0 + 16, 52], 200, 340, fill=30)
    d.polygon([(0, 152), (60, 88), (120, 152)], fill=55)
    d.polygon([(80, 152), (170, 74), (256, 152)], fill=80)
    d.rectangle([0, 152, 256, 240], fill=150)  # ground
    for cx, tone in ((100, 35), (156, 45)):  # two badge-wearing figures
        d.ellipse([cx - 11, 130, cx + 11, 152], fill=235)
        d.rectangle([cx - 6, 152, cx + 6, 192], fill=tone)
        d.polygon([(cx - 6, 192), (cx - 13, 222), (cx - 6, 222)], fill=tone)
        d.polygon([(cx + 6, 192), (cx + 13, 222), (cx + 6, 222)], fill=tone)
        d.rectangle([cx - 4, 162, cx + 4, 174], fill=252)  # badges
    d.line([(106, 168), (150, 168)], fill=250, width=2)  # QR beam
    d.text((6, 198), "DC34 BAOGRAM v2", fill=15)
    d.text((6, 214), "badge to badge", fill=15)
    rng = random.Random(0xF00D)
    px = img.load()
    for _ in range(2600):  # sensor noise for despeckle to remove
        x, y = rng.randrange(W), rng.randrange(H)
        px[x, y] = max(0, min(255, px[x, y] + rng.choice([-70, 70])))
    raw = img.tobytes()
    assert len(raw) == W * H
    OUT.mkdir(exist_ok=True)
    (OUT / "simphoto.gray8").write_bytes(raw)
    img.save(OUT / "simphoto.png")
    print(f"wrote {OUT}/simphoto.gray8 (+.png), {len(raw)} bytes")


if __name__ == "__main__":
    main()
