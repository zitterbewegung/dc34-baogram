//! Golden-vector tests.
//!
//! Deterministic vectors live in `test-vectors/`. Every vector is derived
//! from a fixed identity seed, the synthetic test frame, and fixed metadata,
//! so regeneration is byte-for-byte reproducible on any host.
//!
//! To (re)generate the committed vectors run:
//! `BAOGRAM_REGEN_VECTORS=1 cargo test golden -- --nocapture`
//! and inspect the diff before committing. Normal test runs compare the
//! committed files against freshly computed bytes and exercise every
//! negative vector's rejection path.
//!
//! `test-vectors/python/` holds vectors produced by the Python peer
//! (`tools/baogram-host`); when present they are verified here, closing the
//! Python -> Rust interop loop.

use std::path::{Path, PathBuf};

use crate::codec;
use crate::crypto::Identity;
use crate::error::BaogramError;
use crate::fragment::{FeedResult, Fragment, Reassembler, fragment_post};
use crate::image::{MONO1_PACKED_LEN, Mono1Image, synthetic_test_frame};
use crate::post::Post;

const TEST_SEED: [u8; 32] = *b"BAOGRAM-TEST-SEED-0000000000001!";
const GOLDEN_HANDLE: &str = "golden";
const GOLDEN_CAPTION: &str = "baogram golden vector v1";
const GOLDEN_SEQ: u64 = 7;
const FRAGMENT_CHUNK: usize = 64;

fn vectors_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("test-vectors")
}

fn regen() -> bool {
    std::env::var("BAOGRAM_REGEN_VECTORS").as_deref() == Ok("1")
}

/// Compare `bytes` against the committed file, or rewrite it in regen mode.
fn check_or_write(name: &str, bytes: &[u8]) {
    let path = vectors_dir().join(name);
    if regen() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        eprintln!("wrote {} ({} bytes)", path.display(), bytes.len());
    } else {
        let committed = std::fs::read(&path).unwrap_or_else(|e| {
            panic!("missing golden vector {} ({e}); run with BAOGRAM_REGEN_VECTORS=1", path.display())
        });
        assert_eq!(committed, bytes, "golden vector {} drifted", name);
    }
}

/// The canonical compressible golden post: synthetic frame, PackBits codec.
fn golden_post_packbits() -> Post {
    let identity = Identity::from_seed(&TEST_SEED);
    let image = Mono1Image::quantize(&synthetic_test_frame(), None).unwrap();
    let post = Post::create(&identity, GOLDEN_SEQ, GOLDEN_HANDLE, GOLDEN_CAPTION, &image).unwrap();
    assert_eq!(post.codec, codec::CODEC_PACKBITS_MONO1, "synthetic frame must compress");
    post
}

/// The canonical raw golden post: xorshift noise image, raw codec.
fn golden_post_raw() -> Post {
    let identity = Identity::from_seed(&TEST_SEED);
    let mut state = 0x1234_5678u32;
    let mut noise = vec![0u8; MONO1_PACKED_LEN];
    for b in noise.iter_mut() {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *b = (state & 0xff) as u8;
    }
    let image = Mono1Image::from_packed(&noise).unwrap();
    let post = Post::create(&identity, GOLDEN_SEQ + 1, GOLDEN_HANDLE, "raw golden", &image).unwrap();
    assert_eq!(post.codec, codec::CODEC_RAW_MONO1, "noise image must not compress");
    post
}

#[test]
fn golden_valid_posts() {
    let pb = golden_post_packbits();
    let pb_bytes = pb.serialize();
    check_or_write("valid-post-packbits.bgrm", &pb_bytes);
    let parsed = Post::parse(&pb_bytes).unwrap();
    assert_eq!(
        parsed.decode_image().unwrap().packed()[..],
        Mono1Image::quantize(&synthetic_test_frame(), None).unwrap().packed()[..]
    );

    let raw = golden_post_raw();
    let raw_bytes = raw.serialize();
    check_or_write("valid-post-raw.bgrm", &raw_bytes);
    Post::parse(&raw_bytes).unwrap();
}

#[test]
fn golden_invalid_signature() {
    let mut bytes = golden_post_packbits().serialize();
    let n = bytes.len();
    bytes[n - 1] ^= 0x01; // flip last signature byte
    check_or_write("invalid-signature.bgrm", &bytes);
    assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::InvalidSignature);
}

#[test]
fn golden_modified_caption() {
    let post = golden_post_packbits();
    let mut bytes = post.serialize();
    let cap_off = crate::post::CANONICAL_HEADER_LEN + post.handle.len();
    bytes[cap_off] ^= 0x20; // 'b' -> 'B' in the caption
    check_or_write("modified-caption.bgrm", &bytes);
    assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::DigestMismatch);
}

