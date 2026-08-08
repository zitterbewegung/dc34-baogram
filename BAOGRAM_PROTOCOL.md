# Baogram protocol, version 1

Normative description of the bytes on the wire (and on disk). The Rust
reference implementation is `libraries/baogram-core`; the Python peer
(`tools/baogram-host`) is byte-for-byte compatible and cross-tested
against the golden vectors in `libraries/baogram-core/test-vectors/`.

**All multi-byte integers are big-endian.** The reference encoders are
fully deterministic (identical bytes from identical inputs across Rust
and Python). Parsers enforce every canonicality rule listed below, with
one documented exception: any *well-formed* PackBits stream that decodes
to exactly 7,680 bytes is accepted, so a third-party encoder using
different (valid) run/literal choices produces a post with different
bytes — and therefore a different digest and post ID — for the same
pixels. Post IDs identify exact post bytes, not pixel content.

## 1. Image

* Dimensions: exactly **256 x 240**. All 61,440 pixel positions are
  preserved ("full spatial resolution"); the MVP quantizes the 8-bit
  camera luminance to **1 bit per pixel**.
* Packed Mono1: row-major, MSB-first within each byte, **bit 1 = white**,
  0 = black. 256/8 = 32 bytes per row x 240 rows = **7,680 bytes**.
* Quantization (canonical, both implementations): threshold `t` =
  `floor(sum(luminance) / 61440)` unless explicitly overridden; a pixel
  is white iff `luminance > t`. No dithering (error diffusion would hurt
  run-length compression and make cross-implementation validation
  nondeterministic).

## 2. Codecs

| id | name           | payload                                    |
|----|----------------|--------------------------------------------|
| 0  | RawMono1       | the 7,680 packed bytes verbatim            |
| 1  | PackBitsMono1  | deterministic PackBits variant (below)     |
| 2  | RowDeltaMono1  | row-delta filter, then PackBitsMono1       |

PackBitsMono1 stream = sequence of blocks, control byte `c`:

* `0x00..=0x7F`: literal — next `c + 1` bytes copied verbatim (1..=128);
* `0x81..=0xFF`: run — next byte repeated `257 - c` times (2..=128);
* `0x80`: reserved — decoders MUST reject.

Canonical encoder (identical output in Rust and Python): scan left to
right; a run of **>= 3** identical bytes (capped at 128) becomes a run
block; everything else accumulates into literal blocks capped at 128
bytes. Decoders MUST require exactly 7,680 output bytes and reject:
premature end, output overflow or underflow, control `0x80`, and
trailing input.

RowDeltaMono1 (codec 2): treat the packed image as 240 rows of 32 bytes;
the filtered image is row 0 verbatim, then each row XORed byte-wise with
the **original** row above it. The filtered bytes are then encoded with
PackBitsMono1 exactly as codec 1. Decoding un-filters top-down using the
already-reconstructed previous row. Threshold-quantized photos are
vertically correlated, so the filtered image is mostly zero and
compresses far better (the golden synthetic frame: 2,439 encoded bytes
under codec 1 vs 195 under codec 2).

Canonicality rules: a compressed codec (1 or 2) may be used **only when
the encoded form is strictly smaller than 7,680 bytes**; parsers reject
compressed posts whose encoded length is >= 7,680. Creators MUST compute
both codec 1 and codec 2 and pick the smaller encoding, ties going to
the lower codec id (so both implementations produce identical posts);
raw (0) when neither compresses.

## 3. Post container (`BGRM`)

