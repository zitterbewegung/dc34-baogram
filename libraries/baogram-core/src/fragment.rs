//! Fragmenting a serialized post into QR-sized pieces, and reassembling
//! them in any order.
//!
//! Two wire versions share the `BG` magic:
//!
//! * **v1** (`Fragment`): plain uniform chunking; the receiver needs every
//!   chunk exactly once, so a missed frame costs a full display loop.
//! * **v2** (`FountainFrame`): rateless fountain coding. Frames `0..k` are
//!   the source chunks in order (systematic prefix); every later frame is
//!   the XOR of a deterministically chosen chunk subset, so any caught
//!   frame is useful and the stream never repeats. See `fountain_indices`
//!   for the exact PRNG/degree specification shared with the Python peer.
//!
//! Fragments are binary structures, Base45-encoded for the QR text layer
//! (Base45 maps efficiently onto QR alphanumeric mode). All integers are
//! **big-endian**.
//!
//! ```text
//! offset  len  field
//! ------  ---  -----
//!      0    2  magic "BG"
//!      2    1  version         (= 1)
//!      3    1  flags           (= 0; reserved bits must be zero)
//!      4    8  short_post_id   (first 8 bytes of the post digest)
//!     12    2  frag_index      (0-based)
//!     14    2  frag_count      (1..=MAX_FRAGMENT_COUNT)
//!     16    4  total_len       (serialized post length, <= MAX_POST_BYTES)
//!     20    2  chunk_size      (payload bytes per non-final fragment)
//!     22    2  payload_len
//!     24    n  payload         (n = payload_len)
//!   24+n    4  crc32           (CRC-32/ISO-HDLC over bytes 0..24+n)
//! ```
//!
//! Chunking is uniform: fragment `i` carries post bytes
//! `[i * chunk_size, min((i+1) * chunk_size, total_len))`. The geometry is
//! over-determined on purpose — `frag_count` must equal
//! `ceil(total_len / chunk_size)` and `payload_len` must match exactly —
//! so a receiver can validate every fragment in isolation before touching
//! its reassembly buffer.

use crate::crypto::SHORT_ID_LEN;
use crate::error::{BaogramError, Result};
use crate::post::MAX_POST_BYTES;

/// Fragment container magic.
pub const FRAGMENT_MAGIC: [u8; 2] = *b"BG";
/// Fragment protocol version.
pub const FRAGMENT_VERSION: u8 = 1;
/// Fixed header length before the payload.
pub const FRAGMENT_HEADER_LEN: usize = 24;
/// CRC length.
pub const FRAGMENT_CRC_LEN: usize = 4;
/// Upper bound on fragment count (fixed 512-byte receive bitmap).
pub const MAX_FRAGMENT_COUNT: usize = 4096;
/// Upper bound on the chunk size a sender may use.
pub const MAX_FRAGMENT_PAYLOAD: usize = 1024;
/// Default payload size used by the badge sender.
pub const DEFAULT_FRAGMENT_PAYLOAD: usize = 64;

/// CRC-32/ISO-HDLC (the zlib/PNG polynomial, reflected, init and xorout
/// 0xFFFFFFFF). Implemented locally so the core library carries no
/// dependency for it; checked against the standard test vector.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// One parsed (or to-be-encoded) fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub short_post_id: [u8; SHORT_ID_LEN],
    pub frag_index: u16,
    pub frag_count: u16,
    pub total_len: u32,
    pub chunk_size: u16,
    pub payload: Vec<u8>,
}

impl Fragment {
    /// Serialize to binary (header + payload + CRC).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(FRAGMENT_HEADER_LEN + self.payload.len() + FRAGMENT_CRC_LEN);
        out.extend_from_slice(&FRAGMENT_MAGIC);
        out.push(FRAGMENT_VERSION);
        out.push(0); // flags
        out.extend_from_slice(&self.short_post_id);
        out.extend_from_slice(&self.frag_index.to_be_bytes());
        out.extend_from_slice(&self.frag_count.to_be_bytes());
        out.extend_from_slice(&self.total_len.to_be_bytes());
        out.extend_from_slice(&self.chunk_size.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.payload);
        let crc = crc32(&out);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    }

    /// Base45 text for the QR layer.
    pub fn to_base45(&self) -> String {
        base45::encode(&self.to_bytes())
    }

    /// Parse and validate a binary fragment.
    ///
    /// Checks, in order: minimum length, magic, version, flags, CRC,
    /// geometry (count/chunk/total bounds and consistency), index range,
    /// payload length, and exact container length.
    pub fn from_bytes(bytes: &[u8]) -> Result<Fragment> {
        if bytes.len() < FRAGMENT_HEADER_LEN + FRAGMENT_CRC_LEN {
            return Err(BaogramError::Truncated);
        }
        if bytes[0..2] != FRAGMENT_MAGIC {
            return Err(BaogramError::BadMagic);
        }
        if bytes[2] != FRAGMENT_VERSION {
            return Err(BaogramError::UnknownVersion);
        }
        if bytes[3] != 0 {
            return Err(BaogramError::UnknownFlags);
        }
        // CRC covers everything except the trailing 4 CRC bytes
        let crc_off = bytes.len() - FRAGMENT_CRC_LEN;
        let stored_crc = u32::from_be_bytes(bytes[crc_off..].try_into().unwrap());
        if crc32(&bytes[..crc_off]) != stored_crc {
            return Err(BaogramError::FragmentCrcMismatch);
        }
        let short_post_id: [u8; SHORT_ID_LEN] = bytes[4..12].try_into().unwrap();
        let frag_index = u16::from_be_bytes(bytes[12..14].try_into().unwrap());
        let frag_count = u16::from_be_bytes(bytes[14..16].try_into().unwrap());
        let total_len = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let chunk_size = u16::from_be_bytes(bytes[20..22].try_into().unwrap());
        let payload_len = u16::from_be_bytes(bytes[22..24].try_into().unwrap()) as usize;

        validate_geometry(frag_count, total_len, chunk_size)?;
        if frag_index >= frag_count {
            return Err(BaogramError::FragmentIndexOutOfRange);
        }
        let expected_payload = expected_payload_len(frag_index, frag_count, total_len, chunk_size);
        if payload_len != expected_payload {
            return Err(BaogramError::FragmentPayloadLenInvalid);
        }
        let expected_total = FRAGMENT_HEADER_LEN + payload_len + FRAGMENT_CRC_LEN;
        if bytes.len() < expected_total {
            return Err(BaogramError::Truncated);
        }
        if bytes.len() > expected_total {
            return Err(BaogramError::TrailingBytes);
        }
        let payload = bytes[FRAGMENT_HEADER_LEN..FRAGMENT_HEADER_LEN + payload_len].to_vec();
        Ok(Fragment { short_post_id, frag_index, frag_count, total_len, chunk_size, payload })
    }

    /// Parse a Base45 QR string into a validated fragment.
    pub fn from_base45(text: &str) -> Result<Fragment> {
        let bytes = base45::decode(text).map_err(|_| BaogramError::Base45Decode)?;
        Fragment::from_bytes(&bytes)
    }
}

