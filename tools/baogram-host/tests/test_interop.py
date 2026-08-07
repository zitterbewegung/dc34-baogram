"""Interop tests: the Python implementation must consume every Rust golden
vector and produce vectors that Rust accepts.

The Rust vectors live in ../../libraries/baogram-core/test-vectors and are
committed; regen with `BAOGRAM_REGEN_VECTORS=1 cargo test golden` there.
"""

import pathlib

import pytest

from baogram_host import codec, crypto
from baogram_host.format import FormatError, Post
from baogram_host.fragments import Fragment, FragmentError, Reassembler
from baogram_host.image import quantize, synthetic_test_frame

VECTORS = (
    pathlib.Path(__file__).resolve().parents[3] / "libraries" / "baogram-core" / "test-vectors"
)

TEST_SEED = b"BAOGRAM-TEST-SEED-0000000000001!"


def read(name: str) -> bytes:
    return (VECTORS / name).read_bytes()


def test_vectors_directory_exists():
    assert VECTORS.is_dir(), f"golden vectors missing at {VECTORS}"


def test_valid_packbits_post_parses_and_matches_rust_identity():
    data = read("valid-post-packbits.bgrm")
    post = Post.parse(data)
    assert post.handle == "golden"
    assert post.caption == "baogram golden vector v1"
    assert post.seq == 7
    assert post.codec == codec.CODEC_PACKBITS_MONO1
    # Python's Ed25519 must derive the same public key from the shared seed
    ident = crypto.Identity(TEST_SEED)
    assert post.author_pubkey == ident.public_key
    # image decodes to exactly the Python-quantized synthetic frame
    assert post.decode_image() == quantize(synthetic_test_frame())


def test_valid_raw_post_parses():
    post = Post.parse(read("valid-post-raw.bgrm"))
    assert post.codec == codec.CODEC_RAW_MONO1
    assert len(post.encoded_image) == 7680


def test_python_reserializes_rust_posts_byte_identically():
    for name in ("valid-post-packbits.bgrm", "valid-post-raw.bgrm"):
        data = read(name)
        assert Post.parse(data).serialize() == data, name


def test_python_signature_matches_rust_exactly():
    """Ed25519 is deterministic: building the same post from the same seed
    must reproduce the Rust bytes exactly."""
    data = read("valid-post-packbits.bgrm")
    ident = crypto.Identity(TEST_SEED)
    packed = quantize(synthetic_test_frame())
    rebuilt = Post.create(ident, 7, "golden", "baogram golden vector v1", packed)
    assert rebuilt.serialize() == data


@pytest.mark.parametrize(
    "name,reason",
    [
        ("invalid-signature.bgrm", "invalid_signature"),
        ("modified-caption.bgrm", "digest_mismatch"),
        ("modified-image.bgrm", "digest_mismatch"),
        ("truncated-post.bgrm", "truncated"),
        ("unknown-version.bgrm", "unknown_version"),
    ],
)
def test_negative_vectors_rejected(name, reason):
    with pytest.raises(FormatError) as e:
        Post.parse(read(name))
    assert e.value.reason == reason


def test_valid_fragment_and_crc_vectors():
    frag = Fragment.from_base45(read("valid-fragment.b45").decode())
    assert frag.frag_index == 0
    with pytest.raises(FragmentError) as e:
        Fragment.from_base45(read("invalid-crc.b45").decode())
    assert e.value.reason == "crc_mismatch"


def test_out_of_order_reassembly_of_rust_fragments():
    lines = read("fragments-all.b45").decode().splitlines()
    frags = [Fragment.from_base45(l) for l in lines]
    order = list(reversed(range(len(frags))))
    order[0], order[7] = order[7], order[0]
    order[3], order[11] = order[11], order[3]
    r = Reassembler(frags[order[0]])
    for i in order[1:]:
        r.feed(frags[i])
    # duplicate ignored
    assert r.feed(frags[2]) == "duplicate"
    # conflicting duplicate fails
    conflict = Fragment.from_base45(read("conflicting-fragment.b45").decode())
    with pytest.raises(FragmentError) as e:
        r.feed(conflict)
    assert e.value.reason == "conflict"
    data = r.to_bytes()
    assert data == read("valid-post-packbits.bgrm")
    Post.parse(data)  # verifies signature


def test_python_fragments_match_rust_fragments():
    """Fragmenting the golden post in Python must produce the exact Base45
    lines Rust produced."""
    data = read("valid-post-packbits.bgrm")
    post = Post.parse(data)
    from baogram_host.fragments import fragment_post

    frags = fragment_post(post.short_id, data, 64)
    expected = read("fragments-all.b45").decode().splitlines()
    assert [f.to_base45() for f in frags] == expected


def test_generate_python_vectors_for_rust(tmp_path):
    """Write the Python-generated vectors consumed by the Rust test
    `golden::python_generated_vectors` into test-vectors/python/."""
    outdir = VECTORS / "python"
    outdir.mkdir(exist_ok=True)
    ident = crypto.Identity(b"BAOGRAM-PYTEST-SEED-00000000002!")
    packed = quantize(synthetic_test_frame())
    post = Post.create(ident, 42, "pyhost", "made in python", packed)
    data = post.serialize()
    (outdir / "post-from-python.bgrm").write_bytes(data)
    from baogram_host.fragments import fragment_post

    frag = fragment_post(post.short_id, data, 64)[0]
    (outdir / "fragment-from-python.b45").write_text(frag.to_base45() + "\n")
    # self-check: parse back
    assert Post.parse(data).post_id == post.post_id
