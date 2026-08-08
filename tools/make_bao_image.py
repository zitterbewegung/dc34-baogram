#!/usr/bin/env python3
"""Draw a 128x128 1-bit bao (steamed bun): a sample upload for dc34-image.

Writes ../test-images/bao.png, ready to send with
`dc34-image --port <port> --image bao.png` (no --force needed - it is
already 128x128 and 1-bit). See ../BAOGRAM_IMAGE_UPLOAD.md.

Requires Pillow (available in the tools/baogram-host venv).

Clean line art: the badge is 1bpp and the upload path thresholds, so bold
outlines and solid fills survive where photographic shading would not.

The bun silhouette is drawn as one filled union and its outline derived by
erosion, so the dome and the base leave no seam where they overlap.
"""
import math
import pathlib

from PIL import Image, ImageDraw, ImageFilter, ImageChops

W = H = 128
SS = 4
BIG = (W * SS, H * SS)


def S(*v):
    return [c * SS for c in v]


# --- bun silhouette: dome + flat base, unioned -----------------------
sil = Image.new("L", BIG, 0)
ds = ImageDraw.Draw(sil)
ds.pieslice(S(10, 34, 118, 112), start=180, end=360, fill=255)
ds.rounded_rectangle(S(10, 72, 118, 106), radius=22 * SS, fill=255)

# outline = silhouette minus an eroded copy
inner = sil.filter(ImageFilter.MinFilter(3 * SS + 1))
outline = ImageChops.subtract(sil, inner)

# --- the drawing canvas ----------------------------------------------
img = Image.new("L", BIG, 255)
d = ImageDraw.Draw(img)

# steam curls, above the bun
for x0, phase in ((42, 0.0), (64, 1.1), (86, 2.2)):
    pts = [((x0 + 7.0 * math.sin(t / 4.0 + phase)) * SS, (3 + t * 1.4) * SS) for t in range(20)]
    d.line(pts, fill=0, width=3 * SS, joint="curve")

# paste the bun: white interior, black outline
img.paste(255, (0, 0), sil)
img.paste(0, (0, 0), outline)

# --- pleats: four bold strokes sweeping down from the crown ----------
knot = (64, 50)
for k in (-2, -1, 1, 2):
    a0 = math.radians(90 - k * 27)
    pts = []
    for t in range(14):
        r = 12 + t * 2.9
        ang = a0 + k * t * 0.02
        pts.append(((knot[0] + r * math.cos(ang)) * SS,
                    (knot[1] - r * math.sin(ang) + t * t * 0.22) * SS))
    d.line(pts, fill=0, width=3 * SS, joint="curve")

# --- the twisted knot at the crown -----------------------------------
d.ellipse(S(knot[0] - 11, knot[1] - 11, knot[0] + 11, knot[1] + 11),
          fill=255, outline=0, width=3 * SS)
d.arc(S(knot[0] - 7, knot[1] - 7, knot[0] + 7, knot[1] + 7),
      start=205, end=25, fill=0, width=2 * SS)

# --- plate -----------------------------------------------------------
d.line(S(14, 110, 114, 110), fill=0, width=3 * SS)
d.arc(S(6, 102, 122, 122), start=0, end=180, fill=0, width=3 * SS)

img = img.resize((W, H), Image.LANCZOS).point(lambda p: 0 if p < 150 else 255).convert("1")
out = pathlib.Path(__file__).resolve().parents[2] / "test-images" / "bao.png"
out.parent.mkdir(parents=True, exist_ok=True)
img.save(out)

px = img.load()
for y in range(0, H, 2):
    print("".join("#" if px[x, y] == 0 else "." for x in range(W)))
print("saved", out)
