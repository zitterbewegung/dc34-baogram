"""baogram-host command-line interface.

Commands: make-post, inspect, verify, export, send, receive, keygen.
Key generation is local and explicit; a badge's private key is never
imported or reused here.
"""

from __future__ import annotations

import argparse
import os
import sys

from . import crypto
from .format import FormatError, Post, format_geometry
from .fragments import DEFAULT_FOUNTAIN_PAYLOAD, DEFAULT_FRAGMENT_PAYLOAD
from .image import (
    IMAGE_HEIGHT,
    IMAGE_WIDTH,
    despeckle_small,
    load_png_as_mono1,
    save_mono1_as_png,
    small_from_gray,
)

CODEC_NAMES = {0: "RawMono1", 1: "PackBitsMono1", 2: "RowDeltaMono1", 3: "CtxArithMono1"}


def _format_name(pixel_format: int) -> str:
    geometry = format_geometry(pixel_format)
    if geometry is None:
        return f"pixel format {pixel_format}"
    return f"{geometry[0]}x{geometry[1]}"


def _load_or_create_identity(key_path: str) -> crypto.Identity:
    """Load a 32-byte seed from key_path, creating it (0600) if absent."""
    if os.path.exists(key_path):
        with open(key_path, "rb") as f:
            seed = f.read()
        if len(seed) != 32:
            raise SystemExit(f"{key_path}: expected exactly 32 seed bytes, got {len(seed)}")
    else:
        seed = os.urandom(32)
        fd = os.open(key_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as f:
            f.write(seed)
        print(f"generated new host identity seed at {key_path}", file=sys.stderr)
    return crypto.Identity(seed)


def cmd_keygen(args) -> int:
    if os.path.exists(args.key):
        print(f"refusing to overwrite existing key {args.key}", file=sys.stderr)
        return 1
    ident = _load_or_create_identity(args.key)
    print(f"public key: {ident.public_key.hex()}")
    print(f"fingerprint: {crypto.hex_fingerprint(ident.public_key)}")
    return 0


def _load_gray_frame(path: str) -> bytes:
    """Load an image file as a 256x240 8-bit grayscale frame."""
    from PIL import Image

    img = Image.open(path).convert("L")
    if img.size != (IMAGE_WIDTH, IMAGE_HEIGHT):
        img = img.resize((IMAGE_WIDTH, IMAGE_HEIGHT))
    return img.tobytes()


def cmd_make_post(args) -> int:
    ident = _load_or_create_identity(args.key)
    if args.size == "small":
        gray = _load_gray_frame(args.image)
        packed = despeckle_small(small_from_gray(gray, args.threshold))
        post = Post.create_small(ident, args.seq, args.handle, args.caption, packed)
    else:
        packed = load_png_as_mono1(args.image, args.threshold)
        post = Post.create(ident, args.seq, args.handle, args.caption, packed)
    data = post.serialize()
    with open(args.output, "wb") as f:
        f.write(data)
    unc_len = format_geometry(post.pixel_format)[2]
    print(
        f"wrote {args.output}: {len(data)} bytes, post id {post.post_id.hex()}, "
        f"{_format_name(post.pixel_format)}, "
        f"codec {CODEC_NAMES.get(post.codec, str(post.codec))} "
        f"({len(post.encoded_image)}/{unc_len} image bytes)"
    )
    return 0


def _read_post(path: str) -> tuple[bytes, Post]:
    with open(path, "rb") as f:
        data = f.read()
    return data, Post.parse(data)


def cmd_inspect(args) -> int:
    with open(args.post, "rb") as f:
        data = f.read()
    try:
        post = Post.parse(data)
    except FormatError as e:
        print(f"INVALID post ({e}); raw length {len(data)} bytes")
        return 1
    print(f"post id:      {post.post_id.hex()}")
    print(f"short id:     {post.short_id.hex()}")
    print(f"author:       {post.author_pubkey.hex()}")
    print(f"fingerprint:  {crypto.hex_fingerprint(post.author_pubkey)}")
    print(f"handle:       {post.handle!r}")
    print(f"caption:      {post.caption!r}")
    print(f"sequence:     {post.seq}")
    print(f"format:       {_format_name(post.pixel_format)}")
    print(f"codec:        {CODEC_NAMES.get(post.codec, str(post.codec))}")
    print(f"image bytes:  {len(post.encoded_image)} encoded / {format_geometry(post.pixel_format)[2]} raw")
    print(f"total bytes:  {len(data)}")
    print("signature:    VALID")
    return 0


def cmd_verify(args) -> int:
    try:
        _data, post = _read_post(args.post)
    except FormatError as e:
        print(f"INVALID: {e}")
        return 1
    print(f"VALID: post {post.post_id.hex()} signed by {crypto.hex_fingerprint(post.author_pubkey)}")
    return 0


def cmd_export(args) -> int:
    try:
        _data, post = _read_post(args.post)
    except FormatError as e:
        print(f"refusing to export invalid post: {e}", file=sys.stderr)
        return 1
    save_mono1_as_png(post.decode_image(), args.output)
    print(f"wrote {args.output} (256x240 PNG)")
    return 0


def cmd_send(args) -> int:
    from . import qr_send

    with open(args.post, "rb") as f:
        data = f.read()
    qr_send.send(data, args.payload_bytes, args.period_ms, protocol=args.protocol)
    return 0


def cmd_receive(args) -> int:
    from . import qr_receive
    from .fragments import FragmentError

    try:
        data = qr_receive.receive(args.camera, args.timeout)
    except (FragmentError, FormatError) as e:
        print(f"receive failed: {e}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("canceled", file=sys.stderr)
        return 1
    with open(args.output, "wb") as f:
        f.write(data)
    print(f"wrote {len(data)} bytes to {args.output}")
    return 0


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="baogram-host", description=__doc__)
    sub = p.add_subparsers(dest="command", required=True)

    kg = sub.add_parser("keygen", help="generate a local host identity seed")
    kg.add_argument("--key", default="baogram-host.key")
    kg.set_defaults(fn=cmd_keygen)

    mp = sub.add_parser("make-post", help="build and sign a post from an image")
    mp.add_argument("--image", required=True)
    mp.add_argument("--handle", default="")
    mp.add_argument("--caption", default="")
    mp.add_argument("--output", required=True)
    mp.add_argument("--key", default="baogram-host.key")
    mp.add_argument("--seq", type=int, default=0)
    mp.add_argument("--threshold", type=int, default=None,
                    help="override the global quantization threshold (0-255)")
    mp.add_argument("--size", choices=("small", "full"), default="small",
                    help="small = 128x120 pixel format 2 (default), "
                    "full = 256x240 pixel format 1")
    mp.set_defaults(fn=cmd_make_post)

    ins = sub.add_parser("inspect", help="print post metadata")
    ins.add_argument("post")
    ins.set_defaults(fn=cmd_inspect)

    ver = sub.add_parser("verify", help="verify a post's digest and signature")
    ver.add_argument("post")
    ver.set_defaults(fn=cmd_verify)

    ex = sub.add_parser("export", help="export a post's image to PNG")
    ex.add_argument("post")
    ex.add_argument("--output", required=True)
    ex.set_defaults(fn=cmd_export)

    sd = sub.add_parser("send", help="display a post as animated QR codes")
    sd.add_argument("post")
    sd.add_argument("--protocol", choices=("v2", "v1"), default="v2",
                    help="v2 = fountain frames (default), v1 = looping fragments")
    sd.add_argument("--payload-bytes", type=int, default=None,
                    help=f"chunk size (default {DEFAULT_FOUNTAIN_PAYLOAD} for v2, "
                    f"{DEFAULT_FRAGMENT_PAYLOAD} for v1)")
    sd.add_argument("--period-ms", type=int, default=500)
    sd.set_defaults(fn=cmd_send)

    rc = sub.add_parser("receive", help="receive a post from animated QR codes")
    rc.add_argument("--camera", type=int, default=0)
    rc.add_argument("--output", required=True)
    rc.add_argument("--timeout", type=float, default=None)
    rc.set_defaults(fn=cmd_receive)

    args = p.parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
