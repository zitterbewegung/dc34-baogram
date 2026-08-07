"""RawMono1 / PackBitsMono1 codecs — byte-exact mirror of
libraries/baogram-core/src/codec.rs.

PackBitsMono1 stream: control byte `c` —
* 0x00..=0x7F: literal, next c+1 bytes verbatim (1..=128);
* 0x81..=0xFF: run, next byte repeated 257-c times (2..=128);
* 0x80: reserved, decoders MUST reject.

Canonical encoder: runs of >= 3 identical bytes (capped 128) become run
blocks; everything else joins literal blocks capped at 128 bytes. The
Rust and Python encoders produce byte-identical output.
"""

from __future__ import annotations

CODEC_RAW_MONO1 = 0
CODEC_PACKBITS_MONO1 = 1


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


def encode_best(packed: bytes) -> tuple[int, bytes]:
    """Compress-or-raw decision; compressed only when strictly smaller."""
    compressed = packbits_encode(packed)
    if len(compressed) < len(packed):
        return CODEC_PACKBITS_MONO1, compressed
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
    raise CodecError(f"unknown codec {codec}")
