//! Lossless codecs for packed Mono1 image data.
//!
//! Two codecs exist in protocol version 1:
//!
//! * `RawMono1` (0): the 7,680 packed bytes verbatim.
//! * `PackBitsMono1` (1): a deterministic PackBits-style byte run/literal
//!   codec, defined below.
//!
//! # PackBitsMono1 stream definition
//!
//! The stream is a sequence of blocks. Each block starts with a control
//! byte `c`:
//!
//! * `c` in `0x00..=0x7F`: literal block — the next `c + 1` bytes are copied
//!   to the output verbatim (1..=128 bytes).
//! * `c` in `0x81..=0xFF`: run block — the next single byte is repeated
//!   `257 - c` times (2..=128 repetitions).
//! * `c == 0x80`: invalid; decoders MUST reject it.
//!
//! The canonical **encoder** is deterministic: scan left to right; a run of
//! 3 or more identical bytes (capped at 128) is emitted as a run block;
//! anything else accumulates into literal blocks capped at 128 bytes.
//! Decoders accept any well-formed stream (including 2-byte runs), but the
//! Rust and Python encoders produce byte-identical output by construction.
//!
//! The decoder requires the output to be exactly `expected_len` bytes:
//! premature end of input, output overflow, output underflow, the reserved
//! `0x80` control byte, and trailing input after completion are all errors.

use crate::error::{BaogramError, Result};

/// Codec identifier for raw packed Mono1 (no compression).
pub const CODEC_RAW_MONO1: u8 = 0;
/// Codec identifier for the deterministic PackBits variant defined here.
pub const CODEC_PACKBITS_MONO1: u8 = 1;

/// Deterministically compress `input` with the canonical PackBitsMono1
/// encoder. The output is only useful if it is smaller than the input;
/// callers should use [`encode_best`] for the compress-or-raw decision.
pub fn packbits_encode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() / 2 + 16);
    let mut i = 0usize;
    let n = input.len();
    let mut literal_start = usize::MAX; // MAX = no literal block open

    while i < n {
        // measure the run starting at i, capped at 128
        let b = input[i];
        let mut run = 1usize;
        while run < 128 && i + run < n && input[i + run] == b {
            run += 1;
        }
        if run >= 3 {
            // flush any open literal block first
            if literal_start != usize::MAX {
                flush_literal(&mut out, &input[literal_start..i]);
                literal_start = usize::MAX;
            }
            out.push((257 - run) as u8);
            out.push(b);
            i += run;
        } else {
            if literal_start == usize::MAX {
                literal_start = i;
            }
            i += run;
            // cap literal blocks at 128 bytes
            if i - literal_start >= 128 {
                flush_literal(&mut out, &input[literal_start..literal_start + 128]);
                literal_start += 128;
                if literal_start == i {
                    literal_start = usize::MAX;
                }
            }
        }
    }
    if literal_start != usize::MAX {
        flush_literal(&mut out, &input[literal_start..n]);
    }
    out
}

fn flush_literal(out: &mut Vec<u8>, lit: &[u8]) {
    debug_assert!(!lit.is_empty() && lit.len() <= 128);
    out.push((lit.len() - 1) as u8);
    out.extend_from_slice(lit);
}

/// Decode a PackBitsMono1 stream, requiring exactly `expected_len` output
/// bytes and no unused input.
pub fn packbits_decode(input: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected_len);
    let mut i = 0usize;
    while i < input.len() {
        if out.len() == expected_len {
            // input remains but output is already complete
            return Err(BaogramError::CodecTrailingGarbage);
        }
        let c = input[i];
        i += 1;
        if c == 0x80 {
            return Err(BaogramError::CodecInvalidRun);
        } else if c < 0x80 {
            let count = c as usize + 1;
            if i + count > input.len() {
                return Err(BaogramError::CodecPrematureEnd);
            }
            if out.len() + count > expected_len {
                return Err(BaogramError::CodecOutputOverflow);
            }
            out.extend_from_slice(&input[i..i + count]);
            i += count;
        } else {
            let count = 257 - c as usize;
            if i >= input.len() {
                return Err(BaogramError::CodecPrematureEnd);
            }
            if out.len() + count > expected_len {
                return Err(BaogramError::CodecOutputOverflow);
            }
            let b = input[i];
            i += 1;
            out.resize(out.len() + count, b);
        }
    }
    if out.len() < expected_len {
        return Err(BaogramError::CodecPrematureEnd);
    }
    Ok(out)
}