/// Validate the (count, total, chunk) triple for self-consistency and bounds.
fn validate_geometry(frag_count: u16, total_len: u32, chunk_size: u16) -> Result<()> {
    if frag_count == 0 || frag_count as usize > MAX_FRAGMENT_COUNT {
        return Err(BaogramError::FragmentCountOutOfRange);
    }
    if chunk_size == 0 || chunk_size as usize > MAX_FRAGMENT_PAYLOAD {
        return Err(BaogramError::FragmentChunkSizeInvalid);
    }
    if total_len == 0 || total_len as usize > MAX_POST_BYTES {
        return Err(BaogramError::FragmentGeometryInvalid);
    }
    let expected_count = (total_len as usize).div_ceil(chunk_size as usize);
    if expected_count != frag_count as usize {
        return Err(BaogramError::FragmentGeometryInvalid);
    }
    Ok(())
}

fn expected_payload_len(index: u16, count: u16, total_len: u32, chunk_size: u16) -> usize {
    if index + 1 < count {
        chunk_size as usize
    } else {
        // final fragment: the remainder, in 1..=chunk_size
        total_len as usize - (count as usize - 1) * chunk_size as usize
    }
}

/// Split a serialized post into fragments with a uniform chunk size.
///
/// Prefer [`FragmentIter`] on the badge — it materializes one fragment at a
/// time instead of holding all of them.
pub fn fragment_post(
    short_post_id: &[u8; SHORT_ID_LEN],
    serialized_post: &[u8],
    chunk_size: usize,
) -> Result<Vec<Fragment>> {
    FragmentIter::new(short_post_id, serialized_post, chunk_size).map(|it| it.collect())
}

/// Lazy fragment generator: yields one fragment at a time so the sender
/// never holds more than one fragment (plus the serialized post) in memory.
#[derive(Debug)]
pub struct FragmentIter<'a> {
    short_post_id: [u8; SHORT_ID_LEN],
    post: &'a [u8],
    chunk_size: usize,
    count: usize,
    next_index: usize,
}

impl<'a> FragmentIter<'a> {
    pub fn new(
        short_post_id: &[u8; SHORT_ID_LEN],
        serialized_post: &'a [u8],
        chunk_size: usize,
    ) -> Result<Self> {
        if chunk_size == 0 || chunk_size > MAX_FRAGMENT_PAYLOAD {
            return Err(BaogramError::FragmentChunkSizeInvalid);
        }
        if serialized_post.is_empty() || serialized_post.len() > MAX_POST_BYTES {
            return Err(BaogramError::FragmentGeometryInvalid);
        }
        let count = serialized_post.len().div_ceil(chunk_size);
        if count > MAX_FRAGMENT_COUNT {
            return Err(BaogramError::FragmentCountOutOfRange);
        }
        Ok(FragmentIter {
            short_post_id: *short_post_id,
            post: serialized_post,
            chunk_size,
            count,
            next_index: 0,
        })
    }

    /// Total number of fragments this iterator will yield.
    pub fn count_total(&self) -> usize {
        self.count
    }

    /// Build the fragment at an arbitrary index (for looping displays).
    pub fn fragment_at(&self, index: usize) -> Option<Fragment> {
        if index >= self.count {
            return None;
        }
        let start = index * self.chunk_size;
        let end = (start + self.chunk_size).min(self.post.len());
        Some(Fragment {
            short_post_id: self.short_post_id,
            frag_index: index as u16,
            frag_count: self.count as u16,
            total_len: self.post.len() as u32,
            chunk_size: self.chunk_size as u16,
            payload: self.post[start..end].to_vec(),
        })
    }
}

impl<'a> Iterator for FragmentIter<'a> {
    type Item = Fragment;

    fn next(&mut self) -> Option<Fragment> {
        let f = self.fragment_at(self.next_index)?;
        self.next_index += 1;
        Some(f)
    }
}

/// Reassembles fragments arriving in any order into a serialized post.
///
/// Memory is bounded: the buffer is allocated once from the validated
/// `total_len` (<= MAX_POST_BYTES) and the received-set is a fixed bitmap
/// of MAX_FRAGMENT_COUNT bits.
pub struct Reassembler {
    short_post_id: [u8; SHORT_ID_LEN],
    frag_count: u16,
    total_len: u32,
    chunk_size: u16,
    buffer: Vec<u8>,
    received: [u8; MAX_FRAGMENT_COUNT / 8],
    received_count: usize,
}

/// Outcome of feeding one fragment to the reassembler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedResult {
    /// New fragment accepted; (received, total).
    Accepted { received: usize, total: usize },
    /// Exact duplicate of an already-received fragment; ignored.
    Duplicate,
    /// All fragments have been received.
    Complete,
}

impl Reassembler {
    /// Start a transfer from the first validated fragment received.
    pub fn new(first: &Fragment) -> Result<(Reassembler, FeedResult)> {
        // Fragment::from_bytes has validated geometry already, but re-check
        // so a hand-constructed Fragment cannot bypass the bounds.
        validate_geometry(first.frag_count, first.total_len, first.chunk_size)?;
        let mut r = Reassembler {
            short_post_id: first.short_post_id,
            frag_count: first.frag_count,
            total_len: first.total_len,
            chunk_size: first.chunk_size,
            buffer: vec![0u8; first.total_len as usize],
            received: [0u8; MAX_FRAGMENT_COUNT / 8],
            received_count: 0,
        };
        let res = r.feed(first)?;
        Ok((r, res))
    }

