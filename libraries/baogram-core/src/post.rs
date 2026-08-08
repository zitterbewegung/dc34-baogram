//! Canonical Baogram post format, version 1.
//!
//! A post is a fixed, canonical binary structure. All multi-byte integers
//! are **big-endian**. There is exactly one valid serialization for any
//! given post content: parsers reject noncanonical forms.
//!
//! ```text
//! offset  len  field
//! ------  ---  -----
//!      0    4  magic "BGRM"
//!      4    1  version               (= 1)
//!      5    1  pixel_format          (1 = 256x240 full, 2 = 128x120 small; Mono1 packed MSB-first, 1 = white)
//!      6    1  codec                 (0 = RawMono1, 1 = PackBitsMono1, 2 = RowDeltaMono1, 3 = CtxArithMono1)
//!      7    1  flags                 (= 0; reserved bits must be zero)
//!      8    2  width                 (256 full / 128 small)
//!     10    2  height                (240 full / 120 small)
//!     12    8  seq                   (persistent author sequence number)
//!     20   32  author_pubkey         (Ed25519)
//!     52    1  handle_len            (0..=24)
//!     53    1  caption_len           (0..=64)
//!     54    4  image_uncompressed_len (7,680 full / 1,920 small)
//!     58    4  image_encoded_len     (<= 61,440)
//!  -- end of canonical header (62 bytes) --
//!     62    m  handle                (UTF-8, m = handle_len)
//!   62+m    n  caption               (UTF-8, n = caption_len)
//! 62+m+n    e  encoded_image         (e = image_encoded_len)
//!    ...   32  digest = SHA256("BAOGRAM-POST-V1" || header || handle || caption || encoded_image)
//!    ...   64  signature = Ed25519_sign(sk, "BAOGRAM-SIG-V1" || digest)
//! ```
//!
//! The post ID is the first 16 bytes of the digest; fragments use the first
//! 8 bytes as a short ID.

use crate::codec;
use crate::crypto::{self, DIGEST_LEN, POST_ID_LEN, PUBKEY_LEN, SHORT_ID_LEN, SIGNATURE_LEN};
use crate::error::{BaogramError, Result};
use crate::image::{
    IMAGE_HEIGHT, IMAGE_WIDTH, MONO1_PACKED_LEN, Mono1Image, Mono1Small, SMALL_HEIGHT,
    SMALL_PACKED_LEN, SMALL_WIDTH,
};

/// Post container magic.
pub const POST_MAGIC: [u8; 4] = *b"BGRM";
/// Protocol version implemented by this library.
pub const POST_VERSION: u8 = 1;
/// Pixel format 1: 256x240 packed 1-bit monochrome, MSB-first, 1 = white.
pub const PIXEL_FORMAT_MONO1: u8 = 1;
/// Pixel format 2: 128x120 (the badge display's native resolution).
pub const PIXEL_FORMAT_MONO1_SMALL: u8 = 2;

/// (width, height, packed length) for a pixel format, if supported.
pub fn format_geometry(pixel_format: u8) -> Option<(usize, usize, usize)> {
    match pixel_format {
        PIXEL_FORMAT_MONO1 => Some((IMAGE_WIDTH, IMAGE_HEIGHT, MONO1_PACKED_LEN)),
        PIXEL_FORMAT_MONO1_SMALL => Some((SMALL_WIDTH, SMALL_HEIGHT, SMALL_PACKED_LEN)),
        _ => None,
    }
}

/// Maximum UTF-8 bytes in a handle.
pub const HANDLE_MAX_BYTES: usize = 24;
/// Maximum UTF-8 bytes in a caption.
pub const CAPTION_MAX_BYTES: usize = 64;
/// Maximum encoded image bytes.
pub const ENCODED_IMAGE_MAX_BYTES: usize = 61_440;
/// Maximum total serialized post bytes.
pub const MAX_POST_BYTES: usize = 65_536;
/// Length of the canonical header in bytes.
pub const CANONICAL_HEADER_LEN: usize = 62;
/// Length of the digest + signature trailer.
pub const TRAILER_LEN: usize = DIGEST_LEN + SIGNATURE_LEN;

