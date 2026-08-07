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
