"""Tests for baogram_host.ctxcodec (CtxArithMono1).

The cross-language pins below were computed by the Rust reference
implementation (libraries/baogram-core/src/ctxcodec.rs) and are hardcoded
identically in its #[cfg(test)] suite. The Python encoder matching them
byte-for-byte is the acceptance criterion.
"""

import hashlib
import time

import pytest

from baogram_host.codec import CodecError
from baogram_host.ctxcodec import ctx_decode, ctx_encode

# ---- cross-language pins (from the Rust implementation) --------------------
PIN_NOISE_128_LEN = 1933
PIN_NOISE_128_SHA256 = "f97cbd9eb5ccd46628ca7e1e369b52a74df9219de0e1e2c4bb102f54a76b9201"
PIN_ZERO_128_LEN = 50
PIN_ZERO_128_SHA256 = "cc2786e1f9910a9d811400edcddaf7075195f7a16b216dcbefba3bc7c4f2ae51"
PIN_CHECKER_128_FIRST16 = "0056cf5fd2ac42309497985172fc1a4a"


def _xorshift_bytes(n: int) -> bytes:
    """xorshift32, seed 0x12345678, shifts 13/17/5, low byte (matches Rust)."""
    state = 0x12345678
    out = bytearray(n)
    for i in range(n):
        state ^= (state << 13) & 0xFFFFFFFF
        state ^= state >> 17
        state ^= (state << 5) & 0xFFFFFFFF
        out[i] = state & 0xFF
    return bytes(out)


def _checkerboard(w: int, h: int) -> bytes:
    # pixel(x, y) = (x + y) & 1: rows alternate 0x55 / 0xAA.
    return b"".join(
        bytes([0x55 if y % 2 == 0 else 0xAA]) * (w // 8) for y in range(h)
    )


def _horizontal_bands(w: int, h: int) -> bytes:
    # 16-row bands, first band black (0).
    return b"".join(
        bytes([0x00 if (y // 16) % 2 == 0 else 0xFF]) * (w // 8) for y in range(h)
    )


def _vertical_bands(w: int, h: int) -> bytes:
    # 16-px-wide bands, first band black (0): bytes 00 00 FF FF ...
    row = bytes(0x00 if (xb // 2) % 2 == 0 else 0xFF for xb in range(w // 8))
    return row * h


def _roundtrip(img: bytes, w: int, h: int) -> int:
    enc = ctx_encode(img, w, h)
    assert ctx_decode(enc, w, h) == img, f"roundtrip mismatch at {w}x{h}"
    return len(enc)


@pytest.mark.parametrize("w,h", [(128, 120), (256, 240)])
def test_roundtrip_flat_images(w, h):
    n = w // 8 * h
    _roundtrip(bytes(n), w, h)
    _roundtrip(b"\xff" * n, w, h)


@pytest.mark.parametrize("w,h", [(128, 120), (256, 240)])
def test_roundtrip_structured_images(w, h):
    _roundtrip(_checkerboard(w, h), w, h)
    _roundtrip(_horizontal_bands(w, h), w, h)
    _roundtrip(_vertical_bands(w, h), w, h)


@pytest.mark.parametrize("w,h", [(128, 120), (256, 240)])
def test_roundtrip_noise(w, h):
    _roundtrip(_xorshift_bytes(w // 8 * h), w, h)


def test_compression_sanity():
    enc_zero = ctx_encode(bytes(1920), 128, 120)
    assert len(enc_zero) < 64, f"all-zero 128x120 must encode to < 64 bytes, got {len(enc_zero)}"

    enc_noise = ctx_encode(_xorshift_bytes(1920), 128, 120)
    assert len(enc_noise) <= 1920 * 1.1, f"noise must stay <= ~1.1x input, got {len(enc_noise)}"


def test_rejects_truncated_stream():
    enc = ctx_encode(_xorshift_bytes(1920), 128, 120)
    # Chop enough that the decoder must run out mid-image (the last 4
    # flushed bytes are legitimately unread, so remove more than that).
    with pytest.raises(CodecError, match="premature end"):
        ctx_decode(enc[:-8], 128, 120)


def test_rejects_trailing_garbage():
    enc = ctx_encode(_xorshift_bytes(1920), 128, 120)
    with pytest.raises(CodecError, match="trailing"):
        ctx_decode(enc + b"\xde\xad\xbe\xef\x99", 128, 120)


def test_rejects_short_input():
    for n in range(5):
        with pytest.raises(CodecError, match="premature end"):
            ctx_decode(bytes(n), 128, 120)


def test_cross_language_pins():
    enc_noise = ctx_encode(_xorshift_bytes(1920), 128, 120)
    assert len(enc_noise) == PIN_NOISE_128_LEN
    assert hashlib.sha256(enc_noise).hexdigest() == PIN_NOISE_128_SHA256

    enc_zero = ctx_encode(bytes(1920), 128, 120)
    assert len(enc_zero) == PIN_ZERO_128_LEN
    assert hashlib.sha256(enc_zero).hexdigest() == PIN_ZERO_128_SHA256

    enc_checker = ctx_encode(_checkerboard(128, 120), 128, 120)
    assert enc_checker[:16].hex() == PIN_CHECKER_128_FIRST16


def test_timing_encode_noise_256x240():
    noise = _xorshift_bytes(7680)
    start = time.perf_counter()
    enc = ctx_encode(noise, 256, 240)
    elapsed = time.perf_counter() - start
    print(f"ctxcodec: encode 256x240 noise (7680 -> {len(enc)} bytes) took {elapsed * 1000:.1f} ms")
