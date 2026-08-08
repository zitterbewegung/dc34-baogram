"""Animated QR sender: display a sequence of frame QR codes.

One frame is Base45-encoded and rendered to a QR at a time (error
correction level M) in an OpenCV window until the window is closed or
'q'/Esc is pressed. Protocol v2 (default) streams BG v2 fountain frames
frame_no 0, 1, 2, ... without ever looping back; v1 loops the fixed BG
v1 fragment sequence.
"""

from __future__ import annotations

import sys

from .fragments import (
    DEFAULT_FOUNTAIN_PAYLOAD,
    DEFAULT_FRAGMENT_PAYLOAD,
    fountain_frames,
    fragment_post,
)
from .format import Post


def render_qr_image(text: str, box_size: int = 8, border: int = 4):
    """Render Base45 text to a numpy grayscale image via the qrcode lib."""
    import numpy as np
    import qrcode

    qr = qrcode.QRCode(
        error_correction=qrcode.constants.ERROR_CORRECT_M,
        box_size=box_size,
        border=border,
    )
    qr.add_data(text)
    qr.make(fit=True)
    img = qr.make_image(fill_color="black", back_color="white").convert("L")
    return np.array(img)


def send(
    post_bytes: bytes,
    payload_bytes: int | None = None,
    period_ms: int = 500,
    window: str = "baogram-send",
    protocol: str = "v2",
) -> None:
    import cv2

    post = Post.parse(post_bytes)  # refuse to transmit an invalid post

    if protocol == "v1":
        if payload_bytes is None:
            payload_bytes = DEFAULT_FRAGMENT_PAYLOAD
        frags = fragment_post(post.short_id, post_bytes, payload_bytes)
        print(
            f"post {post.post_id.hex()} ({len(post_bytes)} bytes) -> "
            f"{len(frags)} fragments of <= {payload_bytes} bytes, {period_ms} ms/frame"
        )

        def frame_stream():
            idx = 0
            while True:
                frag = frags[idx % len(frags)]
                yield (
                    frag.to_base45(),
                    f"{frag.frag_index + 1}/{frag.frag_count}  {post.short_id.hex()}",
                )
                idx += 1

    elif protocol == "v2":
        if payload_bytes is None:
            payload_bytes = DEFAULT_FOUNTAIN_PAYLOAD
        k = -(-len(post_bytes) // payload_bytes)  # ceil division
        print(
            f"post {post.post_id.hex()} ({len(post_bytes)} bytes) -> "
            f"fountain of {k} chunks x {payload_bytes} bytes, {period_ms} ms/frame"
        )

        def frame_stream():
            for frame in fountain_frames(post.short_id, post_bytes, payload_bytes):
                yield (
                    frame.to_base45(),
                    f"frame {frame.frame_no + 1} • {frame.k} chunks  {post.short_id.hex()}",
                )

    else:
        raise ValueError(f"unknown protocol {protocol!r}")

    cv2.namedWindow(window, cv2.WINDOW_AUTOSIZE)
    try:
        for text, label in frame_stream():
            img = render_qr_image(text)
            labeled = _label(img, label)
            cv2.imshow(window, labeled)
            key = cv2.waitKey(period_ms) & 0xFF
            if key in (ord("q"), 27):  # q or Esc
                break
            if cv2.getWindowProperty(window, cv2.WND_PROP_VISIBLE) < 1:
                break
    finally:
        cv2.destroyAllWindows()


def _label(img, text: str):
    import cv2
    import numpy as np

    bar = np.full((28, img.shape[1]), 255, dtype=img.dtype)
    out = np.vstack([img, bar])
    cv2.putText(out, text, (8, out.shape[0] - 9), cv2.FONT_HERSHEY_SIMPLEX, 0.5, 0, 1, cv2.LINE_AA)
    return out


def main(argv=None):  # pragma: no cover - thin wrapper
    import argparse

    p = argparse.ArgumentParser(description="Send a Baogram post as animated QR codes")
    p.add_argument("post", help="path to a .bgrm post file")
    p.add_argument("--protocol", choices=("v2", "v1"), default="v2")
    p.add_argument(
        "--payload-bytes",
        type=int,
        default=None,
        help=f"chunk size (default {DEFAULT_FOUNTAIN_PAYLOAD} for v2, "
        f"{DEFAULT_FRAGMENT_PAYLOAD} for v1)",
    )
    p.add_argument("--period-ms", type=int, default=500)
    args = p.parse_args(argv)
    with open(args.post, "rb") as f:
        send(f.read(), args.payload_bytes, args.period_ms, protocol=args.protocol)
    return 0


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