```text
offset  len  field
------  ---  -----
     0    4  magic = "BGRM"
     4    1  version = 1
     5    1  pixel_format = 1  (Mono1 as in section 1)
     6    1  codec (0 | 1 | 2)
     7    1  flags = 0 (all bits reserved; nonzero rejected)
     8    2  width = 256
    10    2  height = 240
    12    8  seq        (author's persistent post sequence number)
    20   32  author_pubkey (Ed25519)
    52    1  handle_len  (0..=24)
    53    1  caption_len (0..=64)
    54    4  image_uncompressed_len = 7680
    58    4  image_encoded_len (<= 61,440)
 -- end of canonical header (62 bytes) --
    62    m  handle   (UTF-8, m = handle_len)
  62+m    n  caption  (UTF-8, n = caption_len)
62+m+n    e  encoded_image (e = image_encoded_len)
   ...   32  digest
   ...   64  signature
```

* `digest = SHA256("BAOGRAM-POST-V1" || canonical_header || handle ||
  caption || encoded_image)` — the domain string is the 15 raw ASCII
  bytes, no terminator.
* `signature = Ed25519_sign(sk, "BAOGRAM-SIG-V1" || digest)` (RFC 8032,
  deterministic; the 14-byte domain prefix prevents cross-protocol
  signature reuse). The private key/seed is never serialized into a post.
* **Post ID** = first 16 bytes of the digest. **Short ID** (fragments) =
  first 8 bytes.
* Bounds: total serialized post <= **65,536** bytes.

Parsers MUST validate, in an order that never sizes an allocation from
an unvalidated length: magic, version, pixel format, codec, zero flags,
dimensions (exactly 256x240), handle/caption bounds, uncompressed length
== 7,680, encoded length <= 61,440, exact total length (no truncation,
no trailing bytes), UTF-8 validity, digest recomputation, signature, and
image decode to exactly 7,680 bytes.

## 4. Fragments (`BG` version 1), Base45, QR

Version 1 is the legacy fixed loop; current senders default to the
fountain stream of section 4b, but v1 remains valid and receivers accept
both.

A serialized post is split with a **uniform chunk size** chosen by the
sender (diagnostics allow 24/40/64/80/96): fragment `i` carries post
bytes `[i*chunk, min((i+1)*chunk, total))`.

```text
offset  len  field
------  ---  -----
     0    2  magic = "BG"
     2    1  version = 1
     3    1  flags = 0 (nonzero rejected)
     4    8  short_post_id
    12    2  frag_index (0-based)
    14    2  frag_count (1..=4096)
    16    4  total_len  (1..=65,536)
    20    2  chunk_size (1..=1024)
    22    2  payload_len
    24    n  payload (n = payload_len)
  24+n    4  crc32 = CRC-32/ISO-HDLC over bytes [0, 24+n)
```

Validation (each fragment in isolation): magic/version/flags, CRC,
`frag_count == ceil(total_len / chunk_size)`, `frag_index < frag_count`,
`payload_len` exactly `chunk_size` (non-final) or
`total_len - (frag_count-1)*chunk_size` (final, in 1..=chunk_size), and
exact container length.

The QR text layer is the **Base45** (RFC 9285) encoding of the binary
fragment; QR codes use error-correction level M. Receivers:

* accept fragments in any order;
* ignore exact duplicates;
* ignore fragments whose short post ID differs from the transfer in
  progress (another sender in view is not a conflict);
* fail the transfer on conflicting duplicates or on any same-ID
  disagreement in (count, total length, chunk size);
* allocate the reassembly buffer once from the validated `total_len`
  (bounded by 65,536) and track receipt in a fixed 4,096-bit bitmap;
* refuse to parse an incomplete transfer as a post;
* verify the completed post (section 3) before reporting or saving it.

## 4b. Fountain frames (`BG` version 2)

Version 2 replaces the fixed fragment loop with a **rateless fountain
stream**: the sender emits frames `0, 1, 2, ...` forever and never
repeats; any `k` sufficiently distinct frames reconstruct the post, so
frame order and frame loss cost only time, never a full loop. v1 remains
valid; receivers accept both and lock to the version of the first frame
of a transfer.

