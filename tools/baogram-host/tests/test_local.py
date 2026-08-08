"""Host-local unit tests (no Rust vectors needed)."""

import random

import pytest

from baogram_host import codec, crypto
from baogram_host.format import (
    FormatError,
    PIXEL_FORMAT_MONO1,
    PIXEL_FORMAT_MONO1_SMALL,
    Post,
)
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
    IMAGE_PIXELS,
    IMAGE_WIDTH,
    MONO1_PACKED_LEN,
    MONO1_ROW_BYTES,
    SMALL_HEIGHT,
    SMALL_PACKED_LEN,
    SMALL_ROW_BYTES,
    SMALL_WIDTH,
    ImageError,
    despeckle_small,
    quantize,
    save_mono1_as_png,
    small_from_gray,
    small_to_full,
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
        f = codec.rowdelta_filter(p, 32)
        assert len(f) == len(p)
        assert f[:32] == p[:32]
        assert codec.rowdelta_unfilter(f, 32) == p
    # filter semantics: alternating aa/55 rows filter to 0xff after row 0
    f = codec.rowdelta_filter((b"\xaa" * 32 + b"\x55" * 32) * 120, 32)
    assert f[:32] == b"\xaa" * 32
    assert f[32:] == b"\xff" * (32 * 239)
    # the small-format 16-byte stride filters at the small row boundary
    f = codec.rowdelta_filter((b"\xaa" * 16 + b"\x55" * 16) * 120, 16)
    assert f[:16] == b"\xaa" * 16
    assert f[16:] == b"\xff" * (16 * 239)
    assert codec.rowdelta_unfilter(f, 16) == (b"\xaa" * 16 + b"\x55" * 16) * 120
    with pytest.raises(codec.CodecError):
        codec.rowdelta_filter(b"\x00" * 33, 32)  # not a multiple of the stride


def test_rowdelta_vertically_constant_image_picks_codec2():
    # a noise row repeated 240x: horizontally incompressible (PackBits
    # loses), vertically constant (RowDelta filters rows 1.. to zero)
    packed = _prand(32, 7) * 240
    codec_id, enc = codec.encode_best(packed, IMAGE_WIDTH, IMAGE_HEIGHT)
    assert codec_id == codec.CODEC_ROWDELTA_MONO1
    assert len(enc) < len(codec.packbits_encode(packed))
    assert codec.decode(codec_id, enc, len(packed), 32) == packed
    # ...and through the full Post container
    ident = crypto.Identity(SEED)
    post = Post.create(ident, 3, "bob", "vertical", packed)
    assert post.codec == codec.CODEC_ROWDELTA_MONO1
    parsed = Post.parse(post.serialize())
    assert parsed.codec == codec.CODEC_ROWDELTA_MONO1
    assert parsed.decode_image() == packed


def test_rowdelta_tie_prefers_packbits():
    # rows each solid with a per-row varying byte: PackBits and RowDelta
    # both encode to one run per row -> exact tie -> the LOWEST codec id
    # (1) wins; the ctx codec loses outright on this pattern
    packed = b"".join(bytes([y]) * 32 for y in range(240))
    codec_id, enc = codec.encode_best(packed, IMAGE_WIDTH, IMAGE_HEIGHT)
    assert codec_id == codec.CODEC_PACKBITS_MONO1
    assert len(enc) == len(codec.packbits_encode(codec.rowdelta_filter(packed, 32)))


def test_rowdelta_decode_canonicality():
    # not strictly smaller than raw
    with pytest.raises(codec.CodecError):
        codec.decode(codec.CODEC_ROWDELTA_MONO1, b"\x00" * 64, 64, 32)
    with pytest.raises(codec.CodecError):
        codec.decode(codec.CODEC_ROWDELTA_MONO1, b"\x00" * 65, 64, 32)
    # expected_len not a multiple of the 32-byte row stride
    with pytest.raises(codec.CodecError):
        codec.decode(codec.CODEC_ROWDELTA_MONO1, b"\xe0\x00", 33, 32)


def test_decode_row_stride_rejections():
    # the stride checks run up front for EVERY codec id, valid or not
    for cid in (0, 1, 2, 3, 9):
        with pytest.raises(codec.CodecError):
            codec.decode(cid, b"\x00" * 10, 64, 0)  # zero row stride
        with pytest.raises(codec.CodecError):
            codec.decode(cid, b"\x00" * 10, 33, 32)  # not whole rows
    # a valid stride still enforces the per-codec rules
    assert codec.decode(codec.CODEC_RAW_MONO1, b"\x00" * 64, 64, 32) == b"\x00" * 64
    with pytest.raises(codec.CodecError):
        codec.decode(codec.CODEC_RAW_MONO1, b"\x00" * 63, 64, 32)
    with pytest.raises(codec.CodecError):
        codec.decode(9, b"", 32, 32)  # unknown codec


