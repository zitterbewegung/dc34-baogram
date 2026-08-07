"""Host-local unit tests (no Rust vectors needed)."""

import pytest

from baogram_host import codec, crypto
from baogram_host.format import FormatError, Post
from baogram_host.fragments import Fragment, FragmentError, fragment_post
from baogram_host.image import (
    IMAGE_HEIGHT,
    IMAGE_WIDTH,
    MONO1_PACKED_LEN,
    quantize,
    save_mono1_as_png,
    synthetic_test_frame,
    unpack_to_gray,
)

SEED = b"x" * 32


def test_synthetic_frame_sha():
    import hashlib

    assert (
        hashlib.sha256(synthetic_test_frame()).hexdigest()
        == "c82bfcf7e7ee582e204c151af00570cb832dcddced95df56f484d1cc7d0303db"
    )


def test_packbits_roundtrip_patterns():
    for pattern in (
        b"\xff" * MONO1_PACKED_LEN,
        b"\x00" * MONO1_PACKED_LEN,
        (b"\xaa" * 32 + b"\x55" * 32) * 120,
        bytes(range(256)) * 30,
    ):
        enc = codec.packbits_encode(pattern)
        assert codec.packbits_decode(enc, len(pattern)) == pattern


def test_packbits_rejections():
    with pytest.raises(codec.CodecError):
        codec.packbits_decode(b"\x80\x00", 1)  # reserved control
    with pytest.raises(codec.CodecError):
        codec.packbits_decode(b"\x03\x01\x02", 4)  # truncated literal
    with pytest.raises(codec.CodecError):
        codec.packbits_decode(b"\x81\x05", 10)  # overflow (128 into 10)
    with pytest.raises(codec.CodecError):
        codec.packbits_decode(b"\xfe\x07\x00\x01", 3)  # trailing garbage


def test_post_roundtrip_and_tamper():
    ident = crypto.Identity(SEED)
    packed = quantize(synthetic_test_frame())
    post = Post.create(ident, 1, "alice", "hello", packed)
    data = post.serialize()
    parsed = Post.parse(data)
    assert parsed.decode_image() == packed
    # tamper caption
    bad = bytearray(data)
    bad[62 + len("alice")] ^= 1
    with pytest.raises(FormatError) as e:
        Post.parse(bytes(bad))
    assert e.value.reason == "digest_mismatch"
    # tamper signature
    bad = bytearray(data)
    bad[-1] ^= 1
    with pytest.raises(FormatError) as e:
        Post.parse(bytes(bad))
    assert e.value.reason == "invalid_signature"


def test_handle_caption_bounds():
    ident = crypto.Identity(SEED)
    packed = quantize(synthetic_test_frame())
    Post.create(ident, 0, "x" * 24, "y" * 64, packed)
    with pytest.raises(FormatError):
        Post.create(ident, 0, "x" * 25, "", packed)
    with pytest.raises(FormatError):
        Post.create(ident, 0, "", "y" * 65, packed)


def test_fragment_geometry_rejections():
    frags = fragment_post(b"\x01" * 8, b"z" * 100, 64)
    assert len(frags) == 2
    # inconsistent count
    f = Fragment(b"\x01" * 8, 0, 5, 100, 64, b"z" * 64)
    with pytest.raises(FragmentError) as e:
        Fragment.from_bytes(f.to_bytes())
    assert e.value.reason == "geometry_invalid"
    # index out of range
    f = Fragment(b"\x01" * 8, 2, 2, 100, 64, b"z" * 36)
    with pytest.raises(FragmentError) as e:
        Fragment.from_bytes(f.to_bytes())
    assert e.value.reason == "index_out_of_range"


def test_png_export_reimport_is_lossless(tmp_path):
    packed = quantize(synthetic_test_frame())
    png = tmp_path / "out.png"
    save_mono1_as_png(packed, str(png))
    from PIL import Image

    img = Image.open(png).convert("L")
    assert img.size == (IMAGE_WIDTH, IMAGE_HEIGHT)
    # 255/0 pixels re-quantize (any threshold between 0 and 254) to the
    # identical packed image: full spatial resolution is preserved
    assert quantize(img.tobytes(), 127) == packed
    assert unpack_to_gray(packed) == img.tobytes()
