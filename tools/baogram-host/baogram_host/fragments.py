"""BG v1 fragments and the any-order reassembler — byte-exact mirror of
libraries/baogram-core/src/fragment.rs. Fragments are Base45-encoded for
the QR text layer. CRC-32 is ISO-HDLC (zlib).
"""

from __future__ import annotations

import struct
import zlib
from dataclasses import dataclass

import base45

from .crypto import SHORT_ID_LEN
from .format import MAX_POST_BYTES

FRAGMENT_MAGIC = b"BG"
FRAGMENT_VERSION = 1
FRAGMENT_HEADER_LEN = 24
FRAGMENT_CRC_LEN = 4
MAX_FRAGMENT_COUNT = 4096
MAX_FRAGMENT_PAYLOAD = 1024
DEFAULT_FRAGMENT_PAYLOAD = 64


class FragmentError(ValueError):
    def __init__(self, reason: str, detail: str = ""):
        self.reason = reason
        super().__init__(f"{reason}{': ' + detail if detail else ''}")


def _validate_geometry(frag_count: int, total_len: int, chunk_size: int) -> None:
    if frag_count == 0 or frag_count > MAX_FRAGMENT_COUNT:
        raise FragmentError("count_out_of_range")
    if chunk_size == 0 or chunk_size > MAX_FRAGMENT_PAYLOAD:
        raise FragmentError("chunk_size_invalid")
    if total_len == 0 or total_len > MAX_POST_BYTES:
        raise FragmentError("geometry_invalid", "total length")
    if -(-total_len // chunk_size) != frag_count:  # ceil division
        raise FragmentError("geometry_invalid", "count != ceil(total/chunk)")


def _expected_payload_len(index: int, count: int, total_len: int, chunk_size: int) -> int:
    if index + 1 < count:
        return chunk_size
    return total_len - (count - 1) * chunk_size


@dataclass
class Fragment:
    short_post_id: bytes
    frag_index: int
    frag_count: int
    total_len: int
    chunk_size: int
    payload: bytes

    def to_bytes(self) -> bytes:
        body = (
            FRAGMENT_MAGIC
            + struct.pack(">BB", FRAGMENT_VERSION, 0)
            + self.short_post_id
            + struct.pack(
                ">HHIHH",
                self.frag_index,
                self.frag_count,
                self.total_len,
                self.chunk_size,
                len(self.payload),
            )
            + self.payload
        )
        return body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    def to_base45(self) -> str:
        return base45.b45encode(self.to_bytes()).decode("ascii")

    @classmethod
    def from_bytes(cls, data: bytes) -> "Fragment":
        if len(data) < FRAGMENT_HEADER_LEN + FRAGMENT_CRC_LEN:
            raise FragmentError("truncated")
        if data[0:2] != FRAGMENT_MAGIC:
            raise FragmentError("bad_magic")
        if data[2] != FRAGMENT_VERSION:
            raise FragmentError("unknown_version")
        if data[3] != 0:
            raise FragmentError("unknown_flags")
        (stored_crc,) = struct.unpack(">I", data[-FRAGMENT_CRC_LEN:])
        if zlib.crc32(data[:-FRAGMENT_CRC_LEN]) & 0xFFFFFFFF != stored_crc:
            raise FragmentError("crc_mismatch")
        short_post_id = data[4:12]
        frag_index, frag_count, total_len, chunk_size, payload_len = struct.unpack(
            ">HHIHH", data[12:24]
        )
        _validate_geometry(frag_count, total_len, chunk_size)
        if frag_index >= frag_count:
            raise FragmentError("index_out_of_range")
        if payload_len != _expected_payload_len(frag_index, frag_count, total_len, chunk_size):
            raise FragmentError("payload_len_invalid")
        expected_total = FRAGMENT_HEADER_LEN + payload_len + FRAGMENT_CRC_LEN
        if len(data) < expected_total:
            raise FragmentError("truncated")
        if len(data) > expected_total:
            raise FragmentError("trailing_bytes")
        payload = data[FRAGMENT_HEADER_LEN : FRAGMENT_HEADER_LEN + payload_len]
        return cls(bytes(short_post_id), frag_index, frag_count, total_len, chunk_size, bytes(payload))

    @classmethod
    def from_base45(cls, text: str) -> "Fragment":
        try:
            raw = base45.b45decode(text)
        except Exception as e:
            raise FragmentError("base45_decode", str(e)) from e
        return cls.from_bytes(raw)


def fragment_post(short_post_id: bytes, serialized_post: bytes, chunk_size: int) -> list[Fragment]:
    if len(short_post_id) != SHORT_ID_LEN:
        raise FragmentError("geometry_invalid", "short id length")
    if chunk_size == 0 or chunk_size > MAX_FRAGMENT_PAYLOAD:
        raise FragmentError("chunk_size_invalid")
    if not serialized_post or len(serialized_post) > MAX_POST_BYTES:
        raise FragmentError("geometry_invalid", "post length")
    count = -(-len(serialized_post) // chunk_size)
    if count > MAX_FRAGMENT_COUNT:
        raise FragmentError("count_out_of_range")
    frags = []
    for i in range(count):
        start = i * chunk_size
        payload = serialized_post[start : start + chunk_size]
        frags.append(
            Fragment(bytes(short_post_id), i, count, len(serialized_post), chunk_size, payload)
        )
    return frags


class Reassembler:
    """Rebuilds a post from fragments arriving in any order.

    Exact duplicates are ignored; conflicting duplicates or any
    disagreement in (short id, count, total length, chunk size) raise
    FragmentError('conflict'). Memory is bounded by MAX_POST_BYTES and a
    fixed fragment bitmap.
    """

    def __init__(self, first: Fragment):
        _validate_geometry(first.frag_count, first.total_len, first.chunk_size)
        self.short_post_id = first.short_post_id
        self.frag_count = first.frag_count
        self.total_len = first.total_len
        self.chunk_size = first.chunk_size
        self._buffer = bytearray(first.total_len)
        self._received = bytearray(MAX_FRAGMENT_COUNT // 8)
        self.received_count = 0
        self.feed(first)

    def feed(self, frag: Fragment) -> str:
        """Feed one fragment; returns 'accepted', 'duplicate', or 'complete'."""
        if (
            frag.short_post_id != self.short_post_id
            or frag.frag_count != self.frag_count
            or frag.total_len != self.total_len
            or frag.chunk_size != self.chunk_size
        ):
            raise FragmentError("conflict", "transfer parameters disagree")
        if frag.frag_index >= self.frag_count:
            raise FragmentError("index_out_of_range")
        expected = _expected_payload_len(
            frag.frag_index, self.frag_count, self.total_len, self.chunk_size
        )
        if len(frag.payload) != expected:
            raise FragmentError("payload_len_invalid")
        idx = frag.frag_index
        start = idx * self.chunk_size
        if self._received[idx // 8] & (1 << (idx % 8)):
            if bytes(self._buffer[start : start + expected]) == frag.payload:
                return "duplicate"
            raise FragmentError("conflict", f"fragment {idx} bytes differ")
        self._buffer[start : start + expected] = frag.payload
        self._received[idx // 8] |= 1 << (idx % 8)
        self.received_count += 1
        return "complete" if self.is_complete else "accepted"

    @property
    def is_complete(self) -> bool:
        return self.received_count == self.frag_count

    def to_bytes(self) -> bytes:
        if not self.is_complete:
            raise FragmentError("incomplete")
        return bytes(self._buffer)