/// A parsed-and-verified (or about-to-be-signed) Baogram post.
#[derive(Clone, PartialEq, Eq)]
pub struct Post {
    pub pixel_format: u8,
    pub codec: u8,
    pub seq: u64,
    pub author_pubkey: [u8; PUBKEY_LEN],
    pub handle: String,
    pub caption: String,
    pub encoded_image: Vec<u8>,
    pub digest: [u8; DIGEST_LEN],
    pub signature: [u8; SIGNATURE_LEN],
}

impl core::fmt::Debug for Post {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Post(id={}, seq={}, handle={:?}, caption_len={}, codec={}, enc_len={})",
            self.post_id().iter().map(|b| format!("{:02x}", b)).collect::<String>(),
            self.seq,
            self.handle,
            self.caption.len(),
            self.codec,
            self.encoded_image.len()
        )
    }
}

fn canonical_header(
    pixel_format: u8,
    codec: u8,
    seq: u64,
    author_pubkey: &[u8; PUBKEY_LEN],
    handle_len: usize,
    caption_len: usize,
    encoded_len: usize,
) -> [u8; CANONICAL_HEADER_LEN] {
    let (width, height, unc_len) = format_geometry(pixel_format).expect("supported pixel format");
    let mut h = [0u8; CANONICAL_HEADER_LEN];
    h[0..4].copy_from_slice(&POST_MAGIC);
    h[4] = POST_VERSION;
    h[5] = pixel_format;
    h[6] = codec;
    h[7] = 0; // flags
    h[8..10].copy_from_slice(&(width as u16).to_be_bytes());
    h[10..12].copy_from_slice(&(height as u16).to_be_bytes());
    h[12..20].copy_from_slice(&seq.to_be_bytes());
    h[20..52].copy_from_slice(author_pubkey);
    h[52] = handle_len as u8;
    h[53] = caption_len as u8;
    h[54..58].copy_from_slice(&(unc_len as u32).to_be_bytes());
    h[58..62].copy_from_slice(&(encoded_len as u32).to_be_bytes());
    h
}

impl Post {
    /// Build and sign a post from a full-resolution Mono1 image.
    ///
    /// Encodes the image with the best codec (compressed only when smaller
    /// than raw), computes the canonical digest, and signs it with the
    /// identity's Ed25519 key.
    pub fn create(
        identity: &crypto::Identity,
        seq: u64,
        handle: &str,
        caption: &str,
        image: &Mono1Image,
    ) -> Result<Post> {
        if handle.len() > HANDLE_MAX_BYTES {
            return Err(BaogramError::FieldTooLong);
        }
        if caption.len() > CAPTION_MAX_BYTES {
            return Err(BaogramError::FieldTooLong);
        }
        let (codec_id, encoded_image) = codec::encode_best(image.packed(), IMAGE_WIDTH, IMAGE_HEIGHT);
        Self::sign_new(identity, PIXEL_FORMAT_MONO1, codec_id, seq, handle, caption, encoded_image)
    }

    /// Build and sign a small-format (128x120, pixel format 2) post.
    /// ~4x less raw data than the full format; nothing is lost on the
    /// badge's 128x128 display.
    pub fn create_small(
        identity: &crypto::Identity,
        seq: u64,
        handle: &str,
        caption: &str,
        image: &Mono1Small,
    ) -> Result<Post> {
        if handle.len() > HANDLE_MAX_BYTES {
            return Err(BaogramError::FieldTooLong);
        }
        if caption.len() > CAPTION_MAX_BYTES {
            return Err(BaogramError::FieldTooLong);
        }
        let (codec_id, encoded_image) = codec::encode_best(image.packed(), SMALL_WIDTH, SMALL_HEIGHT);
        Self::sign_new(identity, PIXEL_FORMAT_MONO1_SMALL, codec_id, seq, handle, caption, encoded_image)
    }