#[test]
fn golden_modified_image() {
    let post = golden_post_packbits();
    let mut bytes = post.serialize();
    let img_off = crate::post::CANONICAL_HEADER_LEN + post.handle.len() + post.caption.len();
    bytes[img_off + 10] ^= 0xff;
    check_or_write("modified-image.bgrm", &bytes);
    assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::DigestMismatch);
}

#[test]
fn golden_truncated_post() {
    let bytes = golden_post_packbits().serialize();
    let truncated = &bytes[..bytes.len() / 2];
    check_or_write("truncated-post.bgrm", truncated);
    assert_eq!(Post::parse(truncated).unwrap_err(), BaogramError::Truncated);
}

#[test]
fn golden_unknown_version() {
    let mut bytes = golden_post_packbits().serialize();
    bytes[4] = 0x7f;
    check_or_write("unknown-version.bgrm", &bytes);
    assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::UnknownVersion);
}

#[test]
fn golden_fragments() {
    let post = golden_post_packbits();
    let bytes = post.serialize();
    let frags = fragment_post(&post.short_id(), &bytes, FRAGMENT_CHUNK).unwrap();

    // all fragments, one Base45 line each
    let all: String = frags.iter().map(|f| f.to_base45()).collect::<Vec<_>>().join("\n");
    check_or_write("fragments-all.b45", all.as_bytes());

    // single valid fragment
    check_or_write("valid-fragment.b45", frags[0].to_base45().as_bytes());

    // invalid CRC: corrupt one payload byte of the binary form (CRC stays)
    let mut corrupt = frags[0].to_bytes();
    corrupt[crate::fragment::FRAGMENT_HEADER_LEN] ^= 0x55;
    let corrupt45 = base45::encode(&corrupt);
    check_or_write("invalid-crc.b45", corrupt45.as_bytes());
    assert_eq!(Fragment::from_base45(&corrupt45).unwrap_err(), BaogramError::FragmentCrcMismatch);

    // conflicting duplicate: index 0 with different payload, valid CRC
    let mut conflict = frags[0].clone();
    conflict.payload[0] ^= 0xff;
    let conflict45 = conflict.to_base45();
    check_or_write("conflicting-fragment.b45", conflict45.as_bytes());

    // reassemble out of order (deterministic scramble: reverse, then swaps)
    let mut order: Vec<usize> = (0..frags.len()).collect();
    order.reverse();
    if order.len() >= 12 {
        order.swap(0, 7);
        order.swap(3, 11);
    }
    let parsed: Vec<Fragment> = all.lines().map(|l| Fragment::from_base45(l).unwrap()).collect();
    let (mut r, _) = Reassembler::new(&parsed[order[0]]).unwrap();
    for &i in &order[1..] {
        r.feed(&parsed[i]).unwrap();
    }
    // duplicate is ignored
    assert_eq!(r.feed(&parsed[2]).unwrap(), FeedResult::Duplicate);
    // conflicting duplicate fails the transfer
    let conflict_parsed = Fragment::from_base45(&conflict45).unwrap();
    assert_eq!(r.feed(&conflict_parsed).unwrap_err(), BaogramError::FragmentConflict);
    // and the reassembled bytes parse + verify to the same post
    assert!(r.is_complete());
    let rebuilt = r.into_bytes().unwrap();
    assert_eq!(rebuilt, bytes);
    let reparsed = Post::parse(&rebuilt).unwrap();
    assert_eq!(reparsed, post);
}

/// Interop: verify vectors produced by the Python peer, when committed.
#[test]
fn python_generated_vectors() {
    let dir = vectors_dir().join("python");
    let post_path = dir.join("post-from-python.bgrm");
    if !post_path.exists() {
        eprintln!(
            "NOTE: {} not present; Python interop vector not yet generated (see tools/baogram-host).",
            post_path.display()
        );
        return;
    }
    let bytes = std::fs::read(&post_path).unwrap();
    let post = Post::parse(&bytes).expect("Python-generated post must parse and verify in Rust");
    let img = post.decode_image().expect("image must decode");
    // the Python vector uses the same synthetic frame + threshold rules
    let expected = Mono1Image::quantize(&synthetic_test_frame(), None).unwrap();
    assert_eq!(img.packed()[..], expected.packed()[..], "Python Mono1 pixels must match Rust exactly");

    let frag_path = dir.join("fragment-from-python.b45");
    if frag_path.exists() {
        let text = std::fs::read_to_string(&frag_path).unwrap();
        let frag = Fragment::from_base45(text.trim()).expect("Python fragment must parse");
        assert_eq!(frag.short_post_id, post.short_id());
    }
}
