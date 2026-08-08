"""BG v1 fragments / BG v2 fountain frames and their receivers —
byte-exact mirror of libraries/baogram-core/src/fragment.rs. Both frame
kinds are Base45-encoded for the QR text layer. CRC-32 is ISO-HDLC
(zlib).

v1 carries each source chunk exactly once per loop (index/count). v2 is
a fountain code: frames 0..k-1 are the systematic prefix (chunk i
verbatim), every later frame XORs a SplitMix64-derived set of source
chunks so any sufficiently large subset of frames reconstructs the post
without the sender looping back.
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

FOUNTAIN_VERSION = 2
FOUNTAIN_HEADER_LEN = 26
DEFAULT_FOUNTAIN_PAYLOAD = 96

MAX_PENDING_FRAMES = 512
MAX_PENDING_BYTES = 65_536

_M64 = (1 << 64) - 1


class FragmentError(ValueError):
    def __init__(self, reason: str, detail: str = ""):
        self.reason = reason
        super().__init__(f"{reason}{': ' + detail if detail else ''}")


def _validate_geometry(frag_count: int, total_len: int, chunk_size: int) -> None:
    if frag_count <= 0 or frag_count > MAX_FRAGMENT_COUNT:
        raise FragmentError("count_out_of_range")
    if chunk_size <= 0 or chunk_size > MAX_FRAGMENT_PAYLOAD:
        raise FragmentError("chunk_size_invalid")
    if total_len <= 0 or total_len > MAX_POST_BYTES:
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
    if chunk_size <= 0 or chunk_size > MAX_FRAGMENT_PAYLOAD:
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


class SplitMix64:
    """The standard splitmix64 generator (all arithmetic mod 2**64);
    byte-exact with the Rust side and the published reference outputs."""

    def __init__(self, seed: int):
        self.state = seed & _M64

    def next(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & _M64
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & _M64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & _M64
        return z ^ (z >> 31)


def fountain_indices(short_post_id: bytes, frame_no: int, k: int) -> list[int]:
    """The sorted source-chunk index set XORed into fountain frame
    `frame_no`.

    Frames 0..k-1 are the systematic prefix carrying exactly chunk
    `frame_no` (PRNG unused). Coded frames seed SplitMix64 with the
    short post id (as a big-endian u64) XOR frame_no; the first draw
    picks the degree from weights W(d) = 2**32 // d for d = 1..k
    (smallest d whose cumulative weight exceeds draw % total), further
    draws pick `degree` DISTINCT chunk indices via draw % k, skipping
    repeats. Sorted is the canonical representation (XOR is
    order-independent).
    """
    if frame_no < k:
        return [frame_no]
    rng = SplitMix64(int.from_bytes(short_post_id, "big") ^ frame_no)
    weights = [(1 << 32) // d for d in range(1, k + 1)]
    total = sum(weights)
    r = rng.next() % total
    degree = k
    cum = 0
    for d, w in enumerate(weights, start=1):
        cum += w
        if cum > r:
            degree = d
            break
    chosen: set[int] = set()
    while len(chosen) < degree:
        chosen.add(rng.next() % k)
    return sorted(chosen)


@dataclass
class FountainFrame:
    short_post_id: bytes
    frame_no: int
    k: int
    total_len: int
    chunk_size: int
    payload: bytes

    def to_bytes(self) -> bytes:
        body = (
            FRAGMENT_MAGIC
            + struct.pack(">BB", FOUNTAIN_VERSION, 0)
            + self.short_post_id
            + struct.pack(
                ">IHIHH",
                self.frame_no,
                self.k,
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
    def from_bytes(cls, data: bytes) -> "FountainFrame":
        if len(data) < FOUNTAIN_HEADER_LEN + FRAGMENT_CRC_LEN:
            raise FragmentError("truncated")
        if data[0:2] != FRAGMENT_MAGIC:
            raise FragmentError("bad_magic")
        if data[2] != FOUNTAIN_VERSION:
            raise FragmentError("unknown_version")
        if data[3] != 0:
            raise FragmentError("unknown_flags")
        (stored_crc,) = struct.unpack(">I", data[-FRAGMENT_CRC_LEN:])
        if zlib.crc32(data[:-FRAGMENT_CRC_LEN]) & 0xFFFFFFFF != stored_crc:
            raise FragmentError("crc_mismatch")
        short_post_id = data[4:12]
        frame_no, k, total_len, chunk_size, payload_len = struct.unpack(
            ">IHIHH", data[12:26]
        )
        _validate_geometry(k, total_len, chunk_size)
        if payload_len != chunk_size:
            raise FragmentError("payload_len_invalid")
        expected_total = FOUNTAIN_HEADER_LEN + payload_len + FRAGMENT_CRC_LEN
        if len(data) < expected_total:
            raise FragmentError("truncated")
        if len(data) > expected_total:
            raise FragmentError("trailing_bytes")
        payload = data[FOUNTAIN_HEADER_LEN : FOUNTAIN_HEADER_LEN + payload_len]
        return cls(
            bytes(short_post_id), frame_no, k, total_len, chunk_size, bytes(payload)
        )

    @classmethod
    def from_base45(cls, text: str) -> "FountainFrame":
        try:
            raw = base45.b45decode(text)
        except Exception as e:
            raise FragmentError("base45_decode", str(e)) from e
        return cls.from_bytes(raw)


def _fountain_chunk_count(short_post_id: bytes, serialized_post: bytes, chunk_size: int) -> int:
    """Validate sender geometry (same rules as fragment_post) and return k."""
    if len(short_post_id) != SHORT_ID_LEN:
        raise FragmentError("geometry_invalid", "short id length")
    if chunk_size <= 0 or chunk_size > MAX_FRAGMENT_PAYLOAD:
        raise FragmentError("chunk_size_invalid")
    if not serialized_post or len(serialized_post) > MAX_POST_BYTES:
        raise FragmentError("geometry_invalid", "post length")
    count = -(-len(serialized_post) // chunk_size)
    if count > MAX_FRAGMENT_COUNT:
        raise FragmentError("count_out_of_range")
    return count


def fountain_frame_at(
    short_post_id: bytes, post_bytes: bytes, chunk_size: int, frame_no: int
) -> FountainFrame:
    """Build fountain frame `frame_no`: the XOR of the source chunks named
    by fountain_indices (chunk i = post_bytes[i*cs:(i+1)*cs], the final
    chunk zero-padded to chunk_size)."""
    k = _fountain_chunk_count(short_post_id, post_bytes, chunk_size)
    payload = bytearray(chunk_size)
    for i in fountain_indices(short_post_id, frame_no, k):
        chunk = post_bytes[i * chunk_size : (i + 1) * chunk_size]
        for j, b in enumerate(chunk):
            payload[j] ^= b
    return FountainFrame(
        bytes(short_post_id), frame_no, k, len(post_bytes), chunk_size, bytes(payload)
    )


def fountain_frames(short_post_id: bytes, post_bytes: bytes, chunk_size: int):
    """Yield fountain frames for frame_no 0, 1, 2, ... endlessly."""
    _fountain_chunk_count(short_post_id, post_bytes, chunk_size)
    frame_no = 0
    while True:
        yield fountain_frame_at(short_post_id, post_bytes, chunk_size, frame_no)
        frame_no += 1


class FountainDecoder:
    """Rebuilds a post from fountain frames via peeling.

    Locks (short id, k, total length, chunk size) from the first frame;
    any disagreement raises FragmentError('conflict'). Memory is bounded
    by the validated k * chunk_size chunk buffer, a fixed resolved
    bitmap, and a capped pending store (MAX_PENDING_FRAMES frames /
    MAX_PENDING_BYTES payload bytes; the oldest pending frames are
    evicted to make room).
    """

    def __init__(self, first: FountainFrame):
        _validate_geometry(first.k, first.total_len, first.chunk_size)
        self.short_post_id = first.short_post_id
        self.k = first.k
        self.total_len = first.total_len
        self.chunk_size = first.chunk_size
        self._chunks = bytearray(first.k * first.chunk_size)
        self._resolved = bytearray(MAX_FRAGMENT_COUNT // 8)
        self.received_count = 0  # resolved source chunks
        self._pending: list[tuple[set[int], bytearray]] = []  # arrival order
        self._pending_bytes = 0
        self.feed(first)

    def feed(self, frame: FountainFrame) -> str:
        """Feed one frame; returns 'accepted', 'duplicate', or 'complete'."""
        if (
            frame.short_post_id != self.short_post_id
            or frame.k != self.k
            or frame.total_len != self.total_len
            or frame.chunk_size != self.chunk_size
        ):
            raise FragmentError("conflict", "transfer parameters disagree")
        if len(frame.payload) != self.chunk_size:
            raise FragmentError("payload_len_invalid")
        indices = set(fountain_indices(self.short_post_id, frame.frame_no, self.k))
        payload = bytearray(frame.payload)
        for i in sorted(indices):
            if self._is_resolved(i):
                self._xor_chunk_into(i, payload)
                indices.discard(i)
        if not indices:
            return "duplicate"  # fully redundant with resolved chunks
        if len(indices) == 1:
            self._resolve_and_cascade(indices.pop(), payload)
            return "complete" if self.is_complete else "accepted"
        self._store_pending(indices, payload)
        return "accepted"

    def _is_resolved(self, idx: int) -> bool:
        return bool(self._resolved[idx // 8] & (1 << (idx % 8)))

    def _xor_chunk_into(self, idx: int, payload: bytearray) -> None:
        start = idx * self.chunk_size
        chunk = self._chunks[start : start + self.chunk_size]
        for j in range(self.chunk_size):
            payload[j] ^= chunk[j]

    def _mark_resolved(self, idx: int, payload: bytearray) -> None:
        start = idx * self.chunk_size
        self._chunks[start : start + self.chunk_size] = payload
        self._resolved[idx // 8] |= 1 << (idx % 8)
        self.received_count += 1

    def _resolve_and_cascade(self, idx: int, payload: bytearray) -> None:
        """Resolve one chunk, then peel: XOR it out of every pending frame
        that references it; pending frames reduced to a single unresolved
        index resolve too (worklist until fixpoint); frames reduced to
        empty are dropped as redundant."""
        self._mark_resolved(idx, payload)
        worklist = [idx]
        while worklist:
            c = worklist.pop()
            keep: list[tuple[set[int], bytearray]] = []
            for indices, pend in self._pending:
                if c not in indices:
                    keep.append((indices, pend))
                    continue
                self._xor_chunk_into(c, pend)
                indices.discard(c)
                if len(indices) == 1:
                    j = next(iter(indices))
                    if not self._is_resolved(j):
                        self._mark_resolved(j, pend)
                        worklist.append(j)
                        self._pending_bytes -= len(pend)
                        continue
                    indices.discard(j)
                if not indices:
                    self._pending_bytes -= len(pend)  # redundant: drop
                    continue
                keep.append((indices, pend))
            self._pending = keep

    def _store_pending(self, indices: set[int], payload: bytearray) -> None:
        while self._pending and (
            len(self._pending) >= MAX_PENDING_FRAMES
            or self._pending_bytes + len(payload) > MAX_PENDING_BYTES
        ):
            _, oldest_payload = self._pending.pop(0)
            self._pending_bytes -= len(oldest_payload)
        self._pending.append((indices, payload))
        self._pending_bytes += len(payload)

    @property
    def frag_count(self) -> int:
        return self.k

    @property
    def is_complete(self) -> bool:
        return self.received_count == self.k

    def to_bytes(self) -> bytes:
        if not self.is_complete:
            raise FragmentError("incomplete")
        return bytes(self._chunks[: self.total_len])


def parse_frame(text_or_bytes: str | bytes) -> "Fragment | FountainFrame":
    """Decode one QR payload (Base45 text, or the raw container bytes) and
    dispatch on the version byte: 1 -> Fragment, 2 -> FountainFrame."""
    if isinstance(text_or_bytes, str):
        try:
            raw = base45.b45decode(text_or_bytes)
        except Exception as e:
            raise FragmentError("base45_decode", str(e)) from e
    else:
        raw = bytes(text_or_bytes)
    if len(raw) < 3:
        raise FragmentError("truncated")
    if raw[0:2] != FRAGMENT_MAGIC:
        raise FragmentError("bad_magic")
    if raw[2] == FRAGMENT_VERSION:
        return Fragment.from_bytes(raw)
    if raw[2] == FOUNTAIN_VERSION:
        return FountainFrame.from_bytes(raw)
    raise FragmentError("unknown_version")


class Receiver:
    """Uniform receiver over both protocol versions: wraps a Reassembler
    (v1 fragments) or FountainDecoder (v2 fountain frames) based on the
    first frame. Feeding a frame of the other version raises
    FragmentError('conflict'). received_count/frag_count mirror both
    inner types (for v2 that is resolved chunks / k)."""

    def __init__(self, first: "Fragment | FountainFrame"):
        if isinstance(first, Fragment):
            self.version = FRAGMENT_VERSION
            self._inner: "Reassembler | FountainDecoder" = Reassembler(first)
        elif isinstance(first, FountainFrame):
            self.version = FOUNTAIN_VERSION
            self._inner = FountainDecoder(first)
        else:
            raise FragmentError("unknown_version")

    def feed(self, frame: "Fragment | FountainFrame") -> str:
        expects_v1 = self.version == FRAGMENT_VERSION
        if isinstance(frame, Fragment) != expects_v1:
            raise FragmentError("conflict", "protocol version disagrees")
        return self._inner.feed(frame)

    @property
    def short_post_id(self) -> bytes:
        return self._inner.short_post_id

    @property
    def received_count(self) -> int:
        return self._inner.received_count

    @property
    def frag_count(self) -> int:
        return self._inner.frag_count

    @property
    def is_complete(self) -> bool:
        return self._inner.is_complete

    def to_bytes(self) -> bytes:
        return self._inner.to_bytes()
