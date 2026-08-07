"""Animated QR sender: display a looping sequence of fragment QR codes.

One fragment is Base45-encoded and rendered to a QR at a time (error
correction level M), shown in an OpenCV window; the sequence loops until
the window is closed or 'q'/Esc is pressed.
"""

from __future__ import annotations

import sys

from .fragments import DEFAULT_FRAGMENT_PAYLOAD, fragment_post
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
    payload_bytes: int = DEFAULT_FRAGMENT_PAYLOAD,
    period_ms: int = 500,
    window: str = "baogram-send",
) -> None:
    import cv2

    post = Post.parse(post_bytes)  # refuse to transmit an invalid post
    frags = fragment_post(post.short_id, post_bytes, payload_bytes)
    print(
        f"post {post.post_id.hex()} ({len(post_bytes)} bytes) -> "
        f"{len(frags)} fragments of <= {payload_bytes} bytes, {period_ms} ms/frame"
    )
    idx = 0
    cv2.namedWindow(window, cv2.WINDOW_AUTOSIZE)
    try:
        while True:
            frag = frags[idx % len(frags)]
            img = render_qr_image(frag.to_base45())
            labeled = _label(img, f"{frag.frag_index + 1}/{frag.frag_count}  {post.short_id.hex()}")
            cv2.imshow(window, labeled)
            key = cv2.waitKey(period_ms) & 0xFF
            if key in (ord("q"), 27):  # q or Esc
                break
            if cv2.getWindowProperty(window, cv2.WND_PROP_VISIBLE) < 1:
                break
            idx += 1
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
    p.add_argument("--payload-bytes", type=int, default=DEFAULT_FRAGMENT_PAYLOAD)
    p.add_argument("--period-ms", type=int, default=500)
    args = p.parse_args(argv)
    with open(args.post, "rb") as f:
        send(f.read(), args.payload_bytes, args.period_ms)
    return 0


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