    /// Feed one fragment. Duplicates (same index, same bytes) are ignored;
    /// conflicting duplicates or any disagreement in transfer parameters
    /// fail the transfer with `FragmentConflict`.
    pub fn feed(&mut self, frag: &Fragment) -> Result<FeedResult> {
        if frag.short_post_id != self.short_post_id
            || frag.frag_count != self.frag_count
            || frag.total_len != self.total_len
            || frag.chunk_size != self.chunk_size
        {
            return Err(BaogramError::FragmentConflict);
        }
        if frag.frag_index >= self.frag_count {
            return Err(BaogramError::FragmentIndexOutOfRange);
        }
        let expected =
            expected_payload_len(frag.frag_index, self.frag_count, self.total_len, self.chunk_size);
        if frag.payload.len() != expected {
            return Err(BaogramError::FragmentPayloadLenInvalid);
        }
        let idx = frag.frag_index as usize;
        let start = idx * self.chunk_size as usize;
        let slot = &mut self.buffer[start..start + expected];
        if self.received[idx / 8] & (1 << (idx % 8)) != 0 {
            // already have this index: exact duplicate or conflict?
            return if slot == frag.payload.as_slice() {
                Ok(FeedResult::Duplicate)
            } else {
                Err(BaogramError::FragmentConflict)
            };
        }
        slot.copy_from_slice(&frag.payload);
        self.received[idx / 8] |= 1 << (idx % 8);
        self.received_count += 1;
        if self.received_count == self.frag_count as usize {
            Ok(FeedResult::Complete)
        } else {
            Ok(FeedResult::Accepted { received: self.received_count, total: self.frag_count as usize })
        }
    }

    /// Fragments received so far.
    pub fn received_count(&self) -> usize {
        self.received_count
    }

    /// Total fragments expected.
    pub fn total_count(&self) -> usize {
        self.frag_count as usize
    }

    /// The transfer's short post ID.
    pub fn short_post_id(&self) -> [u8; SHORT_ID_LEN] {
        self.short_post_id
    }

    /// True when every fragment has been received.
    pub fn is_complete(&self) -> bool {
        self.received_count == self.frag_count as usize
    }

    /// Take the reassembled post bytes. Errors unless complete.
    pub fn into_bytes(self) -> Result<Vec<u8>> {
        if !self.is_complete() {
            return Err(BaogramError::TransferIncomplete);
        }
        Ok(self.buffer)
    }
}

// ---------------------------------------------------------------------------
// BG version 2: fountain frames
// ---------------------------------------------------------------------------
//
// ```text
// offset  len  field
// ------  ---  -----
//      0    2  magic "BG"
//      2    1  version         (= 2)
//      3    1  flags           (= 0; reserved bits must be zero)
//      4    8  short_post_id
//     12    4  frame_no        (u32; systematic for frame_no < k)
//     16    2  k               (source chunk count = ceil(total_len / chunk_size))
//     18    4  total_len       (serialized post length, <= MAX_POST_BYTES)
//     22    2  chunk_size
//     24    2  payload_len     (always == chunk_size; final chunk zero-padded)
//     26    n  payload         (XOR of the selected source chunks)
//   26+n    4  crc32           (CRC-32/ISO-HDLC over bytes 0..26+n)
// ```

/// Fountain fragment protocol version.
pub const FOUNTAIN_VERSION: u8 = 2;
/// Fixed fountain header length before the payload.
pub const FOUNTAIN_HEADER_LEN: usize = 26;
/// Default payload size used by the badge fountain sender.
pub const DEFAULT_FOUNTAIN_PAYLOAD: usize = 96;
/// Decoder bound: stored unresolved coded frames (drop-oldest beyond).
const PENDING_FRAMES_CAP: usize = 512;
/// Decoder bound: total payload bytes held in unresolved coded frames.
const PENDING_BYTES_CAP: usize = 65_536;

/// The splitmix64 generator (Steele/Lea/Flood). Chosen because it is a
/// dozen integer ops, has no floating point, and is trivially identical
/// across Rust and Python.
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> SplitMix64 {
        SplitMix64 { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// The source-chunk index set carried by fountain frame `frame_no` of a
/// `k`-chunk transfer, sorted ascending. This is the normative definition
/// shared with the Python peer:
///
/// * `frame_no < k`: the single chunk `frame_no` (systematic prefix).
/// * otherwise: seed a [`SplitMix64`] with
///   `BE_u64(short_post_id) XOR u64(frame_no)`. The first draw picks the
///   degree from the fixed-point distribution `W(d) = floor(2^32 / d)`,
///   `d = 1..=k` (draw `r = next() mod sum(W)`; the degree is the smallest
///   `d` whose cumulative weight exceeds `r`). Subsequent draws pick chunk
///   indices as `next() mod k`, skipping already-chosen indices, until
///   `degree` distinct indices are chosen.
pub fn fountain_indices(short_post_id: &[u8; SHORT_ID_LEN], frame_no: u32, k: u16) -> Vec<u16> {
    debug_assert!(k >= 1 && k as usize <= MAX_FRAGMENT_COUNT);
    if (frame_no as u64) < k as u64 {
        return vec![frame_no as u16];
    }
    let mut rng = SplitMix64::new(u64::from_be_bytes(*short_post_id) ^ frame_no as u64);
    // degree draw: fixed-point 1/d weights, integer-only for portability
    let mut total: u64 = 0;
    for d in 1..=k as u64 {
        total += (1u64 << 32) / d;
    }
    let r = rng.next_u64() % total;
    let mut degree = k as usize;
    let mut cum: u64 = 0;
    for d in 1..=k as u64 {
        cum += (1u64 << 32) / d;
        if cum > r {
            degree = d as usize;
            break;
        }
    }
    // index draws: membership tracked in a fixed bitmap; the ascending scan
    // at the end yields the canonical sorted representation
    let mut chosen = [0u8; MAX_FRAGMENT_COUNT / 8];
    let mut count = 0usize;
    while count < degree {
        let idx = (rng.next_u64() % k as u64) as usize;
        if chosen[idx / 8] & (1 << (idx % 8)) == 0 {
            chosen[idx / 8] |= 1 << (idx % 8);
            count += 1;
        }
    }
    let mut out = Vec::with_capacity(degree);
    for idx in 0..k as usize {
        if chosen[idx / 8] & (1 << (idx % 8)) != 0 {
            out.push(idx as u16);
        }
    }
    out
}

/// One parsed (or to-be-encoded) fountain frame. The payload is always
/// exactly `chunk_size` bytes; the final source chunk is zero-padded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FountainFrame {
    pub short_post_id: [u8; SHORT_ID_LEN],
    pub frame_no: u32,
    pub k: u16,
    pub total_len: u32,
    pub chunk_size: u16,
    pub payload: Vec<u8>,
}

impl FountainFrame {
    /// Serialize to binary (header + payload + CRC).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(FOUNTAIN_HEADER_LEN + self.payload.len() + FRAGMENT_CRC_LEN);
        out.extend_from_slice(&FRAGMENT_MAGIC);
        out.push(FOUNTAIN_VERSION);
        out.push(0); // flags
        out.extend_from_slice(&self.short_post_id);
        out.extend_from_slice(&self.frame_no.to_be_bytes());
        out.extend_from_slice(&self.k.to_be_bytes());
        out.extend_from_slice(&self.total_len.to_be_bytes());
        out.extend_from_slice(&self.chunk_size.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.payload);
        let crc = crc32(&out);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    }

    /// Base45 text for the QR layer.
    pub fn to_base45(&self) -> String {
        base45::encode(&self.to_bytes())
    }

    /// Parse and validate a binary fountain frame. Mirrors the v1 checks;
    /// the payload must always be exactly `chunk_size` bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<FountainFrame> {
        if bytes.len() < FOUNTAIN_HEADER_LEN + FRAGMENT_CRC_LEN {
            return Err(BaogramError::Truncated);
        }
        if bytes[0..2] != FRAGMENT_MAGIC {
            return Err(BaogramError::BadMagic);
        }
        if bytes[2] != FOUNTAIN_VERSION {
            return Err(BaogramError::UnknownVersion);
        }
        if bytes[3] != 0 {
            return Err(BaogramError::UnknownFlags);
        }
        let crc_off = bytes.len() - FRAGMENT_CRC_LEN;
        let stored_crc = u32::from_be_bytes(bytes[crc_off..].try_into().unwrap());
        if crc32(&bytes[..crc_off]) != stored_crc {
            return Err(BaogramError::FragmentCrcMismatch);
        }
        let short_post_id: [u8; SHORT_ID_LEN] = bytes[4..12].try_into().unwrap();
        let frame_no = u32::from_be_bytes(bytes[12..16].try_into().unwrap());
        let k = u16::from_be_bytes(bytes[16..18].try_into().unwrap());
        let total_len = u32::from_be_bytes(bytes[18..22].try_into().unwrap());
        let chunk_size = u16::from_be_bytes(bytes[22..24].try_into().unwrap());
        let payload_len = u16::from_be_bytes(bytes[24..26].try_into().unwrap()) as usize;

        validate_geometry(k, total_len, chunk_size)?;
        if payload_len != chunk_size as usize {
            return Err(BaogramError::FragmentPayloadLenInvalid);
        }
        let expected_total = FOUNTAIN_HEADER_LEN + payload_len + FRAGMENT_CRC_LEN;
        if bytes.len() < expected_total {
            return Err(BaogramError::Truncated);
        }
        if bytes.len() > expected_total {
            return Err(BaogramError::TrailingBytes);
        }
        let payload = bytes[FOUNTAIN_HEADER_LEN..FOUNTAIN_HEADER_LEN + payload_len].to_vec();
        Ok(FountainFrame { short_post_id, frame_no, k, total_len, chunk_size, payload })
    }

    /// Parse a Base45 QR string into a validated fountain frame.
    pub fn from_base45(text: &str) -> Result<FountainFrame> {
        let bytes = base45::decode(text).map_err(|_| BaogramError::Base45Decode)?;
        FountainFrame::from_bytes(&bytes)
    }
}

