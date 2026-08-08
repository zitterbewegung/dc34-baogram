//! # CtxArithMono1: context-modeled adaptive binary range coder for Mono1.
//!
//! A 10-bit-context adaptive binary range coder (LZMA-style rc, MOVE_BITS 5,
//! 11-bit probabilities) for bilevel images packed MSB-first row-major.
//! The Python peer (`tools/baogram-host/baogram_host/ctxcodec.py`) produces
//! byte-identical encodings; cross-language pins live in both test suites.
//!
//! ## Frozen wire semantics
//!
//! - Pixels are processed in raster order (y outer, x inner); out-of-bounds
//!   template reads are 0.
//! - Context (10 bits, three-line template), `b(x,y)` = pixel value:
//!   ```text
//!   ctx = b(x-2,y)   << 0 | b(x-1,y)   << 1
//!       | b(x-2,y-1) << 2 | b(x-1,y-1) << 3 | b(x,y-1) << 4
//!       | b(x+1,y-1) << 5 | b(x+2,y-1) << 6
//!       | b(x-1,y-2) << 7 | b(x,y-2)   << 8 | b(x+1,y-2) << 9
//!   ```
//! - `probs[ctx]` is P(next bit is 0) on an 11-bit scale, initialized to
//!   1024; LZMA update rule with MOVE_BITS = 5 after coding each bit.
//! - Range encoder/decoder are the canonical LZMA binary rc. The first
//!   output byte is always 0 (the initial cache) and is kept; the decoder
//!   skips it and may leave up to 4 flushed bytes unread.
//!
//! Note on `shift_low`: the canonical C is `p->low = (UInt32)p->low << 8;`
//! where the shift is performed in 32-bit arithmetic, i.e. the top byte of
//! the 32-bit low is discarded (it has just been captured in `cache` or is
//! represented by a pending `0xFF`). We implement exactly that
//! (`low = ((low as u32) << 8) as u64`), which keeps `low < 2^32` after
//! every shift and the carry in {0, 1}.

use crate::error::{BaogramError, Result};

/// Probability scale: 11 bits.
const PROB_BITS: u32 = 11;
/// 2^11; probabilities live strictly inside (0, PROB_MAX).
const PROB_MAX: u16 = 1 << PROB_BITS;
/// Initial probability: PROB_MAX / 2 (equally likely 0/1).
const PROB_INIT: u16 = PROB_MAX / 2;
/// LZMA adaptation shift.
const MOVE_BITS: u32 = 5;
/// Renormalization threshold.
const TOP: u32 = 1 << 24;
/// Number of contexts: 2^10.
const NUM_CTX: usize = 1 << 10;

// ---------------------------------------------------------------------------
// Range encoder (canonical LZMA binary rc)
// ---------------------------------------------------------------------------

struct RangeEncoder {
    low: u64,
    range: u32,
    cache: u8,
    cache_size: u64,
    out: Vec<u8>,
}

impl RangeEncoder {
    fn new() -> Self {
        RangeEncoder {
            low: 0,
            range: 0xFFFF_FFFF,
            cache: 0,
            cache_size: 1,
            out: Vec::new(),
        }
    }

    /// Canonical LZMA `RangeEnc_ShiftLow`.
    fn shift_low(&mut self) {
        if (self.low as u32) < 0xFF00_0000 || (self.low >> 32) != 0 {
            let carry = (self.low >> 32) as u8; // 0 or 1
            let mut temp = self.cache;
            // do { WriteByte(temp + carry); temp = 0xFF; } while (--cacheSize);
            loop {
                self.out.push(temp.wrapping_add(carry));
                temp = 0xFF;
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    break;
                }
            }
            self.cache = ((self.low >> 24) & 0xFF) as u8;
        }
        self.cache_size += 1;
        // Canonical C: `p->low = (UInt32)p->low << 8;` — 32-bit arithmetic.
        self.low = ((self.low as u32) << 8) as u64;
    }

    /// Encode bit `b` with probability `p` = P(bit == 0), 11-bit scale.
    fn encode_bit(&mut self, b: u8, p: u16) {
        let bound = (self.range >> PROB_BITS) * (p as u32);
        if b == 0 {
            self.range = bound;
        } else {
            self.low += bound as u64;
            self.range -= bound;
        }
        while self.range < TOP {
            self.shift_low();
            self.range <<= 8;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        for _ in 0..5 {
            self.shift_low();
        }
        self.out
    }
}

