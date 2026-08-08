"""CtxArithMono1: context-modeled adaptive binary range coder for Mono1.

A 10-bit-context adaptive binary range coder (LZMA-style rc, MOVE_BITS 5,
11-bit probabilities) for bilevel images packed MSB-first row-major, bit 1 =
white. This module is the byte-exact mirror of the Rust implementation in
``libraries/baogram-core/src/ctxcodec.rs``; both must produce identical
encodings (cross-language pins live in both test suites).

Wire semantics (frozen):

- Pixels are processed in raster order (y outer, x inner); out-of-bounds
  template reads are 0.
- Context (10 bits, three-line template), ``b(x, y)`` = pixel value::

    ctx = b(x-2,y)   << 0 | b(x-1,y)   << 1
        | b(x-2,y-1) << 2 | b(x-1,y-1) << 3 | b(x,y-1) << 4
        | b(x+1,y-1) << 5 | b(x+2,y-1) << 6
        | b(x-1,y-2) << 7 | b(x,y-2)   << 8 | b(x+1,y-2) << 9

- ``probs[ctx]`` is P(next bit is 0) on an 11-bit scale, initialized to
  1024; LZMA update rule with MOVE_BITS = 5 after coding each bit.
- Range encoder/decoder are the canonical LZMA binary rc. The first output
  byte is always 0 (the initial cache) and is kept; the decoder skips it and
  may leave up to 4 flushed bytes unread.

Note on ``shift_low``: the canonical C is ``p->low = (UInt32)p->low << 8;``
where the shift is performed in 32-bit arithmetic, i.e. the top byte of the
32-bit low is discarded (it has just been captured in ``cache`` or is
represented by a pending 0xFF). We implement exactly that
(``low = (low << 8) & 0xFFFFFFFF``), which keeps ``low < 2**32`` after every
shift and the carry in {0, 1}.
"""

from __future__ import annotations

from .codec import CodecError

_PROB_BITS = 11
_PROB_MAX = 1 << _PROB_BITS  # 2048
_PROB_INIT = _PROB_MAX // 2  # 1024
_MOVE_BITS = 5
_TOP = 1 << 24
_NUM_CTX = 1 << 10
_MASK32 = 0xFFFFFFFF


# ---------------------------------------------------------------------------
# Range encoder (canonical LZMA binary rc)
# ---------------------------------------------------------------------------


class _RangeEncoder:
    __slots__ = ("low", "range", "cache", "cache_size", "out")

    def __init__(self) -> None:
        self.low = 0  # < 2**33 always (32 bits + carry)
        self.range = 0xFFFFFFFF
        self.cache = 0
        self.cache_size = 1
        self.out = bytearray()

    def _shift_low(self) -> None:
        # Canonical LZMA RangeEnc_ShiftLow.
        if (self.low & _MASK32) < 0xFF000000 or (self.low >> 32) != 0:
            carry = self.low >> 32  # 0 or 1
            temp = self.cache
            # do { WriteByte(temp + carry); temp = 0xFF; } while (--cacheSize);
            while True:
                self.out.append((temp + carry) & 0xFF)
                temp = 0xFF
                self.cache_size -= 1
                if self.cache_size == 0:
                    break
            self.cache = (self.low >> 24) & 0xFF
        self.cache_size += 1
        # Canonical C: `p->low = (UInt32)p->low << 8;` — 32-bit arithmetic.
        self.low = (self.low << 8) & _MASK32

    def encode_bit(self, b: int, p: int) -> None:
        """Encode bit ``b`` with probability ``p`` = P(bit == 0), 11-bit scale."""
        bound = (self.range >> _PROB_BITS) * p
        if b == 0:
            self.range = bound
        else:
            self.low += bound
            self.range -= bound
        while self.range < _TOP:
            self._shift_low()
            self.range = (self.range << 8) & _MASK32

    def finish(self) -> bytes:
        for _ in range(5):
            self._shift_low()
        return bytes(self.out)


# ---------------------------------------------------------------------------
# Range decoder
# ---------------------------------------------------------------------------