/// Endless fountain-frame generator over a serialized post. One frame is
/// materialized at a time; the stream never repeats and never ends.
#[derive(Debug)]
pub struct FountainIter<'a> {
    short_post_id: [u8; SHORT_ID_LEN],
    post: &'a [u8],
    chunk_size: usize,
    k: usize,
    next_frame: u32,
}

impl<'a> FountainIter<'a> {
    pub fn new(
        short_post_id: &[u8; SHORT_ID_LEN],
        serialized_post: &'a [u8],
        chunk_size: usize,
    ) -> Result<Self> {
        if chunk_size == 0 || chunk_size > MAX_FRAGMENT_PAYLOAD {
            return Err(BaogramError::FragmentChunkSizeInvalid);
        }
        if serialized_post.is_empty() || serialized_post.len() > MAX_POST_BYTES {
            return Err(BaogramError::FragmentGeometryInvalid);
        }
        let k = serialized_post.len().div_ceil(chunk_size);
        if k > MAX_FRAGMENT_COUNT {
            return Err(BaogramError::FragmentCountOutOfRange);
        }
        Ok(FountainIter { short_post_id: *short_post_id, post: serialized_post, chunk_size, k, next_frame: 0 })
    }

    /// Source chunk count (a loss-free receiver completes in exactly k frames).
    pub fn chunk_count(&self) -> usize {
        self.k
    }

    /// Build the frame for an arbitrary frame number.
    pub fn frame_at(&self, frame_no: u32) -> FountainFrame {
        let indices = fountain_indices(&self.short_post_id, frame_no, self.k as u16);
        let mut payload = vec![0u8; self.chunk_size];
        for &idx in &indices {
            let start = idx as usize * self.chunk_size;
            let end = (start + self.chunk_size).min(self.post.len());
            for (dst, src) in payload.iter_mut().zip(self.post[start..end].iter()) {
                *dst ^= src;
            }
        }
        FountainFrame {
            short_post_id: self.short_post_id,
            frame_no,
            k: self.k as u16,
            total_len: self.post.len() as u32,
            chunk_size: self.chunk_size as u16,
            payload,
        }
    }
}

impl<'a> Iterator for FountainIter<'a> {
    type Item = FountainFrame;

    fn next(&mut self) -> Option<FountainFrame> {
        let f = self.frame_at(self.next_frame);
        self.next_frame = self.next_frame.wrapping_add(1);
        Some(f)
    }
}

/// A coded frame the peeling decoder cannot resolve yet.
struct PendingFrame {
    /// Unresolved chunk indices, sorted ascending.
    indices: Vec<u16>,
    payload: Vec<u8>,
}

/// Peeling decoder for fountain frames arriving in any order.
///
/// Memory is bounded: the chunk buffer is allocated once from the validated
/// geometry (k * chunk_size < MAX_POST_BYTES + MAX_FRAGMENT_PAYLOAD), the
/// resolved set is a fixed bitmap, and unresolved coded frames are capped
/// at PENDING_FRAMES_CAP frames / PENDING_BYTES_CAP payload bytes with
/// oldest-first eviction (eviction only costs efficiency, never
/// correctness). Corrupt-but-valid-CRC frames cannot be detected here;
/// the completed post's digest and signature checks catch them.
pub struct FountainDecoder {
    short_post_id: [u8; SHORT_ID_LEN],
    k: u16,
    total_len: u32,
    chunk_size: u16,
    chunks: Vec<u8>,
    resolved: [u8; MAX_FRAGMENT_COUNT / 8],
    resolved_count: usize,
    pending: std::collections::VecDeque<PendingFrame>,
    pending_bytes: usize,
}

