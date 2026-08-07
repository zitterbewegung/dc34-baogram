//! Error types for the Baogram core library.

use core::fmt;

/// Every way a Baogram post, fragment, or image can fail to parse or validate.
///
/// Variants are deliberately fine-grained so that rejection paths can be
/// asserted individually in tests and reported precisely in diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaogramError {
    // ---- container-level ----
    /// Magic bytes did not match.
    BadMagic,
    /// Protocol version is not supported.
    UnknownVersion,
    /// Pixel format byte is not supported.
    UnknownPixelFormat,
    /// Codec byte is not supported.
    UnknownCodec,
    /// Reserved flag bits were set.
    UnknownFlags,
    /// Input ended before the declared structure was complete.
    Truncated,
    /// Input continued past the declared structure.
    TrailingBytes,
    /// A declared length field is inconsistent with the container or with
    /// protocol bounds (e.g. compressed data not smaller than raw).
    NoncanonicalLength,
    /// Declared dimensions are not supported by this protocol version.
    UnsupportedDimensions,
    /// Handle exceeded HANDLE_MAX_BYTES or caption exceeded CAPTION_MAX_BYTES.
    FieldTooLong,
    /// Handle or caption is not valid UTF-8.
    InvalidUtf8,
    /// Total post size exceeds MAX_POST_BYTES.
    PostTooLarge,

    // ---- integrity ----
    /// Stored SHA-256 digest does not match the recomputed digest.
    DigestMismatch,
    /// Ed25519 signature failed to verify.
    InvalidSignature,
    /// The author public key could not be interpreted as an Ed25519 point.
    InvalidPublicKey,

    // ---- codec ----
    /// Compressed stream ended in the middle of a run or literal.
    CodecPrematureEnd,
    /// Decoding would produce more than the expected output length.
    CodecOutputOverflow,
    /// Decoding produced fewer bytes than the expected output length.
    CodecOutputUnderflow,
    /// A control byte was invalid (e.g. the reserved 0x80 no-op).
    CodecInvalidRun,
    /// Compressed stream had bytes after the output was already complete.
    CodecTrailingGarbage,

    // ---- fragments ----
    /// Fragment CRC-32 check failed.
    FragmentCrcMismatch,
    /// Fragment index is >= the declared fragment count.
    FragmentIndexOutOfRange,
    /// Fragment count is zero or exceeds MAX_FRAGMENT_COUNT.
    FragmentCountOutOfRange,
    /// Declared chunk size is zero or exceeds MAX_FRAGMENT_PAYLOAD.
    FragmentChunkSizeInvalid,
    /// The payload length is inconsistent with index/count/total/chunk_size.
    FragmentPayloadLenInvalid,
    /// The fragment geometry (count, total_len, chunk_size) is self-inconsistent.
    FragmentGeometryInvalid,
    /// Base45 text could not be decoded.
    Base45Decode,
    /// A fragment arrived that disagrees with previously accepted fragments
    /// (different post id, count, total length, chunk size, or a duplicate
    /// index with different payload bytes).
    FragmentConflict,
    /// The transfer is not yet complete, so the requested operation is invalid.
    TransferIncomplete,

    // ---- image ----
    /// Grayscale input frame is not exactly IMAGE_WIDTH * IMAGE_HEIGHT bytes.
    BadFrameSize,
    /// Packed mono input is not exactly MONO1_PACKED_LEN bytes.
    BadPackedSize,
}

impl fmt::Display for BaogramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for BaogramError {}

pub type Result<T> = core::result::Result<T, BaogramError>;
