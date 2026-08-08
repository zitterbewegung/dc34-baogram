"""Host-local unit tests (no Rust vectors needed)."""

import random

import pytest

from baogram_host import codec, crypto
from baogram_host.format import FormatError, Post
from baogram_host.fragments import (
    Fragment,
    FragmentError,
    FountainDecoder,
    FountainFrame,
    Receiver,
    SplitMix64,
    fountain_frame_at,
    fountain_frames,
    fountain_indices,
    fragment_post,
    parse_frame,
)
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


# ---------------------------------------------------------------------------
# RowDeltaMono1 (codec 2)
# ---------------------------------------------------------------------------


def _prand(n: int, seed: int) -> bytes:
    """Deterministic pseudo-random bytes from SplitMix64 (stable across
    runs and platforms)."""
    rng = SplitMix64(seed)
    out = bytearray()
    while len(out) < n:
        out.extend(rng.next().to_bytes(8, "big"))
    return bytes(out[:n])


def test_rowdelta_filter_roundtrip_patterns():
    patterns = (
        b"\x00" * (32 * 240),                      # all-zero
        b"\xff" * (32 * 240),                      # all-ff
        (b"\xaa" * 32 + b"\x55" * 32) * 120,       # alternating rows
        b"\xaa" * (32 * 240),                      # alternating columns
        _prand(32 * 240, 0xC0DEC),                 # deterministic noise
    )
    for p in patterns:
        f = codec.rowdelta_filter(p)
        assert len(f) == len(p)
        assert f[:32] == p[:32]
        assert codec.rowdelta_unfilter(f) == p
    # filter semantics: alternating aa/55 rows filter to 0xff after row 0
    f = codec.rowdelta_filter((b"\xaa" * 32 + b"\x55" * 32) * 120)
    assert f[:32] == b"\xaa" * 32
    assert f[32:] == b"\xff" * (32 * 239)
    with pytest.raises(codec.CodecError):
        codec.rowdelta_filter(b"\x00" * 33)  # not a multiple of the stride


def test_rowdelta_vertically_constant_image_picks_codec2():
    # a noise row repeated 240x: horizontally incompressible (PackBits
    # loses), vertically constant (RowDelta filters rows 1.. to zero)
    packed = _prand(32, 7) * 240
    codec_id, enc = codec.encode_best(packed)
    assert codec_id == codec.CODEC_ROWDELTA_MONO1
    assert len(enc) < len(codec.packbits_encode(packed))
    assert codec.decode(codec_id, enc, len(packed)) == packed
    # ...and through the full Post container
    ident = crypto.Identity(SEED)
    post = Post.create(ident, 3, "bob", "vertical", packed)
    assert post.codec == codec.CODEC_ROWDELTA_MONO1
    parsed = Post.parse(post.serialize())
    assert parsed.codec == codec.CODEC_ROWDELTA_MONO1
    assert parsed.decode_image() == packed


def test_rowdelta_tie_prefers_packbits():
    # rows each solid with a per-row varying byte: PackBits and RowDelta
    # both encode to one run per row -> exact tie -> codec 1 wins
    packed = b"".join(bytes([y]) * 32 for y in range(240))
    codec_id, enc = codec.encode_best(packed)
    assert codec_id == codec.CODEC_PACKBITS_MONO1
    assert len(enc) == len(codec.packbits_encode(codec.rowdelta_filter(packed)))


def test_rowdelta_decode_canonicality():
    # not strictly smaller than raw
    with pytest.raises(codec.CodecError):
        codec.decode(codec.CODEC_ROWDELTA_MONO1, b"\x00" * 64, 64)
    with pytest.raises(codec.CodecError):
        codec.decode(codec.CODEC_ROWDELTA_MONO1, b"\x00" * 65, 64)
    # expected_len not a multiple of the 32-byte row stride
    with pytest.raises(codec.CodecError):
        codec.decode(codec.CODEC_ROWDELTA_MONO1, b"\xe0\x00", 33)


# ---------------------------------------------------------------------------
# BG v2 fountain frames
# ---------------------------------------------------------------------------

FOUNTAIN_SID = bytes(range(1, 9))  # 01..08


def test_splitmix64_reference_vectors():
    # standard splitmix64 reference outputs for seed 0
    rng = SplitMix64(0)
    assert rng.next() == 0xE220A8397B1DCDAF
    assert rng.next() == 0x6E789E6AA1B965F4
    assert rng.next() == 0x06C45D188009454F