impl FountainDecoder {
    /// Start a transfer from the first validated fountain frame received.
    pub fn new(first: &FountainFrame) -> Result<(FountainDecoder, FeedResult)> {
        // re-check so a hand-constructed frame cannot bypass the bounds
        validate_geometry(first.k, first.total_len, first.chunk_size)?;
        let mut d = FountainDecoder {
            short_post_id: first.short_post_id,
            k: first.k,
            total_len: first.total_len,
            chunk_size: first.chunk_size,
            chunks: vec![0u8; first.k as usize * first.chunk_size as usize],
            resolved: [0u8; MAX_FRAGMENT_COUNT / 8],
            resolved_count: 0,
            pending: std::collections::VecDeque::new(),
            pending_bytes: 0,
        };
        let res = d.feed(first)?;
        Ok((d, res))
    }

    fn is_resolved(&self, idx: u16) -> bool {
        self.resolved[idx as usize / 8] & (1 << (idx as usize % 8)) != 0
    }

    /// Feed one frame. Returns `Duplicate` for frames that carry no new
    /// information; disagreement in transfer parameters fails the transfer.
    pub fn feed(&mut self, frame: &FountainFrame) -> Result<FeedResult> {
        if frame.short_post_id != self.short_post_id
            || frame.k != self.k
            || frame.total_len != self.total_len
            || frame.chunk_size != self.chunk_size
        {
            return Err(BaogramError::FragmentConflict);
        }
        if frame.payload.len() != self.chunk_size as usize {
            return Err(BaogramError::FragmentPayloadLenInvalid);
        }
        if self.is_complete() {
            return Ok(FeedResult::Complete);
        }
        // reduce the frame by everything already resolved
        let mut payload = frame.payload.clone();
        let mut remaining: Vec<u16> = Vec::new();
        for idx in fountain_indices(&self.short_post_id, frame.frame_no, self.k) {
            if self.is_resolved(idx) {
                let start = idx as usize * self.chunk_size as usize;
                for (dst, src) in payload.iter_mut().zip(self.chunks[start..].iter()) {
                    *dst ^= src;
                }
            } else {
                remaining.push(idx);
            }
        }
        match remaining.len() {
            0 => Ok(FeedResult::Duplicate),
            1 => {
                self.resolve_and_cascade(remaining[0], payload);
                if self.is_complete() {
                    Ok(FeedResult::Complete)
                } else {
                    Ok(FeedResult::Accepted { received: self.resolved_count, total: self.k as usize })
                }
            }
            _ => {
                // store for later peeling, evicting oldest beyond the caps
                while self.pending.len() >= PENDING_FRAMES_CAP
                    || self.pending_bytes + payload.len() > PENDING_BYTES_CAP
                {
                    match self.pending.pop_front() {
                        Some(old) => self.pending_bytes -= old.payload.len(),
                        None => break,
                    }
                }
                self.pending_bytes += payload.len();
                self.pending.push_back(PendingFrame { indices: remaining, payload });
                Ok(FeedResult::Accepted { received: self.resolved_count, total: self.k as usize })
            }
        }
    }

    /// Write a solved chunk, then peel every pending frame that references
    /// it; newly solved frames cascade until a fixpoint.
    fn resolve_and_cascade(&mut self, idx: u16, payload: Vec<u8>) {
        let mut work: Vec<(u16, Vec<u8>)> = vec![(idx, payload)];
        while let Some((i, data)) = work.pop() {
            if self.is_resolved(i) {
                // duplicate resolution (e.g. two coded frames peeled to the
                // same chunk): first result wins; the post-level digest
                // check catches a corrupt sender
                continue;
            }
            let start = i as usize * self.chunk_size as usize;
            self.chunks[start..start + self.chunk_size as usize].copy_from_slice(&data);
            self.resolved[i as usize / 8] |= 1 << (i as usize % 8);
            self.resolved_count += 1;
            let mut j = 0;
            while j < self.pending.len() {
                let pf = &mut self.pending[j];
                if let Ok(pos) = pf.indices.binary_search(&i) {
                    for (dst, src) in pf.payload.iter_mut().zip(data.iter()) {
                        *dst ^= src;
                    }
                    pf.indices.remove(pos);
                    if pf.indices.len() <= 1 {
                        let pf = self.pending.remove(j).unwrap();
                        self.pending_bytes -= pf.payload.len();
                        if let Some(&only) = pf.indices.first() {
                            work.push((only, pf.payload));
                        }
                        continue; // slot j now holds the next frame
                    }
                }
                j += 1;
            }
        }
    }

    /// Source chunks resolved so far.
    pub fn received_count(&self) -> usize {
        self.resolved_count
    }

    /// Total source chunks (k).
    pub fn total_count(&self) -> usize {
        self.k as usize
    }

    /// The transfer's short post ID.
    pub fn short_post_id(&self) -> [u8; SHORT_ID_LEN] {
        self.short_post_id
    }

    /// True when every source chunk has been resolved.
    pub fn is_complete(&self) -> bool {
        self.resolved_count == self.k as usize
    }

    /// Take the reassembled post bytes. Errors unless complete.
    pub fn into_bytes(mut self) -> Result<Vec<u8>> {
        if !self.is_complete() {
            return Err(BaogramError::TransferIncomplete);
        }
        self.chunks.truncate(self.total_len as usize);
        Ok(self.chunks)
    }
}

/// A frame of either wire version, dispatched on the version byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnyFrame {
    V1(Fragment),
    V2(FountainFrame),
}

impl AnyFrame {
    pub fn from_bytes(bytes: &[u8]) -> Result<AnyFrame> {
        if bytes.len() < 3 {
            return Err(BaogramError::Truncated);
        }
        if bytes[0..2] != FRAGMENT_MAGIC {
            return Err(BaogramError::BadMagic);
        }
        match bytes[2] {
            FRAGMENT_VERSION => Ok(AnyFrame::V1(Fragment::from_bytes(bytes)?)),
            FOUNTAIN_VERSION => Ok(AnyFrame::V2(FountainFrame::from_bytes(bytes)?)),
            _ => Err(BaogramError::UnknownVersion),
        }
    }

    pub fn from_base45(text: &str) -> Result<AnyFrame> {
        let bytes = base45::decode(text).map_err(|_| BaogramError::Base45Decode)?;
        AnyFrame::from_bytes(&bytes)
    }

