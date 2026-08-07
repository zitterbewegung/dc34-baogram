# Baogram protocol, version 1

Normative description of the bytes on the wire (and on disk). The Rust
reference implementation is `libraries/baogram-core`; the Python peer
(`tools/baogram-host`) is byte-for-byte compatible and cross-tested
against the golden vectors in `libraries/baogram-core/test-vectors/`.

**All multi-byte integers are big-endian.** There is exactly one valid
serialization for any post: parsers reject noncanonical encodings.

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

Canonicality rule: codec 1 may be used **only when the compressed form
is strictly smaller than 7,680 bytes**; parsers reject codec-1 posts
whose encoded length is >= 7,680.

## 3. Post container (`BGRM`)

```text
offset  len  field
------  ---  -----
     0    4  magic = "BGRM"
     4    1  version = 1
     5    1  pixel_format = 1  (Mono1 as in section 1)
     6    1  codec (0 | 1)
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

## 4. Fragments (`BG`), Base45, QR

A serialized post is split with a **uniform chunk size** chosen by the
sender (default 64 bytes; diagnostics allow 24/40/64/80/96): fragment
`i` carries post bytes `[i*chunk, min((i+1)*chunk, total))`.

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
* fail the transfer on conflicting duplicates or on any disagreement in
  (short id, count, total length, chunk size);
* allocate the reassembly buffer once from the validated `total_len`
  (bounded by 65,536) and track receipt in a fixed 4,096-bit bitmap;
* refuse to parse an incomplete transfer as a post;
* verify the completed post (section 3) before reporting or saving it.

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
