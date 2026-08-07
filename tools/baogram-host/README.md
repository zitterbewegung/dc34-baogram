# baogram-host

Laptop peer for **Baogram**, the signed, local, full-spatial-resolution
monochrome picture-sharing application for the DEF CON 34 Baochip badge.

Implements the exact canonical formats from
`../../libraries/baogram-core` (see `../../BAOGRAM_PROTOCOL.md`):
posts created here verify on the badge and vice versa.

## Install

```sh
cd tools/baogram-host
python3 -m venv .venv && . .venv/bin/activate
pip install -e '.[test]'
```

Requires Python 3.11+. `opencv-python` provides the camera + QR decoding
for `send`/`receive`; the format tools work without a camera.

## Usage

```sh
# one-time local identity (32-byte seed, file mode 0600).
# NEVER copy a badge's private key here — generate a fresh host identity.
baogram-host keygen --key baogram-host.key

# build + sign a post from any image (resized to 256x240, 1-bit quantized)
baogram-host make-post --image input.png --handle alice \
    --caption "hello" --output post.bgrm

# inspect / verify
baogram-host inspect post.bgrm
baogram-host verify post.bgrm

# export the post's image to a 256x240 PNG
baogram-host export post.bgrm --output received.png

# show the post as an animated QR loop (badge scans it)
baogram-host send post.bgrm --payload-bytes 64 --period-ms 500

# watch a webcam for a badge's animated QR loop
baogram-host receive --camera 0 --output received.bgrm
```

`send` refuses to transmit a post that does not verify; `receive` refuses
to save one. Fragments may arrive in any order; exact duplicates are
ignored and conflicting duplicates abort the transfer.

## Tests

```sh
pytest
```

The test suite consumes the Rust golden vectors from
`../../libraries/baogram-core/test-vectors/` (byte-identical
reserialization, deterministic signature reproduction, negative vectors)
and regenerates `test-vectors/python/` — the vectors the Rust suite
verifies in the other direction.