    pub fn short_post_id(&self) -> [u8; SHORT_ID_LEN] {
        match self {
            AnyFrame::V1(f) => f.short_post_id,
            AnyFrame::V2(f) => f.short_post_id,
        }
    }
}

/// Receiver for either wire version, locked to the version of the first
/// frame; a frame of the other version for the same post is a conflict.
pub enum AnyReceiver {
    V1(Reassembler),
    V2(FountainDecoder),
}

impl AnyReceiver {
    pub fn new(first: &AnyFrame) -> Result<(AnyReceiver, FeedResult)> {
        match first {
            AnyFrame::V1(f) => Reassembler::new(f).map(|(r, res)| (AnyReceiver::V1(r), res)),
            AnyFrame::V2(f) => FountainDecoder::new(f).map(|(d, res)| (AnyReceiver::V2(d), res)),
        }
    }

    pub fn feed(&mut self, frame: &AnyFrame) -> Result<FeedResult> {
        match (self, frame) {
            (AnyReceiver::V1(r), AnyFrame::V1(f)) => r.feed(f),
            (AnyReceiver::V2(d), AnyFrame::V2(f)) => d.feed(f),
            _ => Err(BaogramError::FragmentConflict),
        }
    }

    pub fn received_count(&self) -> usize {
        match self {
            AnyReceiver::V1(r) => r.received_count(),
            AnyReceiver::V2(d) => d.received_count(),
        }
    }

    pub fn total_count(&self) -> usize {
        match self {
            AnyReceiver::V1(r) => r.total_count(),
            AnyReceiver::V2(d) => d.total_count(),
        }
    }

    pub fn short_post_id(&self) -> [u8; SHORT_ID_LEN] {
        match self {
            AnyReceiver::V1(r) => r.short_post_id(),
            AnyReceiver::V2(d) => d.short_post_id(),
        }
    }

    pub fn is_complete(&self) -> bool {
        match self {
            AnyReceiver::V1(r) => r.is_complete(),
            AnyReceiver::V2(d) => d.is_complete(),
        }
    }