class _RangeDecoder:
    __slots__ = ("range", "code", "data", "consumed")

    def __init__(self, data: bytes) -> None:
        # Byte 0 is the encoder's always-zero first byte; bytes 1..5 seed code.
        if len(data) < 5:
            raise CodecError("premature end of stream")
        self.range = 0xFFFFFFFF
        self.code = int.from_bytes(data[1:5], "big")
        self.data = data
        self.consumed = 5

    def _next_byte(self) -> int:
        if self.consumed >= len(self.data):
            raise CodecError("premature end of stream")
        b = self.data[self.consumed]
        self.consumed += 1
        return b

    def decode_bit(self, p: int) -> int:
        """Decode one bit with probability ``p`` = P(bit == 0), 11-bit scale."""
        bound = (self.range >> _PROB_BITS) * p
        if self.code < bound:
            b = 0
            self.range = bound
        else:
            b = 1
            self.code -= bound
            self.range -= bound
        while self.range < _TOP:
            self.range = (self.range << 8) & _MASK32
            self.code = ((self.code << 8) & _MASK32) | self._next_byte()
        return b


# ---------------------------------------------------------------------------
# Context model
# ---------------------------------------------------------------------------


def _context_at(bits: bytearray, width_px: int, x: int, y: int) -> int:
    """10-bit three-line context for pixel (x, y); out-of-bounds reads are 0.

    ``bits`` is an unpacked buffer, one byte per pixel (0/1). The template
    only references already-coded pixels, so encoder and decoder see
    identical contexts.
    """

    def b(dx: int, dy: int) -> int:
        xx = x + dx
        yy = y + dy
        if xx < 0 or yy < 0 or xx >= width_px:
            return 0
        return bits[yy * width_px + xx]

    return (
        b(-2, 0)
        | b(-1, 0) << 1
        | b(-2, -1) << 2
        | b(-1, -1) << 3
        | b(0, -1) << 4
        | b(1, -1) << 5
        | b(2, -1) << 6
        | b(-1, -2) << 7
        | b(0, -2) << 8
        | b(1, -2) << 9
    )


def _update_prob(probs: list, ctx: int, b: int) -> None:
    """LZMA probability update after coding bit ``b`` (MOVE_BITS = 5)."""
    p = probs[ctx]
    if b == 0:
        probs[ctx] = p + ((_PROB_MAX - p) >> _MOVE_BITS)
    else:
        probs[ctx] = p - (p >> _MOVE_BITS)


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def ctx_encode(packed: bytes, width_px: int, height_px: int) -> bytes:
    """Encode a packed Mono1 image (MSB-first row-major, bit 1 = white).

    ``packed`` must be exactly ``width_px // 8 * height_px`` bytes and
    ``width_px`` must be divisible by 8.
    """
    if width_px % 8 != 0:
        raise ValueError("width_px must be divisible by 8")
    if len(packed) != width_px // 8 * height_px:
        raise ValueError("packed input length must be width_px/8*height_px")

    row_bytes = width_px // 8
    # Unpack to one byte per pixel for fast template access.
    bits = bytearray(width_px * height_px)
    for y in range(height_px):
        for x in range(width_px):
            bits[y * width_px + x] = (packed[y * row_bytes + x // 8] >> (7 - (x % 8))) & 1

    probs = [_PROB_INIT] * _NUM_CTX
    enc = _RangeEncoder()
    for y in range(height_px):
        for x in range(width_px):
            ctx = _context_at(bits, width_px, x, y)
            bit = bits[y * width_px + x]
            enc.encode_bit(bit, probs[ctx])
            _update_prob(probs, ctx, bit)
    return enc.finish()


def ctx_decode(data: bytes, width_px: int, height_px: int) -> bytes:
    """Decode a CtxArithMono1 stream back to the packed Mono1 image.

    Raises :class:`CodecError` with "premature end" if the input is shorter
    than 5 bytes or ends before all pixels are decoded, and with "trailing"
    if more than 4 unread bytes remain after the last pixel (the decoder
    legitimately leaves up to 4 flushed bytes unread).
    """
    if width_px % 8 != 0:
        raise ValueError("width_px must be divisible by 8")

    dec = _RangeDecoder(data)
    bits = bytearray(width_px * height_px)
    probs = [_PROB_INIT] * _NUM_CTX
    for y in range(height_px):
        for x in range(width_px):
            ctx = _context_at(bits, width_px, x, y)
            bit = dec.decode_bit(probs[ctx])
            _update_prob(probs, ctx, bit)
            bits[y * width_px + x] = bit
    if len(data) - dec.consumed > 4:
        raise CodecError("trailing bytes after complete image")

    row_bytes = width_px // 8
    out = bytearray(row_bytes * height_px)
    for y in range(height_px):
        for x in range(width_px):
            if bits[y * width_px + x]:
                out[y * row_bytes + x // 8] |= 1 << (7 - (x % 8))
    return bytes(out)