    pub(crate) fn sign_new(
        identity: &crypto::Identity,
        pixel_format: u8,
        codec_id: u8,
        seq: u64,
        handle: &str,
        caption: &str,
        encoded_image: Vec<u8>,
    ) -> Result<Post> {
        let author_pubkey = identity.public_key();
        let header = canonical_header(
            pixel_format,
            codec_id,
            seq,
            &author_pubkey,
            handle.len(),
            caption.len(),
            encoded_image.len(),
        );
        let digest = crypto::post_digest(&header, handle.as_bytes(), caption.as_bytes(), &encoded_image);
        let signature = identity.sign_digest(&digest);
        Ok(Post {
            pixel_format,
            codec: codec_id,
            seq,
            author_pubkey,
            handle: handle.to_string(),
            caption: caption.to_string(),
            encoded_image,
            digest,
            signature,
        })
    }

    /// Serialize to the canonical byte form.
    pub fn serialize(&self) -> Vec<u8> {
        let header = canonical_header(
            self.pixel_format,
            self.codec,
            self.seq,
            &self.author_pubkey,
            self.handle.len(),
            self.caption.len(),
            self.encoded_image.len(),
        );
        let mut out = Vec::with_capacity(
            CANONICAL_HEADER_LEN
                + self.handle.len()
                + self.caption.len()
                + self.encoded_image.len()
                + TRAILER_LEN,
        );
        out.extend_from_slice(&header);
        out.extend_from_slice(self.handle.as_bytes());
        out.extend_from_slice(self.caption.as_bytes());
        out.extend_from_slice(&self.encoded_image);
        out.extend_from_slice(&self.digest);
        out.extend_from_slice(&self.signature);
        out
    }

    /// Parse and fully verify a serialized post.
    ///
    /// All length fields are validated before any allocation is sized from
    /// them. Rejects: bad magic, unknown version/pixel format/codec/flags,
    /// unsupported dimensions, field-length violations, invalid UTF-8,
    /// noncanonical lengths, trailing bytes, digest mismatch, and invalid
    /// signatures. On success the image is guaranteed to decode to exactly
    /// MONO1_PACKED_LEN bytes.
    pub fn parse(bytes: &[u8]) -> Result<Post> {
        if bytes.len() > MAX_POST_BYTES {
            return Err(BaogramError::PostTooLarge);
        }
        if bytes.len() < CANONICAL_HEADER_LEN + TRAILER_LEN {
            return Err(BaogramError::Truncated);
        }
        if bytes[0..4] != POST_MAGIC {
            return Err(BaogramError::BadMagic);
        }
        if bytes[4] != POST_VERSION {
            return Err(BaogramError::UnknownVersion);
        }
        let pixel_format = bytes[5];
        let Some((fmt_width, fmt_height, fmt_unc_len)) = format_geometry(pixel_format) else {
            return Err(BaogramError::UnknownPixelFormat);
        };
        let codec_id = bytes[6];
        if codec_id > codec::CODEC_CTXARITH_MONO1 {
            return Err(BaogramError::UnknownCodec);
        }
        if bytes[7] != 0 {
            return Err(BaogramError::UnknownFlags);
        }
        let width = u16::from_be_bytes([bytes[8], bytes[9]]) as usize;
        let height = u16::from_be_bytes([bytes[10], bytes[11]]) as usize;
        if width != fmt_width || height != fmt_height {
            return Err(BaogramError::UnsupportedDimensions);
        }
        let seq = u64::from_be_bytes(bytes[12..20].try_into().unwrap());
        let author_pubkey: [u8; PUBKEY_LEN] = bytes[20..52].try_into().unwrap();
        let handle_len = bytes[52] as usize;
        let caption_len = bytes[53] as usize;
        if handle_len > HANDLE_MAX_BYTES || caption_len > CAPTION_MAX_BYTES {
            return Err(BaogramError::FieldTooLong);
        }
        let unc_len = u32::from_be_bytes(bytes[54..58].try_into().unwrap()) as usize;
        if unc_len != fmt_unc_len {
            return Err(BaogramError::NoncanonicalLength);
        }
        let enc_len = u32::from_be_bytes(bytes[58..62].try_into().unwrap()) as usize;
        if enc_len > ENCODED_IMAGE_MAX_BYTES {
            return Err(BaogramError::NoncanonicalLength);
        }
        // exact total-length check before any slicing derived from lengths
        let expected_total = CANONICAL_HEADER_LEN + handle_len + caption_len + enc_len + TRAILER_LEN;
        if bytes.len() < expected_total {
            return Err(BaogramError::Truncated);
        }
        if bytes.len() > expected_total {
            return Err(BaogramError::TrailingBytes);
        }
        let mut off = CANONICAL_HEADER_LEN;
        let handle_bytes = &bytes[off..off + handle_len];
        off += handle_len;
        let caption_bytes = &bytes[off..off + caption_len];
        off += caption_len;
        let encoded_image = &bytes[off..off + enc_len];
        off += enc_len;
        let digest: [u8; DIGEST_LEN] = bytes[off..off + DIGEST_LEN].try_into().unwrap();
        off += DIGEST_LEN;
        let signature: [u8; SIGNATURE_LEN] = bytes[off..off + SIGNATURE_LEN].try_into().unwrap();

        let handle = core::str::from_utf8(handle_bytes).map_err(|_| BaogramError::InvalidUtf8)?.to_string();
        let caption = core::str::from_utf8(caption_bytes).map_err(|_| BaogramError::InvalidUtf8)?.to_string();

        // recompute the digest over canonical regions
        let header =
            canonical_header(pixel_format, codec_id, seq, &author_pubkey, handle_len, caption_len, enc_len);
        let computed = crypto::post_digest(&header, handle_bytes, caption_bytes, encoded_image);
        if computed != digest {
            return Err(BaogramError::DigestMismatch);
        }
        crypto::verify_digest_signature(&author_pubkey, &digest, &signature)?;

        // the image must decode to exactly the packed length; the decode also
        // enforces the compressed-strictly-smaller canonicality rule
        let _ = codec::decode(codec_id, encoded_image, fmt_unc_len, fmt_width / 8)?;

        Ok(Post {
            pixel_format,
            codec: codec_id,
            seq,
            author_pubkey,
            handle,
            caption,
            encoded_image: encoded_image.to_vec(),
            digest,
            signature,
        })
    }