/// Encode packed image bytes with the best available codec.
///
/// Returns `(codec_id, encoded_bytes)`. The compressed form is selected only
/// when it is strictly smaller than the raw input; otherwise the raw bytes
/// are used. This rule is part of the canonical format: parsers reject
/// PackBits-coded posts whose encoded length is not smaller than raw.
pub fn encode_best(packed: &[u8]) -> (u8, Vec<u8>) {
    let compressed = packbits_encode(packed);
    if compressed.len() < packed.len() {
        (CODEC_PACKBITS_MONO1, compressed)
    } else {
        (CODEC_RAW_MONO1, packed.to_vec())
    }
}

/// Decode `encoded` according to `codec`, requiring exactly `expected_len`
/// output bytes.
pub fn decode(codec: u8, encoded: &[u8], expected_len: usize) -> Result<Vec<u8>> {
    match codec {
        CODEC_RAW_MONO1 => {
            if encoded.len() != expected_len {
                return Err(BaogramError::NoncanonicalLength);
            }
            Ok(encoded.to_vec())
        }
        CODEC_PACKBITS_MONO1 => {
            // canonical rule: compressed data must be strictly smaller than raw
            if encoded.len() >= expected_len {
                return Err(BaogramError::NoncanonicalLength);
            }
            packbits_decode(encoded, expected_len)
        }
        _ => Err(BaogramError::UnknownCodec),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::MONO1_PACKED_LEN;

    fn roundtrip(data: &[u8]) -> usize {
        let enc = packbits_encode(data);
        let dec = packbits_decode(&enc, data.len()).unwrap();
        assert_eq!(dec, data);
        enc.len()
    }

    #[test]
    fn all_white() {
        let n = roundtrip(&[0xffu8; MONO1_PACKED_LEN]);
        // 7680 / 128 = 60 run blocks of 2 bytes each
        assert_eq!(n, 120);
    }

    #[test]
    fn all_black() {
        let n = roundtrip(&[0x00u8; MONO1_PACKED_LEN]);
        assert_eq!(n, 120);
    }

    #[test]
    fn checkerboard_pixels() {
        // 1-px checkerboard packs to rows of 0xAA / 0x55: long byte runs
        let mut img = Vec::with_capacity(MONO1_PACKED_LEN);
        for row in 0..240 {
            let fill = if row % 2 == 0 { 0xAAu8 } else { 0x55u8 };
            img.extend_from_slice(&[fill; 32]);
        }
        let n = roundtrip(&img);
        assert!(n < MONO1_PACKED_LEN / 10, "checkerboard should compress well, got {}", n);
    }

    #[test]
    fn horizontal_bands() {
        let mut img = Vec::with_capacity(MONO1_PACKED_LEN);
        for row in 0..240 {
            let fill = if (row / 16) % 2 == 0 { 0xffu8 } else { 0x00u8 };
            img.extend_from_slice(&[fill; 32]);
        }
        let n = roundtrip(&img);
        assert!(n <= 128, "horizontal bands are one giant run per band, got {}", n);
    }

    #[test]
    fn vertical_bands() {
        // vertical 8-px bands: bytes alternate ff/00 within each row
        let mut img = Vec::with_capacity(MONO1_PACKED_LEN);
        for _row in 0..240 {
            for byte in 0..32 {
                img.push(if byte % 2 == 0 { 0xff } else { 0x00 });
            }
        }
        let n = roundtrip(&img);
        // alternating single bytes never form runs >= 3: worst case literals
        assert!(n > MONO1_PACKED_LEN, "alternating bytes should not compress, got {}", n);
        // and encode_best must therefore fall back to raw
        let (codec, enc) = encode_best(&img);
        assert_eq!(codec, CODEC_RAW_MONO1);
        assert_eq!(enc.len(), MONO1_PACKED_LEN);
    }

    #[test]
    fn deterministic_pseudo_random() {
        // xorshift32 PRNG, seed 0x1234_5678 — reproduced in the Python peer
        let mut state = 0x1234_5678u32;
        let mut img = Vec::with_capacity(MONO1_PACKED_LEN);
        for _ in 0..MONO1_PACKED_LEN {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            img.push((state & 0xff) as u8);
        }
        let n = roundtrip(&img);
        // random bytes rarely run; expect near-incompressible
        assert!(n >= MONO1_PACKED_LEN, "random data should not compress, got {}", n);
    }

    #[test]
    fn two_byte_runs_stay_literal() {
        // aabb ccdd ... 2-byte runs join literals under the >=3 rule
        let data = [0x11, 0x11, 0x22, 0x22, 0x33, 0x33];
        let enc = packbits_encode(&data);
        assert_eq!(enc, vec![0x05, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33]);
        assert_eq!(packbits_decode(&enc, 6).unwrap(), data);
    }

    #[test]
    fn run_then_literal() {
        let data = [7u8, 7, 7, 7, 1, 2, 3];
        let enc = packbits_encode(&data);
        // run of 4 -> control 253; literal of 3 -> control 2
        assert_eq!(enc, vec![253, 7, 2, 1, 2, 3]);
        assert_eq!(packbits_decode(&enc, 7).unwrap(), data);
    }

    #[test]
    fn max_length_runs_and_literals() {
        // 300 identical bytes: 128 + 128 + 44
        let data = vec![9u8; 300];
        let enc = packbits_encode(&data);
        assert_eq!(enc, vec![129, 9, 129, 9, 213, 9]);
        assert_eq!(packbits_decode(&enc, 300).unwrap(), data);

        // 130 non-running bytes: literal 128 + literal 2
        let data: Vec<u8> = (0..130u32).map(|i| (i % 251) as u8).collect();
        let enc = packbits_encode(&data);
        assert_eq!(enc[0], 127);
        assert_eq!(enc[129], 1);
        assert_eq!(enc.len(), 132);
        assert_eq!(packbits_decode(&enc, 130).unwrap(), data);
    }

    #[test]
    fn truncated_input_rejected() {
        // literal control byte promising 4 bytes, only 2 present
        assert_eq!(packbits_decode(&[3, 1, 2], 4).unwrap_err(), BaogramError::CodecPrematureEnd);
        // run control byte with no value byte
        assert_eq!(packbits_decode(&[254], 3).unwrap_err(), BaogramError::CodecPrematureEnd);
        // empty input but nonzero expectation
        assert_eq!(packbits_decode(&[], 1).unwrap_err(), BaogramError::CodecPrematureEnd);
    }

    #[test]
    fn malformed_run_rejected() {
        assert_eq!(packbits_decode(&[0x80, 0], 1).unwrap_err(), BaogramError::CodecInvalidRun);
    }

    #[test]
    fn output_overflow_rejected() {
        // run of 128 into an expected length of 10
        assert_eq!(packbits_decode(&[129, 5], 10).unwrap_err(), BaogramError::CodecOutputOverflow);
        // literal of 4 into expected 2
        assert_eq!(packbits_decode(&[3, 1, 2, 3, 4], 2).unwrap_err(), BaogramError::CodecOutputOverflow);
    }

    #[test]
    fn trailing_garbage_rejected() {
        // valid 3-byte run, then an extra block
        assert_eq!(packbits_decode(&[254, 7, 0, 1], 3).unwrap_err(), BaogramError::CodecTrailingGarbage);
    }

    #[test]
    fn compression_larger_than_raw_falls_back() {
        let mut state = 0xdeadbeefu32;
        let mut img = Vec::with_capacity(MONO1_PACKED_LEN);
        for _ in 0..MONO1_PACKED_LEN {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            img.push((state & 0xff) as u8);
        }
        let (codec, enc) = encode_best(&img);
        assert_eq!(codec, CODEC_RAW_MONO1);
        assert_eq!(enc, img);
        // and decode() enforces the canonicality rule for PackBits
        assert_eq!(
            decode(CODEC_PACKBITS_MONO1, &vec![0u8; MONO1_PACKED_LEN], MONO1_PACKED_LEN).unwrap_err(),
            BaogramError::NoncanonicalLength
        );
    }

    #[test]
    fn unknown_codec_rejected() {
        assert_eq!(decode(9, &[], 0).unwrap_err(), BaogramError::UnknownCodec);
    }
}