// ---------------------------------------------------------------------------
// Range decoder
// ---------------------------------------------------------------------------

struct RangeDecoder<'a> {
    range: u32,
    code: u32,
    data: &'a [u8],
    consumed: usize,
}

impl<'a> RangeDecoder<'a> {
    /// Byte 0 is the encoder's always-zero first byte; bytes 1..5 seed `code`.
    fn new(data: &'a [u8]) -> Result<Self> {
        if data.len() < 5 {
            return Err(BaogramError::CodecPrematureEnd);
        }
        let code = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
        Ok(RangeDecoder {
            range: 0xFFFF_FFFF,
            code,
            data,
            consumed: 5,
        })
    }

    fn next_byte(&mut self) -> Result<u8> {
        if self.consumed >= self.data.len() {
            return Err(BaogramError::CodecPrematureEnd);
        }
        let b = self.data[self.consumed];
        self.consumed += 1;
        Ok(b)
    }

    /// Decode one bit with probability `p` = P(bit == 0), 11-bit scale.
    fn decode_bit(&mut self, p: u16) -> Result<u8> {
        let bound = (self.range >> PROB_BITS) * (p as u32);
        let b;
        if self.code < bound {
            b = 0;
            self.range = bound;
        } else {
            b = 1;
            self.code -= bound;
            self.range -= bound;
        }
        while self.range < TOP {
            self.range <<= 8;
            self.code = (self.code << 8) | self.next_byte()? as u32;
        }
        Ok(b)
    }
}

// ---------------------------------------------------------------------------
// Context model
// ---------------------------------------------------------------------------

/// 10-bit three-line context for pixel (x, y) over an unpacked bit buffer
/// (one byte per pixel, values 0/1). Out-of-bounds reads are 0. The template
/// only references already-coded pixels, so encoder and decoder see
/// identical contexts.
#[inline]
fn context_at(bits: &[u8], width_px: usize, x: usize, y: usize) -> usize {
    let b = |dx: isize, dy: isize| -> usize {
        let xx = x as isize + dx;
        let yy = y as isize + dy;
        if xx < 0 || yy < 0 || xx >= width_px as isize {
            0
        } else {
            bits[yy as usize * width_px + xx as usize] as usize
        }
    };
    b(-2, 0)
        | b(-1, 0) << 1
        | b(-2, -1) << 2
        | b(-1, -1) << 3
        | b(0, -1) << 4
        | b(1, -1) << 5
        | b(2, -1) << 6
        | b(-1, -2) << 7
        | b(0, -2) << 8
        | b(1, -2) << 9
}

