"""RawMono1 / PackBitsMono1 / RowDeltaMono1 / CtxArithMono1 codecs —
byte-exact mirror of libraries/baogram-core/src/codec.rs.

PackBitsMono1 stream: control byte `c` —
* 0x00..=0x7F: literal, next c+1 bytes verbatim (1..=128);
* 0x81..=0xFF: run, next byte repeated 257-c times (2..=128);
* 0x80: reserved, decoders MUST reject.

Canonical encoder: runs of >= 3 identical bytes (capped 128) become run
blocks; everything else joins literal blocks capped at 128 bytes. The
Rust and Python encoders produce byte-identical output.

RowDeltaMono1 (codec 2): the input is row-filtered (each row is XORed
with the ORIGINAL previous row; the first row is copied verbatim) and
the filtered bytes are PackBits-encoded with the identical stream
rules. The row stride is the packed width (width / 8: 32 full-format,
16 small-format); only defined for whole-row inputs.

CtxArithMono1 (codec 3): the context-modeled adaptive binary range
coder defined in `.ctxcodec` (the normative algorithm mirror of
libraries/baogram-core/src/ctxcodec.rs).
"""

from __future__ import annotations

CODEC_RAW_MONO1 = 0
CODEC_PACKBITS_MONO1 = 1
CODEC_ROWDELTA_MONO1 = 2
CODEC_CTXARITH_MONO1 = 3


class CodecError(ValueError):
    pass


# Imported after CodecError is defined: ctxcodec imports CodecError from
# this module, so the circular import resolves in either load order.
from . import ctxcodec  # noqa: E402


def packbits_encode(data: bytes) -> bytes:
    out = bytearray()
    i = 0
    n = len(data)
    literal_start = None

    def flush_literal(end: int) -> None:
        nonlocal literal_start
        if literal_start is None:
            return
        lit = data[literal_start:end]
        assert 0 < len(lit) <= 128
        out.append(len(lit) - 1)
        out.extend(lit)
        literal_start = None

    while i < n:
        b = data[i]
        run = 1
        while run < 128 and i + run < n and data[i + run] == b:
            run += 1
        if run >= 3:
            flush_literal(i)
            out.append(257 - run)
            out.append(b)
            i += run
        else:
            if literal_start is None:
                literal_start = i
            i += run
            if i - literal_start >= 128:
                end = literal_start + 128
                lit = data[literal_start:end]
                out.append(len(lit) - 1)
                out.extend(lit)
                literal_start = end if end < i else None
    if literal_start is not None:
        flush_literal(n)
    return bytes(out)


def packbits_decode(data: bytes, expected_len: int) -> bytes:
    out = bytearray()
    i = 0
    n = len(data)
    while i < n:
        if len(out) == expected_len:
            raise CodecError("trailing garbage after complete output")
        c = data[i]
        i += 1
        if c == 0x80:
            raise CodecError("invalid run control byte 0x80")
        if c < 0x80:
            count = c + 1
            if i + count > n:
                raise CodecError("premature end of literal block")
            if len(out) + count > expected_len:
                raise CodecError("output overflow")
            out.extend(data[i : i + count])
            i += count
        else:
            count = 257 - c
            if i >= n:
                raise CodecError("premature end of run block")
            if len(out) + count > expected_len:
                raise CodecError("output overflow")
            out.extend(bytes([data[i]]) * count)
            i += 1
    if len(out) < expected_len:
        raise CodecError("premature end of stream")
    return bytes(out)


def rowdelta_filter(data: bytes, row_bytes: int) -> bytes:
    """Row filter: F[0:rb] = P[0:rb]; F[i] = P[i] XOR P[i-rb] for i >= rb,
    where P is always the ORIGINAL input (rows processed top-down against
    the original previous row). `row_bytes` is the packed row stride
    (width / 8: 32 full-format, 16 small-format)."""
    if row_bytes <= 0 or len(data) % row_bytes != 0:
        raise CodecError("row-delta input length not a multiple of the row stride")
    out = bytearray(data)
    for i in range(row_bytes, len(data)):
        out[i] ^= data[i - row_bytes]
    return bytes(out)


def rowdelta_unfilter(data: bytes, row_bytes: int) -> bytes:
    """Inverse row filter: O[0:rb] = F[0:rb]; O[i] = F[i] XOR O[i-rb] for
    i >= rb (uses the RECONSTRUCTED previous row)."""
    if row_bytes <= 0 or len(data) % row_bytes != 0:
        raise CodecError("row-delta input length not a multiple of the row stride")
    out = bytearray(data)
    for i in range(row_bytes, len(out)):
        out[i] ^= out[i - row_bytes]
    return bytes(out)


def encode_best(packed: bytes, width_px: int, height_px: int) -> tuple[int, bytes]:
    """Compress-or-raw decision; compressed only when strictly smaller.
    PackBits (1), row-delta + PackBits (2), and CtxArithMono1 (3) are all
    computed; the smallest wins, with ties going to the LOWEST codec id;
    the winner is used only when strictly smaller than the raw input,
    else raw (0)."""
    assert width_px % 8 == 0 and len(packed) == width_px // 8 * height_px
    row_bytes = width_px // 8
    codec_id, best = CODEC_PACKBITS_MONO1, packbits_encode(packed)
    rowdelta = packbits_encode(rowdelta_filter(packed, row_bytes))
    if len(rowdelta) < len(best):
        codec_id, best = CODEC_ROWDELTA_MONO1, rowdelta
    ctx = ctxcodec.ctx_encode(packed, width_px, height_px)
    if len(ctx) < len(best):
        codec_id, best = CODEC_CTXARITH_MONO1, ctx
    if len(best) < len(packed):
        return codec_id, best
    return CODEC_RAW_MONO1, bytes(packed)


def decode(codec: int, encoded: bytes, expected_len: int, row_bytes: int) -> bytes:
    """Decode `encoded` according to `codec`, requiring exactly
    `expected_len` output bytes. `row_bytes` is the packed row stride of
    the target image (width / 8), needed by the 2D-aware codecs."""
    if row_bytes == 0 or expected_len % row_bytes != 0:
        raise CodecError("expected length not a whole number of rows (noncanonical)")
    if codec == CODEC_RAW_MONO1:
        if len(encoded) != expected_len:
            raise CodecError("raw payload has noncanonical length")
        return bytes(encoded)
    if codec == CODEC_PACKBITS_MONO1:
        if len(encoded) >= expected_len:
            raise CodecError("compressed payload not smaller than raw (noncanonical)")
        return packbits_decode(encoded, expected_len)
    if codec == CODEC_ROWDELTA_MONO1:
        if len(encoded) >= expected_len:
            raise CodecError("compressed payload not smaller than raw (noncanonical)")
        return rowdelta_unfilter(packbits_decode(encoded, expected_len), row_bytes)
    if codec == CODEC_CTXARITH_MONO1:
        if len(encoded) >= expected_len:
            raise CodecError("compressed payload not smaller than raw (noncanonical)")
        return ctxcodec.ctx_decode(encoded, row_bytes * 8, expected_len // row_bytes)
    raise CodecError(f"unknown codec {codec}")
