//! Fragmenting a serialized post into QR-sized pieces, and reassembling
//! them in any order.
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
    pub fn to_base45(&self) -> String { base45::encode(&self.to_bytes()) }

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
    FragmentIter::new(short_post_id, serialized_post, chunk_size)
        .map(|it| it.collect())
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
    pub fn count_total(&self) -> usize { self.count }

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
    pub fn received_count(&self) -> usize { self.received_count }

    /// Total fragments expected.
    pub fn total_count(&self) -> usize { self.frag_count as usize }

    /// The transfer's short post ID.
    pub fn short_post_id(&self) -> [u8; SHORT_ID_LEN] { self.short_post_id }

    /// True when every fragment has been received.
    pub fn is_complete(&self) -> bool { self.received_count == self.frag_count as usize }

    /// Take the reassembled post bytes. Errors unless complete.
    pub fn into_bytes(self) -> Result<Vec<u8>> {
        if !self.is_complete() {
            return Err(BaogramError::TransferIncomplete);
        }
        Ok(self.buffer)
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
        assert_eq!(
            Fragment::from_bytes(&f.to_bytes()).unwrap_err(),
            BaogramError::FragmentIndexOutOfRange
        );

        // payload length wrong for non-final fragment
        let f = Fragment {
            short_post_id: ID,
            frag_index: 0,
            frag_count: 2,
            total_len: 100,
            chunk_size: 64,
            payload: test_payload(63),
        };
        assert_eq!(
            Fragment::from_bytes(&f.to_bytes()).unwrap_err(),
            BaogramError::FragmentPayloadLenInvalid
        );

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
}