    /// Decode the image payload to a full-resolution Mono1 image.
    /// Small-format posts are pixel-doubled (lossless for their content),
    /// so every full-resolution rendering path applies unchanged.
    pub fn decode_image(&self) -> Result<Mono1Image> {
        let (width, _height, unc_len) =
            format_geometry(self.pixel_format).ok_or(BaogramError::UnknownPixelFormat)?;
        let packed = codec::decode(self.codec, &self.encoded_image, unc_len, width / 8)?;
        if self.pixel_format == PIXEL_FORMAT_MONO1_SMALL {
            Ok(Mono1Small::from_packed(&packed)?.to_full())
        } else {
            Mono1Image::from_packed(&packed)
        }
    }

    /// The 16-byte post ID (first 16 bytes of the digest).
    pub fn post_id(&self) -> [u8; POST_ID_LEN] {
        self.digest[..POST_ID_LEN].try_into().unwrap()
    }

    /// The 8-byte short post ID used in fragments.
    pub fn short_id(&self) -> [u8; SHORT_ID_LEN] {
        self.digest[..SHORT_ID_LEN].try_into().unwrap()
    }

    /// Total serialized length in bytes.
    pub fn serialized_len(&self) -> usize {
        CANONICAL_HEADER_LEN + self.handle.len() + self.caption.len() + self.encoded_image.len() + TRAILER_LEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Identity;
    use crate::image::synthetic_test_frame;

    const TEST_SEED: [u8; 32] = *b"BAOGRAM-TEST-SEED-0000000000001!";

    fn test_image() -> Mono1Image {
        Mono1Image::quantize(&synthetic_test_frame(), None).unwrap()
    }

    fn test_post() -> Post {
        let id = Identity::from_seed(&TEST_SEED);
        Post::create(&id, 3, "alice", "hello badge", &test_image()).unwrap()
    }

    #[test]
    fn round_trip() {
        let post = test_post();
        let bytes = post.serialize();
        assert!(bytes.len() <= MAX_POST_BYTES);
        let parsed = Post::parse(&bytes).unwrap();
        assert_eq!(parsed, post);
        assert_eq!(parsed.decode_image().unwrap(), test_image());
    }

    #[test]
    fn raw_codec_round_trip() {
        // incompressible image forces raw fallback
        let mut state = 0x1234_5678u32;
        let mut noise = vec![0u8; MONO1_PACKED_LEN];
        for b in noise.iter_mut() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *b = (state & 0xff) as u8;
        }
        let img = Mono1Image::from_packed(&noise).unwrap();
        let id = Identity::from_seed(&TEST_SEED);
        let post = Post::create(&id, 1, "", "", &img).unwrap();
        assert_eq!(post.codec, codec::CODEC_RAW_MONO1);
        let parsed = Post::parse(&post.serialize()).unwrap();
        assert_eq!(parsed.decode_image().unwrap(), img);
    }