    pub fn into_bytes(self) -> Result<Vec<u8>> {
        match self {
            AnyReceiver::V1(r) => r.into_bytes(),
            AnyReceiver::V2(d) => d.into_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    fn test_payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 + 3) as u8).collect()
    }

    #[test]
    fn crc32_reference_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn fragment_round_trip_binary_and_base45() {
        let post = test_payload(1000);
        let frags = fragment_post(&ID, &post, 64).unwrap();
        assert_eq!(frags.len(), 16); // ceil(1000/64)
        assert_eq!(frags[15].payload.len(), 1000 - 15 * 64);
        for f in &frags {
            let parsed = Fragment::from_bytes(&f.to_bytes()).unwrap();
            assert_eq!(&parsed, f);
            let parsed45 = Fragment::from_base45(&f.to_base45()).unwrap();
            assert_eq!(&parsed45, f);
        }
    }

    #[test]
    fn out_of_order_reconstruction() {
        let post = test_payload(777);
        let frags = fragment_post(&ID, &post, 50).unwrap();
        // feed in a scrambled but deterministic order
        let mut order: Vec<usize> = (0..frags.len()).collect();
        order.reverse();
        order.swap(0, 7);
        order.swap(3, 11);
        let (mut r, _) = Reassembler::new(&frags[order[0]]).unwrap();
        for &i in &order[1..] {
            r.feed(&frags[i]).unwrap();
        }
        assert!(r.is_complete());
        assert_eq!(r.into_bytes().unwrap(), post);
    }

    #[test]
    fn duplicates_ignored_conflicts_fail() {
        let post = test_payload(300);
        let frags = fragment_post(&ID, &post, 64).unwrap();
        let (mut r, _) = Reassembler::new(&frags[0]).unwrap();
        // exact duplicate
        assert_eq!(r.feed(&frags[0]).unwrap(), FeedResult::Duplicate);
        // conflicting duplicate: same index, different bytes
        let mut evil = frags[0].clone();
        evil.payload[0] ^= 0xff;
        assert_eq!(r.feed(&evil).unwrap_err(), BaogramError::FragmentConflict);
    }

    #[test]
    fn disagreeing_parameters_rejected() {
        let post = test_payload(300);
        let frags = fragment_post(&ID, &post, 64).unwrap();
        let (mut r, _) = Reassembler::new(&frags[0]).unwrap();
        let mut other_id = frags[1].clone();
        other_id.short_post_id = [9; 8];
        assert_eq!(r.feed(&other_id).unwrap_err(), BaogramError::FragmentConflict);
        // different chunk size implies different geometry
        let refrags = fragment_post(&ID, &post, 100).unwrap();
        assert_eq!(r.feed(&refrags[1]).unwrap_err(), BaogramError::FragmentConflict);
    }

    #[test]
    fn crc_mismatch_rejected() {
        let post = test_payload(100);
        let frags = fragment_post(&ID, &post, 64).unwrap();
        let mut bytes = frags[0].to_bytes();
        let n = bytes.len();
        bytes[n - 1] ^= 1; // corrupt CRC
        assert_eq!(Fragment::from_bytes(&bytes).unwrap_err(), BaogramError::FragmentCrcMismatch);
        // corrupt payload byte -> CRC catches it
        let mut bytes = frags[0].to_bytes();
        bytes[FRAGMENT_HEADER_LEN] ^= 1;
        assert_eq!(Fragment::from_bytes(&bytes).unwrap_err(), BaogramError::FragmentCrcMismatch);
    }

    #[test]
    fn geometry_violations_rejected() {
        // count disagrees with ceil(total/chunk)
        let f = Fragment {
            short_post_id: ID,
            frag_index: 0,
            frag_count: 5,
            total_len: 100,
            chunk_size: 64,
            payload: test_payload(64),
        };
        assert_eq!(Fragment::from_bytes(&f.to_bytes()).unwrap_err(), BaogramError::FragmentGeometryInvalid);

        // index out of range
        let f = Fragment {
            short_post_id: ID,
            frag_index: 2,
            frag_count: 2,
            total_len: 100,
            chunk_size: 64,
            payload: test_payload(36),
        };
        assert_eq!(Fragment::from_bytes(&f.to_bytes()).unwrap_err(), BaogramError::FragmentIndexOutOfRange);

        // payload length wrong for non-final fragment
        let f = Fragment {
            short_post_id: ID,
            frag_index: 0,
            frag_count: 2,
            total_len: 100,
            chunk_size: 64,
            payload: test_payload(63),
        };
        assert_eq!(Fragment::from_bytes(&f.to_bytes()).unwrap_err(), BaogramError::FragmentPayloadLenInvalid);

        // zero chunk size
        assert_eq!(
            FragmentIter::new(&ID, &test_payload(10), 0).unwrap_err(),
            BaogramError::FragmentChunkSizeInvalid
        );
        // oversized total
        assert_eq!(
            FragmentIter::new(&ID, &vec![0u8; MAX_POST_BYTES + 1], 64).unwrap_err(),
            BaogramError::FragmentGeometryInvalid
        );
    }

    #[test]
    fn truncated_and_trailing_rejected() {
        let frags = fragment_post(&ID, &test_payload(100), 64).unwrap();
        let bytes = frags[0].to_bytes();
        assert_eq!(Fragment::from_bytes(&bytes[..10]).unwrap_err(), BaogramError::Truncated);
        // trailing bytes shift the CRC window, so corruption is caught there
        let mut extended = bytes.clone();
        extended.extend_from_slice(&[0, 0]);
        assert!(Fragment::from_bytes(&extended).is_err());
    }

    #[test]
    fn incomplete_transfer_cannot_be_read() {
        let frags = fragment_post(&ID, &test_payload(300), 64).unwrap();
        let (r, _) = Reassembler::new(&frags[0]).unwrap();
        assert!(!r.is_complete());
        assert_eq!(r.into_bytes().unwrap_err(), BaogramError::TransferIncomplete);
    }

    #[test]
    fn single_fragment_transfer() {
        let post = test_payload(40);
        let frags = fragment_post(&ID, &post, 64).unwrap();
        assert_eq!(frags.len(), 1);
        let (r, res) = Reassembler::new(&frags[0]).unwrap();
        assert_eq!(res, FeedResult::Complete);
        assert_eq!(r.into_bytes().unwrap(), post);
    }

    #[test]
    fn lazy_iter_matches_collected() {
        let post = test_payload(500);
        let it = FragmentIter::new(&ID, &post, 48).unwrap();
        let lazy: Vec<Fragment> = it.collect();
        let eager = fragment_post(&ID, &post, 48).unwrap();
        assert_eq!(lazy, eager);
        let it = FragmentIter::new(&ID, &post, 48).unwrap();
        assert_eq!(it.fragment_at(2).unwrap(), eager[2]);
        assert!(it.fragment_at(eager.len()).is_none());
    }

    // ---- fountain (v2) ----

    #[test]
    fn splitmix64_reference_vectors() {
        // canonical splitmix64 outputs for seed 0; shared with the Python peer
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(rng.next_u64(), 0x06C4_5D18_8009_454F);
    }

    #[test]
    fn fountain_indices_systematic_and_deterministic() {
        for k in [1u16, 2, 40, 4096] {
            for f in [0u32, (k as u32) / 2, k as u32 - 1] {
                assert_eq!(fountain_indices(&ID, f, k), vec![f as u16], "systematic frame {f} of k={k}");
            }
        }
        for frame in 40..200u32 {
            let a = fountain_indices(&ID, frame, 40);
            assert!(!a.is_empty() && a.len() <= 40);
            assert!(a.windows(2).all(|w| w[0] < w[1]), "sorted, distinct");
            assert!(a.iter().all(|&i| i < 40));
            assert_eq!(a, fountain_indices(&ID, frame, 40), "deterministic");
        }
    }

    #[test]
    fn fountain_indices_pinned_cross_language() {
        // pinned against the Python peer (same short id 01..08, k = 40);
        // any drift here breaks cross-language interop
        assert_eq!(
            fountain_indices(&ID, 40, 40),
            vec![0, 1, 6, 8, 12, 18, 23, 24, 25, 31, 32, 33, 34, 37, 38]
        );
        assert_eq!(fountain_indices(&ID, 41, 40), vec![28]);
        assert_eq!(
            fountain_indices(&ID, 42, 40),
            vec![
                1, 2, 5, 6, 7, 10, 11, 12, 13, 17, 20, 21, 22, 24, 25, 26, 27, 29, 30, 31, 32, 34, 35,
                36, 37
            ]
        );
    }

    #[test]
    fn fountain_frame_round_trip_and_rejections() {
        let post = test_payload(1000);
        let it = FountainIter::new(&ID, &post, 96).unwrap();
        assert_eq!(it.chunk_count(), 11);
        for frame_no in [0u32, 10, 11, 500, u32::MAX] {
            let f = it.frame_at(frame_no);
            assert_eq!(f.payload.len(), 96, "payload always chunk_size");
            let parsed = FountainFrame::from_bytes(&f.to_bytes()).unwrap();
            assert_eq!(parsed, f);
            let parsed45 = FountainFrame::from_base45(&f.to_base45()).unwrap();
            assert_eq!(parsed45, f);
        }
        // CRC corruption
        let mut bytes = it.frame_at(0).to_bytes();
        bytes[FOUNTAIN_HEADER_LEN] ^= 1;
        assert_eq!(FountainFrame::from_bytes(&bytes).unwrap_err(), BaogramError::FragmentCrcMismatch);
        // truncation and trailing bytes
        let bytes = it.frame_at(0).to_bytes();
        assert_eq!(FountainFrame::from_bytes(&bytes[..10]).unwrap_err(), BaogramError::Truncated);
        let mut ext = bytes.clone();
        ext.extend_from_slice(&[0, 0]);
        assert!(FountainFrame::from_bytes(&ext).is_err());
        // payload_len must equal chunk_size (valid CRC, wrong length field)
        let mut f = it.frame_at(0);
        f.payload.truncate(40);
        assert_eq!(FountainFrame::from_bytes(&f.to_bytes()).unwrap_err(), BaogramError::FragmentPayloadLenInvalid);
        // geometry disagreement (k != ceil(total/chunk))
        let mut f = it.frame_at(0);
        f.k = 12;
        assert_eq!(FountainFrame::from_bytes(&f.to_bytes()).unwrap_err(), BaogramError::FragmentGeometryInvalid);
    }

    #[test]
    fn fountain_systematic_completes_in_exactly_k() {
        let post = test_payload(5400); // ~the reported 84-QR post
        let it = FountainIter::new(&ID, &post, 96).unwrap();
        let k = it.chunk_count();
        assert_eq!(k, 57);
        let (mut d, _) = FountainDecoder::new(&it.frame_at(0)).unwrap();
        for f in 1..k as u32 - 1 {
            assert!(matches!(d.feed(&it.frame_at(f)).unwrap(), FeedResult::Accepted { .. }));
        }
        assert_eq!(d.feed(&it.frame_at(k as u32 - 1)).unwrap(), FeedResult::Complete);
        assert_eq!(d.into_bytes().unwrap(), post);
    }

    #[test]
    fn fountain_decodes_from_shuffled_coded_frames_only() {
        // no systematic frames at all: every frame is from the coded region,
        // in a deterministically scrambled order
        let post = test_payload(3210);
        let it = FountainIter::new(&ID, &post, 64).unwrap();
        let k = it.chunk_count() as u32; // 51
        let mut order: Vec<u32> = (k..k + 4 * k).collect();
        // deterministic shuffle via splitmix64
        let mut rng = SplitMix64::new(0xC0FF_EE00);
        for i in (1..order.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        let (mut d, _) = FountainDecoder::new(&it.frame_at(order[0])).unwrap();
        let mut complete = false;
        for &f in &order[1..] {
            if d.feed(&it.frame_at(f)).unwrap() == FeedResult::Complete {
                complete = true;
                break;
            }
        }
        assert!(complete, "4k coded frames must decode k chunks (got {}/{})", d.received_count(), k);
        assert_eq!(d.into_bytes().unwrap(), post);
    }

    #[test]
    fn fountain_survives_fifty_percent_loss_and_duplicates() {
        let post = test_payload(5400);
        let it = FountainIter::new(&ID, &post, 96).unwrap();
        let (mut d, _) = FountainDecoder::new(&it.frame_at(0)).unwrap();
        let mut complete = false;
        // drop every odd frame; feed every even frame twice (duplicate-heavy)
        for f in (2..2000u32).step_by(2) {
            let frame = it.frame_at(f);
            let r1 = d.feed(&frame).unwrap();
            if r1 == FeedResult::Complete {
                complete = true;
                break;
            }
            let _ = d.feed(&frame).unwrap(); // duplicate: never an error
        }
        assert!(complete, "stalled at {}/{}", d.received_count(), d.total_count());
        assert_eq!(d.into_bytes().unwrap(), post);
    }

    #[test]
    fn fountain_peeling_cascade() {
        // a coded frame held pending must resolve when its other chunk arrives
        let post = test_payload(500);
        let it = FountainIter::new(&ID, &post, 64).unwrap();
        let k = it.chunk_count() as u16; // 8
        // find a coded frame with exactly 2 source chunks
        let mut coded = None;
        for f in k as u32..k as u32 + 200 {
            if fountain_indices(&ID, f, k).len() == 2 {
                coded = Some(f);
                break;
            }
        }
        let coded = coded.expect("a degree-2 frame exists in 200 coded frames");
        let pair = fountain_indices(&ID, coded, k);
        let (a, b) = (pair[0], pair[1]);
        // feed chunk a, then the coded frame (pending), then everything but b
        let (mut d, _) = FountainDecoder::new(&it.frame_at(a as u32)).unwrap();
        assert!(matches!(d.feed(&it.frame_at(coded)).unwrap(), FeedResult::Accepted { .. }));
        // the coded frame reduced to {b} immediately: b is already resolved
        assert_eq!(d.received_count(), 2, "cascade resolved chunk {b} from the coded frame");
        for i in 0..k {
            if i != a && i != b {
                d.feed(&it.frame_at(i as u32)).unwrap();
            }
        }
        assert!(d.is_complete());
        assert_eq!(d.into_bytes().unwrap(), post);
    }

    #[test]
    fn fountain_conflicts_and_cross_version() {
        let post = test_payload(1000);
        let it = FountainIter::new(&ID, &post, 96).unwrap();
        let (mut d, _) = FountainDecoder::new(&it.frame_at(0)).unwrap();
        // disagreeing geometry (different chunk size => different k)
        let other = FountainIter::new(&ID, &post, 64).unwrap();
        assert_eq!(d.feed(&other.frame_at(0)).unwrap_err(), BaogramError::FragmentConflict);
        // different post id
        let mut alien = it.frame_at(1);
        alien.short_post_id = [9; 8];
        assert_eq!(d.feed(&alien).unwrap_err(), BaogramError::FragmentConflict);

        // AnyReceiver: v1 frame for a v2 transfer is a conflict
        let v2 = AnyFrame::from_bytes(&it.frame_at(0).to_bytes()).unwrap();
        let v1frags = fragment_post(&ID, &post, 96).unwrap();
        let v1 = AnyFrame::from_bytes(&v1frags[0].to_bytes()).unwrap();
        let (mut rx, _) = AnyReceiver::new(&v2).unwrap();
        assert_eq!(rx.feed(&v1).unwrap_err(), BaogramError::FragmentConflict);
        // and the receiver still completes as v2
        for f in 1..it.chunk_count() as u32 {
            rx.feed(&AnyFrame::from_bytes(&it.frame_at(f).to_bytes()).unwrap()).unwrap();
        }
        assert!(rx.is_complete());
        assert_eq!(rx.into_bytes().unwrap(), post);

        // unknown version byte
        let mut bytes = it.frame_at(0).to_bytes();
        bytes[2] = 3;
        assert_eq!(AnyFrame::from_bytes(&bytes).unwrap_err(), BaogramError::UnknownVersion);
    }

    #[test]
    fn fountain_pending_eviction_only_costs_efficiency() {
        // k=64 chunks of 1024 bytes = exactly MAX_POST_BYTES; each pending
        // coded frame holds 1024 bytes, so the 65,536-byte pending cap fits
        // 64 frames — feed hundreds of multi-chunk frames to force eviction,
        // then complete systematically and verify the payload anyway
        let post = test_payload(MAX_POST_BYTES);
        let it = FountainIter::new(&ID, &post, 1024).unwrap();
        let k = it.chunk_count() as u32; // 64
        let mut coded: Vec<u32> = Vec::new();
        let mut f = k;
        while coded.len() < 300 {
            if fountain_indices(&ID, f, k as u16).len() >= 2 {
                coded.push(f);
            }
            f += 1;
        }
        let (mut d, _) = FountainDecoder::new(&it.frame_at(coded[0])).unwrap();
        for &c in &coded[1..] {
            let _ = d.feed(&it.frame_at(c)).unwrap();
        }
        // now complete with the systematic frames
        for i in 0..k {
            if d.feed(&it.frame_at(i)).unwrap() == FeedResult::Complete {
                break;
            }
        }
        assert!(d.is_complete());
        assert_eq!(d.into_bytes().unwrap(), post);
    }
}
