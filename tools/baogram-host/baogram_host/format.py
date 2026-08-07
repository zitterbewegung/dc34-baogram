"""Canonical Baogram post container (BGRM v1) — byte-exact mirror of
libraries/baogram-core/src/post.rs. See BAOGRAM_PROTOCOL.md for the
normative layout. All integers big-endian; exactly one valid
serialization per post.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

from . import codec, crypto
from .image import IMAGE_HEIGHT, IMAGE_WIDTH, MONO1_PACKED_LEN

POST_MAGIC = b"BGRM"
POST_VERSION = 1
PIXEL_FORMAT_MONO1 = 1

HANDLE_MAX_BYTES = 24
CAPTION_MAX_BYTES = 64
ENCODED_IMAGE_MAX_BYTES = 61_440
MAX_POST_BYTES = 65_536
CANONICAL_HEADER_LEN = 62
TRAILER_LEN = crypto.DIGEST_LEN + crypto.SIGNATURE_LEN  # 96


class FormatError(ValueError):
    """Post failed to parse or verify. `reason` is a stable identifier."""

    def __init__(self, reason: str, detail: str = ""):
        self.reason = reason
        super().__init__(f"{reason}{': ' + detail if detail else ''}")


def _canonical_header(
    codec_id: int,
    seq: int,
    author_pubkey: bytes,
    handle_len: int,
    caption_len: int,
    encoded_len: int,
) -> bytes:
    return (
        POST_MAGIC
        + struct.pack(
            ">BBBBHH",
            POST_VERSION,
            PIXEL_FORMAT_MONO1,
            codec_id,
            0,  # flags
            IMAGE_WIDTH,
            IMAGE_HEIGHT,
        )
        + struct.pack(">Q", seq)
        + author_pubkey
        + struct.pack(">BB", handle_len, caption_len)
        + struct.pack(">II", MONO1_PACKED_LEN, encoded_len)
    )


@dataclass
class Post:
    codec: int
    seq: int
    author_pubkey: bytes
    handle: str
    caption: str
    encoded_image: bytes
    digest: bytes
    signature: bytes

    @classmethod
    def create(
        cls,
        identity: crypto.Identity,
        seq: int,
        handle: str,
        caption: str,
        packed_image: bytes,
    ) -> "Post":
        if len(handle.encode()) > HANDLE_MAX_BYTES:
            raise FormatError("field_too_long", "handle > 24 bytes")
        if len(caption.encode()) > CAPTION_MAX_BYTES:
            raise FormatError("field_too_long", "caption > 64 bytes")
        if len(packed_image) != MONO1_PACKED_LEN:
            raise FormatError("bad_packed_size")
        codec_id, encoded = codec.encode_best(packed_image)
        pub = identity.public_key
        header = _canonical_header(
            codec_id, seq, pub, len(handle.encode()), len(caption.encode()), len(encoded)
        )
        digest = crypto.post_digest(header, handle.encode(), caption.encode(), encoded)
        signature = identity.sign_digest(digest)
        return cls(codec_id, seq, pub, handle, caption, encoded, digest, signature)

    def serialize(self) -> bytes:
        header = _canonical_header(
            self.codec,
            self.seq,
            self.author_pubkey,
            len(self.handle.encode()),
            len(self.caption.encode()),
            len(self.encoded_image),
        )
        return (
            header
            + self.handle.encode()
            + self.caption.encode()
            + self.encoded_image
            + self.digest
            + self.signature
        )

    @classmethod
    def parse(cls, data: bytes) -> "Post":
        """Parse and fully verify. All lengths are validated before use;
        raises FormatError with a stable `reason` on any violation."""
        if len(data) > MAX_POST_BYTES:
            raise FormatError("post_too_large")
        if len(data) < CANONICAL_HEADER_LEN + TRAILER_LEN:
            raise FormatError("truncated")
        if data[0:4] != POST_MAGIC:
            raise FormatError("bad_magic")
        if data[4] != POST_VERSION:
            raise FormatError("unknown_version")
        if data[5] != PIXEL_FORMAT_MONO1:
            raise FormatError("unknown_pixel_format")
        codec_id = data[6]
        if codec_id not in (codec.CODEC_RAW_MONO1, codec.CODEC_PACKBITS_MONO1):
            raise FormatError("unknown_codec")
        if data[7] != 0:
            raise FormatError("unknown_flags")
        width, height = struct.unpack(">HH", data[8:12])
        if width != IMAGE_WIDTH or height != IMAGE_HEIGHT:
            raise FormatError("unsupported_dimensions")
        (seq,) = struct.unpack(">Q", data[12:20])
        author_pubkey = data[20:52]
        handle_len = data[52]
        caption_len = data[53]
        if handle_len > HANDLE_MAX_BYTES or caption_len > CAPTION_MAX_BYTES:
            raise FormatError("field_too_long")
        unc_len, enc_len = struct.unpack(">II", data[54:62])
        if unc_len != MONO1_PACKED_LEN:
            raise FormatError("noncanonical_length", "uncompressed length")
        if enc_len > ENCODED_IMAGE_MAX_BYTES:
            raise FormatError("noncanonical_length", "encoded length")
        expected_total = CANONICAL_HEADER_LEN + handle_len + caption_len + enc_len + TRAILER_LEN
        if len(data) < expected_total:
            raise FormatError("truncated")
        if len(data) > expected_total:
            raise FormatError("trailing_bytes")
        off = CANONICAL_HEADER_LEN
        handle_bytes = data[off : off + handle_len]
        off += handle_len
        caption_bytes = data[off : off + caption_len]
        off += caption_len
        encoded_image = data[off : off + enc_len]
        off += enc_len
        digest = data[off : off + crypto.DIGEST_LEN]
        off += crypto.DIGEST_LEN
        signature = data[off : off + crypto.SIGNATURE_LEN]

        try:
            handle = handle_bytes.decode("utf-8")
            caption = caption_bytes.decode("utf-8")
        except UnicodeDecodeError as e:
            raise FormatError("invalid_utf8") from e

        header = _canonical_header(
            codec_id, seq, author_pubkey, handle_len, caption_len, enc_len
        )
        computed = crypto.post_digest(header, handle_bytes, caption_bytes, encoded_image)
        if computed != digest:
            raise FormatError("digest_mismatch")
        if not crypto.verify_digest_signature(author_pubkey, digest, signature):
            raise FormatError("invalid_signature")
        # image must decode to exactly the packed length (also enforces the
        # compressed-strictly-smaller canonical rule)
        try:
            codec.decode(codec_id, encoded_image, MONO1_PACKED_LEN)
        except codec.CodecError as e:
            raise FormatError("codec_error", str(e)) from e

        return cls(
            codec_id, seq, bytes(author_pubkey), handle, caption,
            bytes(encoded_image), bytes(digest), bytes(signature),
        )

    def decode_image(self) -> bytes:
        return codec.decode(self.codec, self.encoded_image, MONO1_PACKED_LEN)

    @property
    def post_id(self) -> bytes:
        return self.digest[: crypto.POST_ID_LEN]

    @property
    def short_id(self) -> bytes:
        return self.digest[: crypto.SHORT_ID_LEN]
