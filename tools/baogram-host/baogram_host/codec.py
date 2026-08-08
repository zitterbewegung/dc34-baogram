"""RawMono1 / PackBitsMono1 / RowDeltaMono1 codecs — byte-exact mirror of
libraries/baogram-core/src/codec.rs.

PackBitsMono1 stream: control byte `c` —
* 0x00..=0x7F: literal, next c+1 bytes verbatim (1..=128);
* 0x81..=0xFF: run, next byte repeated 257-c times (2..=128);
* 0x80: reserved, decoders MUST reject.

Canonical encoder: runs of >= 3 identical bytes (capped 128) become run
blocks; everything else joins literal blocks capped at 128 bytes. The
Rust and Python encoders produce byte-identical output.

RowDeltaMono1 (codec 2): the input is row-filtered (each 32-byte row is
XORed with the ORIGINAL previous row; the first row is copied verbatim)
and the filtered bytes are PackBits-encoded with the identical stream
rules. Only defined for inputs whose length is a multiple of the 32-byte
row stride.
"""

from __future__ import annotations

CODEC_RAW_MONO1 = 0
CODEC_PACKBITS_MONO1 = 1
CODEC_ROWDELTA_MONO1 = 2

ROWDELTA_ROW_STRIDE = 32


class CodecError(ValueError):
    pass


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


def rowdelta_filter(data: bytes) -> bytes:
    """Row filter: F[0:32] = P[0:32]; F[i] = P[i] XOR P[i-32] for i >= 32,
    where P is always the ORIGINAL input (rows processed top-down against
    the original previous row)."""
    if len(data) % ROWDELTA_ROW_STRIDE != 0:
        raise CodecError("row-delta input length not a multiple of the row stride")
    out = bytearray(data)
    for i in range(ROWDELTA_ROW_STRIDE, len(data)):
        out[i] ^= data[i - ROWDELTA_ROW_STRIDE]
    return bytes(out)


def rowdelta_unfilter(data: bytes) -> bytes:
    """Inverse row filter: O[0:32] = F[0:32]; O[i] = F[i] XOR O[i-32] for
    i >= 32 (uses the RECONSTRUCTED previous row)."""
    if len(data) % ROWDELTA_ROW_STRIDE != 0:
        raise CodecError("row-delta input length not a multiple of the row stride")
    out = bytearray(data)
    for i in range(ROWDELTA_ROW_STRIDE, len(out)):
        out[i] ^= out[i - ROWDELTA_ROW_STRIDE]
    return bytes(out)


def encode_best(packed: bytes) -> tuple[int, bytes]:
    """Compress-or-raw decision; compressed only when strictly smaller.
    RowDelta is tried when the input is a whole number of rows and wins
    only when strictly smaller than plain PackBits (ties -> codec 1)."""
    codec_id, best = CODEC_PACKBITS_MONO1, packbits_encode(packed)
    if len(packed) % ROWDELTA_ROW_STRIDE == 0:
        filtered = packbits_encode(rowdelta_filter(packed))
        if len(filtered) < len(best):
            codec_id, best = CODEC_ROWDELTA_MONO1, filtered
    if len(best) < len(packed):
        return codec_id, best
    return CODEC_RAW_MONO1, bytes(packed)


def decode(codec: int, encoded: bytes, expected_len: int) -> bytes:
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
        if expected_len % ROWDELTA_ROW_STRIDE != 0:
            raise CodecError("row-delta output length not a row multiple (noncanonical)")
        return rowdelta_unfilter(packbits_decode(encoded, expected_len))
    raise CodecError(f"unknown codec {codec}")
