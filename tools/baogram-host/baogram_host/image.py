"""Mono1 image handling: quantization, packing, PNG import/export, and the
deterministic synthetic test frame.

Mirrors libraries/baogram-core/src/image.rs: 256x240, packed MSB-first,
row-major, bit 1 = white. Quantization threshold = floor(mean luminance);
a pixel is white iff luminance > threshold. No dithering.
"""

from __future__ import annotations

IMAGE_WIDTH = 256
IMAGE_HEIGHT = 240
IMAGE_PIXELS = IMAGE_WIDTH * IMAGE_HEIGHT  # 61,440
MONO1_ROW_BYTES = IMAGE_WIDTH // 8  # 32
MONO1_PACKED_LEN = MONO1_ROW_BYTES * IMAGE_HEIGHT  # 7,680

# Small format (pixel format 2): the badge display's native resolution.
SMALL_WIDTH = 128
SMALL_HEIGHT = 120
SMALL_PIXELS = SMALL_WIDTH * SMALL_HEIGHT  # 15,360
SMALL_ROW_BYTES = SMALL_WIDTH // 8  # 16
SMALL_PACKED_LEN = SMALL_ROW_BYTES * SMALL_HEIGHT  # 1,920


class ImageError(ValueError):
    pass


def global_threshold(gray: bytes) -> int:
    if len(gray) != IMAGE_PIXELS:
        raise ImageError("frame must be exactly 61,440 bytes")
    return sum(gray) // len(gray)


def quantize(gray: bytes, threshold: int | None = None) -> bytes:
    """8-bit grayscale frame -> packed Mono1 (7,680 bytes)."""
    if len(gray) != IMAGE_PIXELS:
        raise ImageError("frame must be exactly 61,440 bytes")
    t = global_threshold(gray) if threshold is None else threshold
    packed = bytearray(MONO1_PACKED_LEN)
    for y in range(IMAGE_HEIGHT):
        row = gray[y * IMAGE_WIDTH : (y + 1) * IMAGE_WIDTH]
        base = y * MONO1_ROW_BYTES
        for xb in range(MONO1_ROW_BYTES):
            b = 0
            for bit in range(8):
                if row[xb * 8 + bit] > t:
                    b |= 0x80 >> bit
            packed[base + xb] = b
    return bytes(packed)


def unpack_to_gray(packed: bytes) -> bytes:
    """Packed Mono1 -> 8-bit grayscale bytes (white=255, black=0)."""
    if len(packed) != MONO1_PACKED_LEN:
        raise ImageError("packed image must be exactly 7,680 bytes")
    out = bytearray(IMAGE_PIXELS)
    for y in range(IMAGE_HEIGHT):
        base = y * MONO1_ROW_BYTES
        for xb in range(MONO1_ROW_BYTES):
            b = packed[base + xb]
            for bit in range(8):
                if b & (0x80 >> bit):
                    out[y * IMAGE_WIDTH + xb * 8 + bit] = 255
    return bytes(out)


def load_png_as_mono1(path: str, threshold: int | None = None) -> bytes:
    """Load an image file, convert to 256x240 grayscale (letterbox-free
    resize), and quantize to packed Mono1."""
    from PIL import Image

    img = Image.open(path).convert("L")
    if img.size != (IMAGE_WIDTH, IMAGE_HEIGHT):
        img = img.resize((IMAGE_WIDTH, IMAGE_HEIGHT))
    return quantize(img.tobytes(), threshold)


def save_mono1_as_png(packed: bytes, path: str) -> None:
    """Export packed Mono1 to a 256x240 grayscale PNG (white=255)."""
    from PIL import Image

    gray = unpack_to_gray(packed)
    img = Image.frombytes("L", (IMAGE_WIDTH, IMAGE_HEIGHT), gray)
    img.save(path, format="PNG")


