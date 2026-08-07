#!/usr/bin/env python3
"""Stage the current UF2 images and serve the Baogram web flasher locally.

Usage:
    python3 serve.py [--images DIR] [--port PORT] [--no-browser]

By default the images are copied from the sibling xous-core build output
(../../../xous-core/target/riscv32imac-unknown-xous-elf/release). The page
is served on http://127.0.0.1:<port>/ — localhost is a secure context, so
the File System Access API works without TLS.
"""

import argparse
import http.server
import shutil
import sys
import webbrowser
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_IMAGES = HERE.parent.parent.parent / "xous-core" / "target" / \
    "riscv32imac-unknown-xous-elf" / "release"
UF2S = ["loader.uf2", "swap.uf2", "xous.uf2"]


class NoCacheHandler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(HERE), **kwargs)

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--images", type=Path, default=DEFAULT_IMAGES,
                    help="directory containing loader.uf2/swap.uf2/xous.uf2")
    ap.add_argument("--port", type=int, default=8342)
    ap.add_argument("--no-browser", action="store_true")
    args = ap.parse_args()

    dest = HERE / "images"
    dest.mkdir(exist_ok=True)
    for name in UF2S:
        src = args.images / name
        if not src.is_file():
            print(f"error: {src} not found — build first with "
                  "scripts/build-baogram.sh", file=sys.stderr)
            return 1
        shutil.copy2(src, dest / name)
        print(f"staged {name} ({src.stat().st_size:,} bytes)")

    url = f"http://127.0.0.1:{args.port}/"
    print(f"serving the flasher at {url}  (Ctrl-C to stop)")
    if not args.no_browser:
        webbrowser.open(url)
    with http.server.ThreadingHTTPServer(("127.0.0.1", args.port),
                                         NoCacheHandler) as httpd:
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