def test_fountain_indices_pins():
    k = 40
    # systematic prefix: PRNG unused
    assert fountain_indices(FOUNTAIN_SID, 0, k) == [0]
    assert fountain_indices(FOUNTAIN_SID, 39, k) == [39]
    # regression pins locking cross-language determinism with the Rust side
    assert fountain_indices(FOUNTAIN_SID, 40, k) == [
        0, 1, 6, 8, 12, 18, 23, 24, 25, 31, 32, 33, 34, 37, 38,
    ]
    assert fountain_indices(FOUNTAIN_SID, 41, k) == [28]
    assert fountain_indices(FOUNTAIN_SID, 42, k) == [
        1, 2, 5, 6, 7, 10, 11, 12, 13, 17, 20, 21, 22, 24, 25,
        26, 27, 29, 30, 31, 32, 34, 35, 36, 37,
    ]


def test_fountain_frame_wire_roundtrip_and_rejections():
    data = _prand(100, 1)
    frame = fountain_frame_at(FOUNTAIN_SID, data, 32, 0)
    assert frame.k == 4 and frame.total_len == 100 and len(frame.payload) == 32
    wire = frame.to_bytes()
    assert len(wire) == 26 + 32 + 4
    assert FountainFrame.from_bytes(wire) == frame
    assert FountainFrame.from_base45(frame.to_base45()) == frame
    # nonzero flags rejected
    bad = bytearray(wire)
    bad[3] = 1
    bad[-4:] = __import__("struct").pack(">I", __import__("zlib").crc32(bytes(bad[:-4])))
    with pytest.raises(FragmentError) as e:
        FountainFrame.from_bytes(bytes(bad))
    assert e.value.reason == "unknown_flags"
    # corrupted CRC rejected
    bad = bytearray(wire)
    bad[-1] ^= 1
    with pytest.raises(FragmentError) as e:
        FountainFrame.from_bytes(bytes(bad))
    assert e.value.reason == "crc_mismatch"
    # payload_len must ALWAYS equal chunk_size
    short = FountainFrame(FOUNTAIN_SID, 0, 4, 100, 32, b"\x00" * 16)
    with pytest.raises(FragmentError) as e:
        FountainFrame.from_bytes(short.to_bytes())
    assert e.value.reason == "payload_len_invalid"
    # inconsistent geometry (k != ceil(total/chunk))
    geom = FountainFrame(FOUNTAIN_SID, 0, 5, 100, 32, b"\x00" * 32)
    with pytest.raises(FragmentError) as e:
        FountainFrame.from_bytes(geom.to_bytes())
    assert e.value.reason == "geometry_invalid"


def _feed_all(frames):
    decoder = None
    statuses = []
    for f in frames:
        if decoder is None:
            decoder = FountainDecoder(f)
        else:
            statuses.append(decoder.feed(f))
    return decoder, statuses