# ---------------------------------------------------------------------------
# CtxArithMono1 (codec 3) inside encode_best / decode
# ---------------------------------------------------------------------------


def _small_pattern(fn) -> bytes:
    out = bytearray(SMALL_PACKED_LEN)
    for y in range(SMALL_HEIGHT):
        for x in range(SMALL_WIDTH):
            if fn(x, y):
                out[y * SMALL_ROW_BYTES + (x >> 3)] |= 0x80 >> (x & 7)
    return bytes(out)


def test_encode_best_three_way_ctx_wins():
    # a filled disc has 2D structure the 1D row codecs cannot exploit:
    # the context-modeled codec must win the three-way pick
    disc = _small_pattern(lambda x, y: (x - 64) ** 2 + (y - 60) ** 2 < 50 ** 2)
    codec_id, enc = codec.encode_best(disc, SMALL_WIDTH, SMALL_HEIGHT)
    assert codec_id == codec.CODEC_CTXARITH_MONO1
    assert len(enc) < len(codec.packbits_encode(disc))
    assert len(enc) < len(codec.packbits_encode(codec.rowdelta_filter(disc, 16)))
    assert codec.decode(codec_id, enc, SMALL_PACKED_LEN, 16) == disc
    # ...and through the full Post container
    ident = crypto.Identity(SEED)
    post = Post.create_small(ident, 4, "ctx", "disc", disc)
    assert post.codec == codec.CODEC_CTXARITH_MONO1
    parsed = Post.parse(post.serialize())
    assert parsed.codec == codec.CODEC_CTXARITH_MONO1
    assert parsed.decode_image() == small_to_full(disc)


def test_ctx_decode_canonicality():
    # encoded length not strictly smaller than raw is rejected for codec 3
    with pytest.raises(codec.CodecError):
        codec.decode(codec.CODEC_CTXARITH_MONO1, b"\x00" * SMALL_PACKED_LEN, SMALL_PACKED_LEN, 16)


# ---------------------------------------------------------------------------
# Small format (pixel format 2): quantize / despeckle / pixel-double
# ---------------------------------------------------------------------------

# SHA-256 of despeckle_small(small_from_gray(synthetic_test_frame())) — the
# canonical small capture pipeline output; the identical constant is pinned
# in the Rust test suite (cross-language determinism lock).
DESPECKLED_SMALL_SHA256 = "a81929e4959f2141a03048f55d29f91c387e6e680a1f9a3222776c42d5d72cac"


def _small_get(packed: bytes, x: int, y: int) -> bool:
    return bool(packed[y * SMALL_ROW_BYTES + (x >> 3)] & (0x80 >> (x & 7)))


def _full_get(packed: bytes, x: int, y: int) -> bool:
    return bool(packed[y * MONO1_ROW_BYTES + (x >> 3)] & (0x80 >> (x & 7)))


def test_small_from_gray_box_average_and_threshold():
    # uniform frames: mean threshold t equals the pixel value, and v > t
    # is false -> all black (the documented deterministic edge behavior)
    assert small_from_gray(b"\x00" * IMAGE_PIXELS) == b"\x00" * SMALL_PACKED_LEN
    assert small_from_gray(b"\xff" * IMAGE_PIXELS) == b"\x00" * SMALL_PACKED_LEN
    # with an explicit low threshold the all-white frame stays all white
    assert small_from_gray(b"\xff" * IMAGE_PIXELS, 0) == b"\xff" * SMALL_PACKED_LEN
    # 2x2 integer box average: block values 10,20,30,45 average to
    # floor(105/4) = 26 -> white iff threshold < 26
    gray = bytearray(IMAGE_PIXELS)
    gray[0], gray[1] = 10, 20
    gray[IMAGE_WIDTH], gray[IMAGE_WIDTH + 1] = 30, 45
    assert _small_get(small_from_gray(bytes(gray), 25), 0, 0)
    assert not _small_get(small_from_gray(bytes(gray), 26), 0, 0)
    with pytest.raises(ImageError):
        small_from_gray(b"\x00" * 100)