/// LZMA probability update after coding bit `b` (MOVE_BITS = 5).
#[inline]
fn update_prob(p: &mut u16, b: u8) {
    if b == 0 {
        *p += (PROB_MAX - *p) >> MOVE_BITS;
    } else {
        *p -= *p >> MOVE_BITS;
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Encode a packed Mono1 image (MSB-first row-major, bit 1 = white).
///
/// `packed` must be exactly `width_px / 8 * height_px` bytes and `width_px`
/// must be divisible by 8.
pub fn ctx_encode(packed: &[u8], width_px: usize, height_px: usize) -> Vec<u8> {
    debug_assert!(width_px % 8 == 0, "width_px must be divisible by 8");
    debug_assert_eq!(
        packed.len(),
        width_px / 8 * height_px,
        "packed input length must be width_px/8*height_px"
    );

    let row_bytes = width_px / 8;
    // Unpack to one byte per pixel for fast template access.
    let mut bits = vec![0u8; width_px * height_px];
    for y in 0..height_px {
        for x in 0..width_px {
            bits[y * width_px + x] = (packed[y * row_bytes + x / 8] >> (7 - (x % 8))) & 1;
        }
    }

    let mut probs = [PROB_INIT; NUM_CTX];
    let mut enc = RangeEncoder::new();
    for y in 0..height_px {
        for x in 0..width_px {
            let ctx = context_at(&bits, width_px, x, y);
            let bit = bits[y * width_px + x];
            enc.encode_bit(bit, probs[ctx]);
            update_prob(&mut probs[ctx], bit);
        }
    }
    enc.finish()
}

/// Decode a CtxArithMono1 stream back to the packed Mono1 image.
///
/// Errors:
/// - [`BaogramError::BadPackedSize`] if `width_px` is not divisible by 8;
/// - [`BaogramError::CodecPrematureEnd`] if the input is shorter than 5
///   bytes or ends before all pixels are decoded;
/// - [`BaogramError::CodecTrailingGarbage`] if more than 4 unread bytes
///   remain after the last pixel (the decoder legitimately leaves up to 4
///   flushed bytes unread).
pub fn ctx_decode(data: &[u8], width_px: usize, height_px: usize) -> Result<Vec<u8>> {
    if width_px % 8 != 0 {
        return Err(BaogramError::BadPackedSize);
    }
    let mut dec = RangeDecoder::new(data)?;
    let mut bits = vec![0u8; width_px * height_px];
    let mut probs = [PROB_INIT; NUM_CTX];
    for y in 0..height_px {
        for x in 0..width_px {
            let ctx = context_at(&bits, width_px, x, y);
            let bit = dec.decode_bit(probs[ctx])?;
            update_prob(&mut probs[ctx], bit);
            bits[y * width_px + x] = bit;
        }
    }
    if data.len() - dec.consumed > 4 {
        return Err(BaogramError::CodecTrailingGarbage);
    }

    let row_bytes = width_px / 8;
    let mut out = vec![0u8; row_bytes * height_px];
    for y in 0..height_px {
        for x in 0..width_px {
            if bits[y * width_px + x] != 0 {
                out[y * row_bytes + x / 8] |= 1 << (7 - (x % 8));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    // ---- cross-language pins (computed by this Rust implementation and
    // ---- hardcoded identically in tools/baogram-host/tests/test_ctxcodec.py)
    const PIN_NOISE_128_LEN: usize = 1933;
    const PIN_NOISE_128_SHA256: &str =
        "f97cbd9eb5ccd46628ca7e1e369b52a74df9219de0e1e2c4bb102f54a76b9201";
    const PIN_ZERO_128_LEN: usize = 50;
    const PIN_ZERO_128_SHA256: &str =
        "cc2786e1f9910a9d811400edcddaf7075195f7a16b216dcbefba3bc7c4f2ae51";
    const PIN_CHECKER_128_FIRST16: &str = "0056cf5fd2ac42309497985172fc1a4a";

    fn sha256_hex(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        hex::encode(h.finalize())
    }

    /// xorshift32, seed 0x1234_5678, shifts 13/17/5, low byte — the
    /// deterministic noise generator shared with codec.rs and the Python peer.
    fn xorshift_bytes(n: usize) -> Vec<u8> {
        let mut state = 0x1234_5678u32;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            v.push((state & 0xff) as u8);
        }
        v
    }

    fn checkerboard(w: usize, h: usize) -> Vec<u8> {
        // pixel(x, y) = (x + y) & 1: rows alternate 0x55 / 0xAA.
        let mut img = Vec::with_capacity(w / 8 * h);
        for y in 0..h {
            let byte = if y % 2 == 0 { 0x55u8 } else { 0xAAu8 };
            img.extend(std::iter::repeat(byte).take(w / 8));
        }
        img
    }

    fn horizontal_bands(w: usize, h: usize) -> Vec<u8> {
        // 16-row bands, first band black (0).
        let mut img = Vec::with_capacity(w / 8 * h);
        for y in 0..h {
            let byte = if (y / 16) % 2 == 0 { 0x00u8 } else { 0xFFu8 };
            img.extend(std::iter::repeat(byte).take(w / 8));
        }
        img
    }

    fn vertical_bands(w: usize, h: usize) -> Vec<u8> {
        // 16-px-wide bands, first band black (0): bytes 00 00 FF FF ...
        let mut img = Vec::with_capacity(w / 8 * h);
        for _y in 0..h {
            for xb in 0..w / 8 {
                img.push(if (xb / 2) % 2 == 0 { 0x00u8 } else { 0xFFu8 });
            }
        }
        img
    }

    fn roundtrip(img: &[u8], w: usize, h: usize) -> usize {
        let enc = ctx_encode(img, w, h);
        let dec = ctx_decode(&enc, w, h).expect("decode must succeed");
        assert_eq!(dec, img, "roundtrip mismatch at {}x{}", w, h);
        enc.len()
    }

    #[test]
    fn roundtrip_flat_images() {
        for &(w, h) in &[(128usize, 120usize), (256usize, 240usize)] {
            let n = w / 8 * h;
            roundtrip(&vec![0x00u8; n], w, h);
            roundtrip(&vec![0xFFu8; n], w, h);
        }
    }

    #[test]
    fn roundtrip_structured_images() {
        for &(w, h) in &[(128usize, 120usize), (256usize, 240usize)] {
            roundtrip(&checkerboard(w, h), w, h);
            roundtrip(&horizontal_bands(w, h), w, h);
            roundtrip(&vertical_bands(w, h), w, h);
        }
    }

    #[test]
    fn roundtrip_noise() {
        for &(w, h) in &[(128usize, 120usize), (256usize, 240usize)] {
            roundtrip(&xorshift_bytes(w / 8 * h), w, h);
        }
    }

    #[test]
    fn compression_sanity() {
        let zero = vec![0u8; 1920];
        let enc_zero = ctx_encode(&zero, 128, 120);
        assert!(
            enc_zero.len() < 64,
            "all-zero 128x120 must encode to < 64 bytes, got {}",
            enc_zero.len()
        );

        let noise = xorshift_bytes(1920);
        let enc_noise = ctx_encode(&noise, 128, 120);
        assert!(
            (enc_noise.len() as f64) <= 1920.0 * 1.1,
            "noise must stay <= ~1.1x input, got {} for 1920",
            enc_noise.len()
        );
    }

    #[test]
    fn rejects_truncated_stream() {
        let img = xorshift_bytes(1920);
        let enc = ctx_encode(&img, 128, 120);
        // Chop enough that the decoder must run out mid-image (the last 4
        // flushed bytes are legitimately unread, so remove more than that).
        let truncated = &enc[..enc.len() - 8];
        assert_eq!(
            ctx_decode(truncated, 128, 120).unwrap_err(),
            BaogramError::CodecPrematureEnd
        );
    }

    #[test]
    fn rejects_trailing_garbage() {
        let img = xorshift_bytes(1920);
        let mut enc = ctx_encode(&img, 128, 120);
        enc.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x99]);
        assert_eq!(
            ctx_decode(&enc, 128, 120).unwrap_err(),
            BaogramError::CodecTrailingGarbage
        );
    }

    #[test]
    fn rejects_short_input() {
        for len in 0..5usize {
            assert_eq!(
                ctx_decode(&vec![0u8; len], 128, 120).unwrap_err(),
                BaogramError::CodecPrematureEnd,
                "len {} must be premature end",
                len
            );
        }
    }

    #[test]
    fn rejects_bad_dimensions() {
        assert_eq!(
            ctx_decode(&[0u8; 16], 100, 120).unwrap_err(),
            BaogramError::BadPackedSize
        );
    }

    #[test]
    fn cross_language_pins() {
        let noise = xorshift_bytes(1920);
        let enc_noise = ctx_encode(&noise, 128, 120);
        assert_eq!(
            (enc_noise.len(), sha256_hex(&enc_noise)),
            (PIN_NOISE_128_LEN, PIN_NOISE_128_SHA256.to_string()),
            "noise 128x120 pin mismatch"
        );

        let zero = vec![0u8; 1920];
        let enc_zero = ctx_encode(&zero, 128, 120);
        assert_eq!(
            (enc_zero.len(), sha256_hex(&enc_zero)),
            (PIN_ZERO_128_LEN, PIN_ZERO_128_SHA256.to_string()),
            "all-zero 128x120 pin mismatch"
        );

        let checker = checkerboard(128, 120);
        let enc_checker = ctx_encode(&checker, 128, 120);
        assert_eq!(
            hex::encode(&enc_checker[..16]),
            PIN_CHECKER_128_FIRST16,
            "checkerboard 128x120 first-16 pin mismatch"
        );
    }

    #[test]
    fn timing_encode_noise_256x240() {
        let noise = xorshift_bytes(7680);
        let start = std::time::Instant::now();
        let enc = ctx_encode(&noise, 256, 240);
        let elapsed = start.elapsed();
        println!(
            "ctxcodec: encode 256x240 noise ({} -> {} bytes) took {:?}",
            7680,
            enc.len(),
            elapsed
        );
    }
}