def small_from_gray(gray: bytes, threshold: int | None = None) -> bytes:
    """Quantize a full 256x240 grayscale frame to a packed small (128x120)
    image: 2x2 box average in the grayscale domain (integer floor
    division), then the deterministic mean threshold over the DOWNSCALED
    frame; a pixel is white iff luminance > threshold. Byte-exact mirror
    of Mono1Small::from_gray in image.rs."""
    if len(gray) != IMAGE_PIXELS:
        raise ImageError("frame must be exactly 61,440 bytes")
    small_gray = bytearray(SMALL_PIXELS)
    for y in range(SMALL_HEIGHT):
        r0 = (y * 2) * IMAGE_WIDTH
        r1 = r0 + IMAGE_WIDTH
        base = y * SMALL_WIDTH
        for x in range(SMALL_WIDTH):
            x2 = x * 2
            small_gray[base + x] = (
                gray[r0 + x2] + gray[r0 + x2 + 1] + gray[r1 + x2] + gray[r1 + x2 + 1]
            ) // 4
    t = sum(small_gray) // SMALL_PIXELS if threshold is None else threshold
    packed = bytearray(SMALL_PACKED_LEN)
    for y in range(SMALL_HEIGHT):
        base = y * SMALL_WIDTH
        obase = y * SMALL_ROW_BYTES
        for x in range(SMALL_WIDTH):
            if small_gray[base + x] > t:
                packed[obase + (x >> 3)] |= 0x80 >> (x & 7)
    return bytes(packed)


def despeckle_small(packed: bytes) -> bytes:
    """3x3 majority despeckle (border coordinates clamp/replicate): a
    pixel becomes white iff at least 5 of the 9 samples are white.
    Byte-exact mirror of Mono1Small::despeckle in image.rs."""
    if len(packed) != SMALL_PACKED_LEN:
        raise ImageError("small packed image must be exactly 1,920 bytes")
    out = bytearray(SMALL_PACKED_LEN)
    max_x = SMALL_WIDTH - 1
    max_y = SMALL_HEIGHT - 1
    for y in range(SMALL_HEIGHT):
        for x in range(SMALL_WIDTH):
            white = 0
            for dy in (-1, 0, 1):
                sy = min(max(y + dy, 0), max_y)
                row = sy * SMALL_ROW_BYTES
                for dx in (-1, 0, 1):
                    sx = min(max(x + dx, 0), max_x)
                    if packed[row + (sx >> 3)] & (0x80 >> (sx & 7)):
                        white += 1
            if white >= 5:
                out[y * SMALL_ROW_BYTES + (x >> 3)] |= 0x80 >> (x & 7)
    return bytes(out)


def small_to_full(packed: bytes) -> bytes:
    """Pixel-double a packed small image (1,920 bytes) to the full 256x240
    format (7,680 bytes): each small pixel becomes a 2x2 block. Lossless
    with respect to the small image's content. Byte-exact mirror of
    Mono1Small::to_full in image.rs."""
    if len(packed) != SMALL_PACKED_LEN:
        raise ImageError("small packed image must be exactly 1,920 bytes")
    out = bytearray(MONO1_PACKED_LEN)
    for y in range(SMALL_HEIGHT):
        row = y * SMALL_ROW_BYTES
        out0 = (y * 2) * MONO1_ROW_BYTES
        out1 = out0 + MONO1_ROW_BYTES
        for x in range(SMALL_WIDTH):
            if packed[row + (x >> 3)] & (0x80 >> (x & 7)):
                fx = x * 2
                mask = 0xC0 >> (fx & 7)  # both pixels of the 2x2 block's row
                out[out0 + (fx >> 3)] |= mask
                out[out1 + (fx >> 3)] |= mask
    return bytes(out)


def synthetic_test_frame() -> bytes:
    """Deterministic 256x240 grayscale frame, byte-identical to
    baogram-core::image::synthetic_test_frame and the hosted badge camera.
    SHA-256: c82bfcf7e7ee582e204c151af00570cb832dcddced95df56f484d1cc7d0303db
    """
    frame = bytearray(IMAGE_PIXELS)
    for y in range(IMAGE_HEIGHT):
        for x in range(IMAGE_WIDTH):
            if y <= 59:
                v = x & 0xFF
            elif y <= 119:
                v = 32 if x < IMAGE_WIDTH // 2 else 224
            elif y <= 179:
                v = 255 if ((x // 16) + (y // 16)) % 2 == 0 else 0
            else:
                v = (y - 180) * 255 // 59
            frame[y * IMAGE_WIDTH + x] = v
    return bytes(frame)