    #[test]
    fn rowdelta_codec_round_trip() {
        // vertically constant image: PackBits alone cannot compress it
        // (alternating bytes within each row) but the row-delta filter
        // zeroes every row after the first
        let mut packed = Vec::with_capacity(MONO1_PACKED_LEN);
        for _row in 0..240 {
            for byte in 0..32 {
                packed.push(if byte % 2 == 0 { 0xff } else { 0x00 });
            }
        }
        let img = Mono1Image::from_packed(&packed).unwrap();
        let id = Identity::from_seed(&TEST_SEED);
        let post = Post::create(&id, 2, "delta", "", &img).unwrap();
        assert_eq!(post.codec, codec::CODEC_ROWDELTA_MONO1);
        let parsed = Post::parse(&post.serialize()).unwrap();
        assert_eq!(parsed.decode_image().unwrap(), img);
    }

    #[test]
    fn small_format_round_trip() {
        // capture pipeline: synthetic gray frame -> small quantize ->
        // despeckle -> sign; decode pixel-doubles back to full res
        let small = Mono1Small::from_gray(&synthetic_test_frame(), None).unwrap().despeckle();
        let id = Identity::from_seed(&TEST_SEED);
        let post = Post::create_small(&id, 5, "small", "compact", &small).unwrap();
        assert_eq!(post.pixel_format, PIXEL_FORMAT_MONO1_SMALL);
        let bytes = post.serialize();
        let parsed = Post::parse(&bytes).unwrap();
        assert_eq!(parsed, post);
        assert_eq!(parsed.decode_image().unwrap(), small.to_full());
        // a small post is drastically smaller than the full-format one
        let full = Post::create(&id, 5, "small", "compact", &test_image()).unwrap();
        assert!(bytes.len() < full.serialize().len());
    }

