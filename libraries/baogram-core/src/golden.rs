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
use crate::fragment::{
    FeedResult, FountainDecoder, FountainFrame, FountainIter, Fragment, Reassembler, SplitMix64,
    fragment_post,
};
use crate::image::{MONO1_PACKED_LEN, Mono1Image, synthetic_test_frame};
use crate::post::Post;

const TEST_SEED: [u8; 32] = *b"BAOGRAM-TEST-SEED-0000000000001!";
const GOLDEN_HANDLE: &str = "golden";
const GOLDEN_CAPTION: &str = "baogram golden vector v1";
const GOLDEN_SEQ: u64 = 7;
const FRAGMENT_CHUNK: usize = 64;
const FOUNTAIN_CHUNK: usize = 96;
/// Coded frames beyond the systematic prefix in the fountain vector file.
const FOUNTAIN_EXTRA: usize = 10;

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

/// The canonical compressible golden post: synthetic frame. Its strong
/// vertical correlation makes the row-delta codec win.
fn golden_post_rowdelta() -> Post {
    let identity = Identity::from_seed(&TEST_SEED);
    let image = Mono1Image::quantize(&synthetic_test_frame(), None).unwrap();
    let post = Post::create(&identity, GOLDEN_SEQ, GOLDEN_HANDLE, GOLDEN_CAPTION, &image).unwrap();
    assert_eq!(post.codec, codec::CODEC_ROWDELTA_MONO1, "synthetic frame must pick row-delta");
    post
}

/// A golden post that exercises codec 1: every row is a solid run of one
/// pseudo-random byte, so PackBits and row-delta tie and the lower codec
/// id wins by the canonical rule.
fn golden_post_packbits() -> Post {
    let identity = Identity::from_seed(&TEST_SEED);
    let mut state = 0x0bad_cafeu32;
    let mut img = Vec::with_capacity(MONO1_PACKED_LEN);
    for _ in 0..240 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        img.extend_from_slice(&[(state & 0xff) as u8; 32]);
    }
    let image = Mono1Image::from_packed(&img).unwrap();
    let post = Post::create(&identity, GOLDEN_SEQ + 2, GOLDEN_HANDLE, "packbits golden", &image).unwrap();
    assert_eq!(post.codec, codec::CODEC_PACKBITS_MONO1, "tie must pick the lower codec id");
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
    let rd = golden_post_rowdelta();
    let rd_bytes = rd.serialize();
    check_or_write("valid-post-rowdelta.bgrm", &rd_bytes);
    let parsed = Post::parse(&rd_bytes).unwrap();
    assert_eq!(
        parsed.decode_image().unwrap().packed()[..],
        Mono1Image::quantize(&synthetic_test_frame(), None).unwrap().packed()[..]
    );

    let pb = golden_post_packbits();
    let pb_bytes = pb.serialize();
    check_or_write("valid-post-packbits.bgrm", &pb_bytes);
    Post::parse(&pb_bytes).unwrap();

    let raw = golden_post_raw();
    let raw_bytes = raw.serialize();
    check_or_write("valid-post-raw.bgrm", &raw_bytes);
    Post::parse(&raw_bytes).unwrap();
}

#[test]
fn golden_invalid_signature() {
    let mut bytes = golden_post_rowdelta().serialize();
    let n = bytes.len();
    bytes[n - 1] ^= 0x01; // flip last signature byte
    check_or_write("invalid-signature.bgrm", &bytes);
    assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::InvalidSignature);
}

#[test]
fn golden_modified_caption() {
    let post = golden_post_rowdelta();
    let mut bytes = post.serialize();
    let cap_off = crate::post::CANONICAL_HEADER_LEN + post.handle.len();
    bytes[cap_off] ^= 0x20; // 'b' -> 'B' in the caption
    check_or_write("modified-caption.bgrm", &bytes);
    assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::DigestMismatch);
}

#[test]
fn golden_modified_image() {
    let post = golden_post_rowdelta();
    let mut bytes = post.serialize();
    let img_off = crate::post::CANONICAL_HEADER_LEN + post.handle.len() + post.caption.len();
    bytes[img_off + 10] ^= 0xff;
    check_or_write("modified-image.bgrm", &bytes);
    assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::DigestMismatch);
}

#[test]
fn golden_truncated_post() {
    let bytes = golden_post_rowdelta().serialize();
    let truncated = &bytes[..bytes.len() / 2];
    check_or_write("truncated-post.bgrm", truncated);
    assert_eq!(Post::parse(truncated).unwrap_err(), BaogramError::Truncated);
}

#[test]
fn golden_unknown_version() {
    let mut bytes = golden_post_rowdelta().serialize();
    bytes[4] = 0x7f;
    check_or_write("unknown-version.bgrm", &bytes);
    assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::UnknownVersion);
}

#[test]
fn golden_fragments() {
    let post = golden_post_rowdelta();
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

#[test]
fn golden_fountain_frames() {
    let post = golden_post_rowdelta();
    let bytes = post.serialize();
    let it = FountainIter::new(&post.short_id(), &bytes, FOUNTAIN_CHUNK).unwrap();
    let k = it.chunk_count();

    // the full systematic prefix plus the first coded frames, one Base45
    // line each — this pins the v2 wire format AND the PRNG/degree spec
    let frames: Vec<FountainFrame> = (0..(k + FOUNTAIN_EXTRA) as u32).map(|f| it.frame_at(f)).collect();
    let all: String = frames.iter().map(|f| f.to_base45()).collect::<Vec<_>>().join("\n");
    check_or_write("fountain-frames.b45", all.as_bytes());

    // decode from a deterministic lossy shuffle of the committed lines:
    // drop every third frame, feed the rest in scrambled order, then keep
    // pulling later coded frames until complete
    let parsed: Vec<FountainFrame> =
        all.lines().map(|l| FountainFrame::from_base45(l).unwrap()).collect();
    let mut order: Vec<usize> = (0..parsed.len()).filter(|i| i % 3 != 0).collect();
    let mut rng = SplitMix64::new(0x601D);
    for i in (1..order.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    let (mut d, _) = FountainDecoder::new(&parsed[order[0]]).unwrap();
    for &i in &order[1..] {
        if d.feed(&parsed[i]).unwrap() == FeedResult::Complete {
            break;
        }
    }
    let mut f = (k + FOUNTAIN_EXTRA) as u32;
    while !d.is_complete() {
        d.feed(&it.frame_at(f)).unwrap();
        f += 1;
        assert!(f < 100 * k as u32, "fountain decode must converge");
    }
    let rebuilt = d.into_bytes().unwrap();
    assert_eq!(rebuilt, bytes);
    assert_eq!(Post::parse(&rebuilt).unwrap(), post);
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

    // fountain frames from Python: feed them in reverse order (any-order by
    // construction) and require the exact post bytes back
    let fountain_path = dir.join("fountain-from-python.b45");
    if fountain_path.exists() {
        let text = std::fs::read_to_string(&fountain_path).unwrap();
        let frames: Vec<FountainFrame> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| FountainFrame::from_base45(l.trim()).expect("Python fountain frame must parse"))
            .collect();
        assert!(!frames.is_empty());
        let (mut d, _) = FountainDecoder::new(frames.last().unwrap()).unwrap();
        for f in frames.iter().rev().skip(1) {
            if d.feed(f).unwrap() == FeedResult::Complete {
                break;
            }
        }
        assert!(d.is_complete(), "Python fountain frames must decode in Rust (got {}/{})",
            d.received_count(), d.total_count());
        assert_eq!(d.into_bytes().unwrap(), bytes, "Python fountain stream must rebuild the post");
    }
}