```text
offset  len  field
------  ---  -----
     0    2  magic = "BG"
     2    1  version = 2
     3    1  flags = 0 (nonzero rejected)
     4    8  short_post_id
    12    4  frame_no   (u32)
    16    2  k          (source chunk count, 1..=4096, == ceil(total_len / chunk_size))
    18    4  total_len  (1..=65,536)
    22    2  chunk_size (1..=1024)
    24    2  payload_len (always == chunk_size; the final source chunk is
              zero-padded for XOR purposes, total_len recovers the length)
    26    n  payload = XOR of the source chunks selected by frame_no
  26+n    4  crc32 = CRC-32/ISO-HDLC over bytes [0, 26+n)
```

Validation (each frame in isolation): magic/version/flags, CRC, geometry
(`k == ceil(total_len / chunk_size)` plus the v1 bounds), `payload_len ==
chunk_size`, and exact container length. `frame_no` may be any u32.

**Chunk selection** (normative; identical in Rust and Python, pinned by
golden vectors and unit tests):

* `frame_no < k`: the frame carries source chunk `frame_no` verbatim
  (**systematic prefix** — a loss-free receiver completes in exactly `k`
  frames).
* `frame_no >= k`: seed a **splitmix64** generator with
  `BE_u64(short_post_id) XOR u64(frame_no)`.
  * The first draw picks the degree `d` from the fixed-point distribution
    `W(d) = floor(2^32 / d)`, `d = 1..=k`: with `r = draw mod sum(W)`,
    the degree is the smallest `d` whose cumulative weight exceeds `r`.
    (Integer-only on purpose: no floating point, no transcendental
    functions, so cross-language determinism is trivial.)
  * Subsequent draws pick chunk indices as `draw mod k`, skipping
    already-chosen indices, until `d` distinct indices are chosen. The
    payload is their XOR (order irrelevant).

splitmix64 (all arithmetic mod 2^64): `state += 0x9E3779B97F4A7C15;
z = state; z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9;
z = (z ^ (z >> 27)) * 0x94D049BB133111EB; return z ^ (z >> 31)`.

Receivers run a **peeling decoder**: a frame is reduced by XORing out
already-resolved chunks; a frame reduced to one unknown resolves that
chunk and cascades through stored frames. Stored unresolved frames are
bounded (512 frames / 65,536 payload bytes, oldest evicted) — eviction
costs efficiency, never correctness. Same conflict rules as v1: any
same-short-ID disagreement in (k, total_len, chunk_size) — or a v1
fragment for a v2 transfer — fails the transfer; other-ID frames are
ignored. A corrupted-but-CRC-valid stream is caught by the post digest
and signature checks after reassembly.

Defaults: 96-byte chunks at 250 ms/frame (sender-side; the display pump
quantizes periods to 250 ms).

## 4c. Compatibility

* Old receivers (v1-only firmware) cannot decode v2 frames and will
  ignore them; senders on new firmware default to v2. The Python peer
  (`baogram-host send --protocol v1`) can still produce v1 streams for
  old badges.
* Codec 2 (`RowDeltaMono1`, section 3) posts do not parse on old
  firmware. Acceptable at this stage: the flashed fleet is one badge.
  Codec choice is canonical (deterministic smallest-wins with ties to
  the lower id), so both implementations produce identical posts.

## 5. Golden vectors

`libraries/baogram-core/test-vectors/` — all deterministic (fixed seed
`b"BAOGRAM-TEST-SEED-0000000000001!"`, synthetic frame, Ed25519 is
deterministic). Regenerate with
`BAOGRAM_REGEN_VECTORS=1 cargo test golden` in `libraries/baogram-core`.
`test-vectors/python/` is written by the Python test suite and consumed
by the Rust test `golden::python_generated_vectors`.

The synthetic 256x240 test frame (gradient / dark-light bands / 16px
checkerboard / vertical gradient) has SHA-256
`c82bfcf7e7ee582e204c151af00570cb832dcddced95df56f484d1cc7d0303db` and
is served by the hosted badge camera emulation.