    #[test]
    fn small_format_dimension_mismatch_rejected() {
        let small = Mono1Small::from_gray(&synthetic_test_frame(), None).unwrap();
        let id = Identity::from_seed(&TEST_SEED);
        let post = Post::create_small(&id, 5, "", "", &small).unwrap();
        let mut bytes = post.serialize();
        // claim full-format dimensions on a small post
        bytes[8..10].copy_from_slice(&256u16.to_be_bytes());
        assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::UnsupportedDimensions);
    }

    #[test]
    fn handle_caption_bounds() {
        let id = Identity::from_seed(&TEST_SEED);
        let img = test_image();
        assert!(Post::create(&id, 0, &"x".repeat(24), "", &img).is_ok());
        assert_eq!(Post::create(&id, 0, &"x".repeat(25), "", &img).unwrap_err(), BaogramError::FieldTooLong);
        assert!(Post::create(&id, 0, "", &"y".repeat(64), &img).is_ok());
        assert_eq!(Post::create(&id, 0, "", &"y".repeat(65), &img).unwrap_err(), BaogramError::FieldTooLong);
    }

    #[test]
    fn tampered_caption_rejected() {
        let post = test_post();
        let mut bytes = post.serialize();
        // caption starts at 62 + handle_len
        let cap_off = CANONICAL_HEADER_LEN + post.handle.len();
        bytes[cap_off] ^= 0x01;
        assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::DigestMismatch);
    }

    #[test]
    fn tampered_image_rejected() {
        let post = test_post();
        let mut bytes = post.serialize();
        let img_off = CANONICAL_HEADER_LEN + post.handle.len() + post.caption.len();
        bytes[img_off + 5] ^= 0xff;
        assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::DigestMismatch);
    }

    #[test]
    fn tampered_signature_rejected() {
        let post = test_post();
        let mut bytes = post.serialize();
        let sig_off = bytes.len() - 1;
        bytes[sig_off] ^= 0x01;
        assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::InvalidSignature);
    }

    #[test]
    fn forged_digest_rejected() {
        // recompute-able digest cannot simply be swapped: flipping a digest
        // byte breaks the recomputation match first
        let post = test_post();
        let mut bytes = post.serialize();
        let digest_off = bytes.len() - TRAILER_LEN;
        bytes[digest_off] ^= 0x01;
        assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::DigestMismatch);
    }

    #[test]
    fn wrong_key_signature_rejected() {
        // sign with one key, claim another author
        let id_a = Identity::from_seed(&TEST_SEED);
        let id_b = Identity::from_seed(&[9u8; 32]);
        let mut post = Post::create(&id_a, 0, "a", "b", &test_image()).unwrap();
        // swap in B's public key and recompute digest so digest check passes
        post.author_pubkey = id_b.public_key();
        let header = canonical_header(
            post.pixel_format,
            post.codec,
            post.seq,
            &post.author_pubkey,
            post.handle.len(),
            post.caption.len(),
            post.encoded_image.len(),
        );
        post.digest = crypto::post_digest(
            &header,
            post.handle.as_bytes(),
            post.caption.as_bytes(),
            &post.encoded_image,
        );
        // signature still A's => must fail
        assert_eq!(Post::parse(&post.serialize()).unwrap_err(), BaogramError::InvalidSignature);
    }

    #[test]
    fn truncated_and_trailing_rejected() {
        let bytes = test_post().serialize();
        assert_eq!(Post::parse(&bytes[..bytes.len() - 1]).unwrap_err(), BaogramError::Truncated);
        assert_eq!(Post::parse(&bytes[..10]).unwrap_err(), BaogramError::Truncated);
        let mut extended = bytes.clone();
        extended.push(0);
        assert_eq!(Post::parse(&extended).unwrap_err(), BaogramError::TrailingBytes);
    }

    #[test]
    fn unknown_version_rejected() {
        let mut bytes = test_post().serialize();
        bytes[4] = 2;
        assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::UnknownVersion);
    }

    #[test]
    fn bad_magic_rejected() {
        let mut bytes = test_post().serialize();
        bytes[0] = b'X';
        assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::BadMagic);
    }

    #[test]
    fn unknown_pixel_format_and_codec_rejected() {
        let mut bytes = test_post().serialize();
        bytes[5] = 7;
        assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::UnknownPixelFormat);
        let mut bytes = test_post().serialize();
        bytes[6] = 7;
        assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::UnknownCodec);
    }

    #[test]
    fn nonzero_flags_rejected() {
        let mut bytes = test_post().serialize();
        bytes[7] = 1;
        assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::UnknownFlags);
    }

    #[test]
    fn wrong_dimensions_rejected() {
        let mut bytes = test_post().serialize();
        bytes[8..10].copy_from_slice(&128u16.to_be_bytes());
        assert_eq!(Post::parse(&bytes).unwrap_err(), BaogramError::UnsupportedDimensions);
    }

    #[test]
    fn post_id_is_digest_prefix() {
        let post = test_post();
        assert_eq!(&post.post_id()[..], &post.digest[..16]);
        assert_eq!(&post.short_id()[..], &post.digest[..8]);
    }

    #[test]
    fn empty_handle_and_caption_ok() {
        let id = Identity::from_seed(&TEST_SEED);
        let post = Post::create(&id, 0, "", "", &test_image()).unwrap();
        let parsed = Post::parse(&post.serialize()).unwrap();
        assert_eq!(parsed.handle, "");
        assert_eq!(parsed.caption, "");
    }
}