def test_despeckle_small_majority_and_border_clamp():
    solid = b"\xff" * SMALL_PACKED_LEN
    assert despeckle_small(solid) == solid
    assert despeckle_small(b"\x00" * SMALL_PACKED_LEN) == b"\x00" * SMALL_PACKED_LEN
    # a single isolated white pixel (1 of 9) is removed
    lone = _small_pattern(lambda x, y: (x, y) == (60, 60))
    assert despeckle_small(lone) == b"\x00" * SMALL_PACKED_LEN
    # a single black hole in white (8 of 9 white) is filled
    hole = _small_pattern(lambda x, y: (x, y) != (60, 60))
    assert despeckle_small(hole) == solid
    # border clamping: at the (0,0) corner the clamped 3x3 samples are
    # (0,0)x4, (1,0)x2, (0,1)x2, (1,1)x1 — a lone corner pixel counts 4
    # of 9 and is removed; corner + both edge neighbors count 8 and stay
    corner = _small_pattern(lambda x, y: (x, y) == (0, 0))
    assert despeckle_small(corner) == b"\x00" * SMALL_PACKED_LEN
    l_shape = _small_pattern(lambda x, y: (x, y) in ((0, 0), (1, 0), (0, 1)))
    assert _small_get(despeckle_small(l_shape), 0, 0)
    with pytest.raises(ImageError):
        despeckle_small(b"\x00" * (SMALL_PACKED_LEN - 1))


def test_small_capture_pipeline_is_deterministic():
    import hashlib

    small = small_from_gray(synthetic_test_frame())
    assert len(small) == SMALL_PACKED_LEN
    assert small_from_gray(synthetic_test_frame()) == small
    despeckled = despeckle_small(small)
    digest = hashlib.sha256(despeckled).hexdigest()
    print(f"despeckled small SHA-256: {digest}")
    assert digest == DESPECKLED_SMALL_SHA256
    assert despeckle_small(small) == despeckled


def test_small_to_full_pixel_doubling():
    small = despeckle_small(small_from_gray(synthetic_test_frame()))
    full = small_to_full(small)
    assert len(full) == MONO1_PACKED_LEN
    # every small pixel becomes a 2x2 block, exactly
    for y in range(SMALL_HEIGHT):
        for x in range(SMALL_WIDTH):
            v = _small_get(small, x, y)
            assert _full_get(full, 2 * x, 2 * y) == v
            assert _full_get(full, 2 * x + 1, 2 * y) == v
            assert _full_get(full, 2 * x, 2 * y + 1) == v
            assert _full_get(full, 2 * x + 1, 2 * y + 1) == v
    # spot-check the packing: a lone white small pixel at (0,0) doubles
    # to 0xC0 bytes at the start of full rows 0 and 1
    lone = _small_pattern(lambda x, y: (x, y) == (0, 0))
    doubled = small_to_full(lone)
    assert doubled[0] == 0xC0
    assert doubled[MONO1_ROW_BYTES] == 0xC0
    assert sum(doubled) == 2 * 0xC0
    with pytest.raises(ImageError):
        small_to_full(b"\x00" * MONO1_PACKED_LEN)


def test_small_post_roundtrip():
    ident = crypto.Identity(SEED)
    small = despeckle_small(small_from_gray(synthetic_test_frame()))
    post = Post.create_small(ident, 9, "smol", "compact post", small)
    assert post.pixel_format == PIXEL_FORMAT_MONO1_SMALL
    data = post.serialize()
    parsed = Post.parse(data)
    assert parsed == post
    # decode_image pixel-doubles small posts to the full 256x240 format
    assert parsed.decode_image() == small_to_full(small)
    # a small post is drastically smaller than the full-format equivalent
    full_post = Post.create(ident, 9, "smol", "compact post", quantize(synthetic_test_frame()))
    assert full_post.pixel_format == PIXEL_FORMAT_MONO1
    assert len(data) < len(full_post.serialize())


def test_small_post_dimension_mismatch_rejected():
    ident = crypto.Identity(SEED)
    small = despeckle_small(small_from_gray(synthetic_test_frame()))
    post = Post.create_small(ident, 9, "", "", small)
    bad = bytearray(post.serialize())
    # claim full-format dimensions on a small post
    bad[8:10] = (256).to_bytes(2, "big")
    with pytest.raises(FormatError) as e:
        Post.parse(bytes(bad))
    assert e.value.reason == "unsupported_dimensions"


def test_create_rejects_wrong_packed_size():
    ident = crypto.Identity(SEED)
    with pytest.raises(FormatError) as e:
        Post.create(ident, 0, "", "", b"\x00" * SMALL_PACKED_LEN)
    assert e.value.reason == "bad_packed_size"
    with pytest.raises(FormatError) as e:
        Post.create_small(ident, 0, "", "", b"\x00" * MONO1_PACKED_LEN)
    assert e.value.reason == "bad_packed_size"


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