def test_fountain_systematic_completes_in_exactly_k_frames():
    data = _prand(5400, 0xBA0BA0)
    cs = 96
    k = -(-len(data) // cs)
    assert k == 57
    gen = fountain_frames(FOUNTAIN_SID, data, cs)
    decoder, statuses = _feed_all(next(gen) for _ in range(k))
    assert statuses[:-1] == ["accepted"] * (k - 2)
    assert statuses[-1] == "complete"
    assert decoder.is_complete
    assert decoder.received_count == decoder.frag_count == k
    assert decoder.short_post_id == FOUNTAIN_SID
    assert decoder.to_bytes() == data


def test_fountain_shuffled_frames_reconstruct():
    data = _prand(5400, 0xBA0BA0)
    cs = 96
    k = -(-len(data) // cs)
    order = list(range(2 * k))
    random.Random(42).shuffle(order)
    decoder = None
    for n in order:
        f = fountain_frame_at(FOUNTAIN_SID, data, cs, n)
        if decoder is None:
            decoder = FountainDecoder(f)
        else:
            decoder.feed(f)
        if decoder.is_complete:
            break
    assert decoder.is_complete
    assert decoder.to_bytes() == data


def test_fountain_50_percent_loss_reconstructs():
    # Drop every other frame and decode purely from coded frames k..4k
    # (the whole systematic prefix is missed). Whether pure peeling
    # finishes inside this window depends on the PRNG stream, i.e. on the
    # short post id; this deterministic id completes at frame_no 211.
    sid = bytes([0x8F] * 8)
    data = _prand(5400, 0xBA0BA0)
    cs = 96
    k = -(-len(data) // cs)
    decoder = None
    for n in range(k, 4 * k, 2):
        f = fountain_frame_at(sid, data, cs, n)
        if decoder is None:
            decoder = FountainDecoder(f)
        else:
            decoder.feed(f)
        if decoder.is_complete:
            break
    assert decoder.is_complete
    assert decoder.to_bytes() == data


def test_fountain_duplicate_heavy_reconstructs():
    data = _prand(5400, 0xBA0BA0)
    cs = 96
    k = -(-len(data) // cs)
    decoder = None
    for n in range(k):
        f = fountain_frame_at(FOUNTAIN_SID, data, cs, n)
        if decoder is None:
            decoder = FountainDecoder(f)
        else:
            decoder.feed(f)
        # an exact repeat of a resolved frame is a duplicate, not an error
        assert decoder.feed(f) == "duplicate"
    assert decoder.is_complete
    # coded frames after completion reduce to nothing: duplicates too
    assert decoder.feed(fountain_frame_at(FOUNTAIN_SID, data, cs, 5 * k)) == "duplicate"
    assert decoder.to_bytes() == data


def test_fountain_peeling_cascade():
    data = _prand(100, 7)
    cs = 32
    k = -(-len(data) // cs)
    assert k == 4
    # the first coded frame for this id happens to have degree 2 over
    # chunks {0, 1}: fed first, it must sit pending...
    assert fountain_indices(FOUNTAIN_SID, 4, k) == [0, 1]
    decoder = FountainDecoder(fountain_frame_at(FOUNTAIN_SID, data, cs, 4))
    assert decoder.received_count == 0
    # ...until systematic chunk 0 arrives: resolving it peels chunk 1 out
    # of the pending frame (cascade resolves two chunks at once)
    assert decoder.feed(fountain_frame_at(FOUNTAIN_SID, data, cs, 0)) == "accepted"
    assert decoder.received_count == 2
    assert decoder.feed(fountain_frame_at(FOUNTAIN_SID, data, cs, 2)) == "accepted"
    assert decoder.feed(fountain_frame_at(FOUNTAIN_SID, data, cs, 3)) == "complete"
    assert decoder.to_bytes() == data


def test_fountain_conflicts():
    data = _prand(200, 9)
    decoder = FountainDecoder(fountain_frame_at(FOUNTAIN_SID, data, 64, 0))
    # disagreeing chunk size (and therefore k)
    with pytest.raises(FragmentError) as e:
        decoder.feed(fountain_frame_at(FOUNTAIN_SID, data, 32, 0))
    assert e.value.reason == "conflict"
    # disagreeing total length
    with pytest.raises(FragmentError) as e:
        decoder.feed(fountain_frame_at(FOUNTAIN_SID, data + b"x", 64, 0))
    assert e.value.reason == "conflict"
    # disagreeing short post id
    with pytest.raises(FragmentError) as e:
        decoder.feed(fountain_frame_at(b"\x02" * 8, data, 64, 0))
    assert e.value.reason == "conflict"


def test_receiver_rejects_cross_version_frames():
    data = _prand(200, 9)
    frag = fragment_post(FOUNTAIN_SID, data, 64)[0]
    frame = fountain_frame_at(FOUNTAIN_SID, data, 64, 0)
    # v1 receiver fed a v2 frame with the same short id
    r = Receiver(frag)
    with pytest.raises(FragmentError) as e:
        r.feed(frame)
    assert e.value.reason == "conflict"
    # v2 receiver fed a v1 fragment with the same short id
    r = Receiver(frame)
    with pytest.raises(FragmentError) as e:
        r.feed(frag)
    assert e.value.reason == "conflict"
    # uniform progress surface
    assert r.short_post_id == FOUNTAIN_SID
    assert (r.received_count, r.frag_count) == (1, 4)


def test_receiver_completes_both_versions():
    data = _prand(300, 11)
    # v1
    r = Receiver(fragment_post(FOUNTAIN_SID, data, 64)[0])
    for frag in fragment_post(FOUNTAIN_SID, data, 64)[1:]:
        r.feed(frag)
    assert r.is_complete and r.to_bytes() == data
    # v2
    gen = fountain_frames(FOUNTAIN_SID, data, 64)
    r = Receiver(next(gen))
    while not r.is_complete:
        r.feed(next(gen))
    assert r.to_bytes() == data


def test_parse_frame_dispatch():
    data = _prand(100, 13)
    frag = fragment_post(FOUNTAIN_SID, data, 64)[0]
    frame = fountain_frame_at(FOUNTAIN_SID, data, 64, 0)
    got = parse_frame(frag.to_base45())
    assert isinstance(got, Fragment) and got == frag
    got = parse_frame(frame.to_base45())
    assert isinstance(got, FountainFrame) and got == frame
    # raw container bytes are accepted too
    assert parse_frame(frame.to_bytes()) == frame
    # any other version byte is rejected
    other = bytearray(frame.to_bytes())
    other[2] = 3
    with pytest.raises(FragmentError) as e:
        parse_frame(bytes(other))
    assert e.value.reason == "unknown_version"
    with pytest.raises(FragmentError) as e:
        parse_frame("not base45 %%%")
    assert e.value.reason == "base45_decode"
