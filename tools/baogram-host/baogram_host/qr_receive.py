"""QR receiver: watch a camera, decode Base45 fragment QR codes, and
reassemble them (any order, duplicates ignored, conflicts fatal) into a
verified Baogram post.
"""

from __future__ import annotations

import sys
import time

from .format import FormatError, Post
from .fragments import Fragment, FragmentError, Reassembler


def receive(camera_index: int = 0, timeout_s: float | None = None) -> bytes:
    """Blocks until a complete, signature-verified post is received.
    Returns the serialized post bytes. Raises TimeoutError on timeout and
    FragmentError('conflict') on conflicting fragments."""
    import cv2

    cap = cv2.VideoCapture(camera_index)
    if not cap.isOpened():
        raise RuntimeError(f"cannot open camera {camera_index}")
    detector = cv2.QRCodeDetector()
    reassembler: Reassembler | None = None
    seen_texts_last = None
    decode_times: list[float] = []
    start = time.monotonic()
    window = "baogram-receive"
    cv2.namedWindow(window, cv2.WINDOW_AUTOSIZE)
    try:
        while True:
            if timeout_s is not None and time.monotonic() - start > timeout_s:
                raise TimeoutError("receive timed out")
            ok, frame = cap.read()
            if not ok:
                continue
            text, _points, _ = detector.detectAndDecode(frame)
            status = "waiting for fragments..."
            if reassembler is not None:
                status = f"{reassembler.received_count}/{reassembler.frag_count}"
            if text:
                if text != seen_texts_last:
                    seen_texts_last = text
                    decode_times.append(time.monotonic())
                    try:
                        frag = Fragment.from_base45(text)
                    except FragmentError as e:
                        print(f"ignoring undecodable QR: {e}", file=sys.stderr)
                        frag = None
                    if frag is not None:
                        if reassembler is None:
                            reassembler = Reassembler(frag)
                        else:
                            result = reassembler.feed(frag)  # conflict raises
                            if result == "duplicate":
                                pass
                        print(
                            f"fragment {frag.frag_index + 1}/{frag.frag_count} "
                            f"({reassembler.received_count}/{reassembler.frag_count} held)"
                        )
                        if reassembler.is_complete:
                            data = reassembler.to_bytes()
                            # verify BEFORE reporting success
                            post = Post.parse(data)  # raises FormatError if invalid
                            if len(decode_times) > 1:
                                intervals = [
                                    b - a for a, b in zip(decode_times, decode_times[1:])
                                ]
                                avg = sum(intervals) / len(intervals)
                                print(f"avg decode interval: {avg * 1000:.0f} ms")
                            print(
                                f"complete: post {post.post_id.hex()} by "
                                f"{post.handle or 'anon'} — signature verified"
                            )
                            return data
            cv2.putText(
                frame, status, (10, 30), cv2.FONT_HERSHEY_SIMPLEX, 0.8, (0, 255, 0), 2
            )
            cv2.imshow(window, frame)
            key = cv2.waitKey(1) & 0xFF
            if key in (ord("q"), 27):
                raise KeyboardInterrupt("canceled")
    finally:
        cap.release()
        cv2.destroyAllWindows()


def main(argv=None):  # pragma: no cover - thin wrapper
    import argparse

    p = argparse.ArgumentParser(description="Receive a Baogram post from animated QR codes")
    p.add_argument("--camera", type=int, default=0)
    p.add_argument("--output", required=True)
    p.add_argument("--timeout", type=float, default=None)
    args = p.parse_args(argv)
    try:
        data = receive(args.camera, args.timeout)
    except (FragmentError, FormatError) as e:
        print(f"receive failed: {e}", file=sys.stderr)
        return 1
    with open(args.output, "wb") as f:
        f.write(data)
    print(f"wrote {len(data)} bytes to {args.output}")
    return 0


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
