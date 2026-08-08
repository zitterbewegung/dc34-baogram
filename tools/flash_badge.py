#!/usr/bin/env python3
"""Build and/or flash a Baogram UF2 image set onto a DC34 badge (macOS).

The badge's UF2 bootloader presents a plain FAT volume named BAOCHIP
(no INFO_UF2.TXT marker file, unlike classic soft-UF2 drives). Flashing
is three file copies in a fixed order, a flush, and a clean unmount:

    loader.uf2  ->  swap.uf2  ->  xous.uf2

Usage:
    # flash the current build products (waits for the badge to appear)
    python3 tools/flash_badge.py

    # rebuild the UF2 set first (scripts/build-baogram.sh), then flash
    python3 tools/flash_badge.py --build

    # flash a snapshot directory instead of the build tree
    python3 tools/flash_badge.py --images ../flash-fc1b8d1

To enter update mode: press and HOLD any badge button while plugging the
module into the computer; keep holding until the BAOCHIP drive mounts.
After the copies finish, PRESS ANY BUTTON on the badge to boot - that
flushes the last sector; do not just unplug. First boot after an update
is slow (PDDB migration/initialization).

WARNING (first flash of a stock badge only): installing a
developer-signed image permanently erases the factory light-encryption
key and switches the badge to developer mode. This cannot be undone.
See BAOGRAM_HARDWARE_TEST.md before flashing a badge you care about.
"""

from __future__ import annotations

import argparse
import pathlib
import shutil
import struct
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_IMAGES = REPO.parent / "xous-core" / "target" / "riscv32imac-unknown-xous-elf" / "release"
IMAGE_ORDER = ["loader.uf2", "swap.uf2", "xous.uf2"]
VOLUME_NAME = "BAOCHIP"
UF2_MAGIC0 = 0x0A324655  # "UF2\n"
UF2_MAGIC1 = 0x9E5D5157


def fail(msg: str) -> "NoReturn":  # noqa: F821 - py3.9-friendly
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def validate_uf2(path: pathlib.Path) -> int:
    """Sanity-check UF2 structure: 512-byte blocks, magic in every block.
    Returns the block count."""
    data = path.read_bytes()
    if not data or len(data) % 512 != 0:
        fail(f"{path.name}: size {len(data)} is not a multiple of 512 (not UF2)")
    blocks = len(data) // 512
    for i in range(blocks):
        m0, m1 = struct.unpack_from("<II", data, i * 512)
        if m0 != UF2_MAGIC0 or m1 != UF2_MAGIC1:
            fail(f"{path.name}: block {i} has bad UF2 magic (corrupt file?)")
    return blocks


def badge_volume() -> pathlib.Path | None:
    vol = pathlib.Path("/Volumes") / VOLUME_NAME
    return vol if vol.is_dir() else None


def wait_for_badge(timeout_s: float, verb: str) -> pathlib.Path:
    start = time.monotonic()
    announced = False
    while time.monotonic() - start < timeout_s:
        vol = badge_volume()
        if vol is not None:
            return vol
        if not announced:
            print(
                f"waiting for /Volumes/{VOLUME_NAME} to {verb} - hold any badge "
                "button while plugging in USB...",
                flush=True,
            )
            announced = True
        time.sleep(1.0)
    fail(f"no {VOLUME_NAME} volume appeared within {int(timeout_s)} s")


def flush_and_unmount() -> None:
    subprocess.run(["sync"], check=False)
    r = subprocess.run(
        ["diskutil", "unmount", f"/Volumes/{VOLUME_NAME}"], capture_output=True, text=True
    )
    if r.returncode == 0:
        print(f"unmounted {VOLUME_NAME}: all writes flushed")
    else:
        # the badge may have re-enumerated on its own; that also flushes
        print(f"note: unmount reported: {r.stderr.strip() or r.stdout.strip()}")


def main() -> int:
    p = argparse.ArgumentParser(
        description="Flash the Baogram UF2 set onto a DC34 badge (macOS)",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__.split("Usage:")[1],
    )
    p.add_argument(
        "--images",
        type=pathlib.Path,
        default=DEFAULT_IMAGES,
        help=f"directory holding {', '.join(IMAGE_ORDER)} (default: the xous-core build tree)",
    )
    p.add_argument(
        "--build",
        action="store_true",
        help="run scripts/build-baogram.sh first (rebuilds the full UF2 set)",
    )
    p.add_argument("--timeout", type=float, default=600.0, help="seconds to wait for the badge")
    p.add_argument("--yes", action="store_true", help="skip the confirmation prompt")
    args = p.parse_args()

    if sys.platform != "darwin":
        fail("this script drives macOS diskutil; on Linux, mount the drive and cp the files")

    if args.build:
        script = REPO / "scripts" / "build-baogram.sh"
        print(f"building UF2 set via {script} (this takes a few minutes)...")
        r = subprocess.run(["bash", str(script)])
        if r.returncode != 0:
            fail("build failed - fix the build before flashing")

    images = {}
    for name in IMAGE_ORDER:
        path = args.images / name
        if not path.is_file():
            fail(f"missing {path} (build first: --build, or point --images at a snapshot)")
        blocks = validate_uf2(path)
        images[name] = path
        print(f"{name}: {path.stat().st_size} bytes, {blocks} UF2 blocks - ok")

    if not args.yes:
        print(
            "\nFlashing replaces the badge firmware. On a STOCK badge this "
            "permanently erases the factory key (see BAOGRAM_HARDWARE_TEST.md)."
        )
        if input("type 'flash' to continue: ").strip().lower() != "flash":
            print("aborted")
            return 1

    vol = badge_volume() or wait_for_badge(args.timeout, "mount")
    print(f"badge volume: {vol}")

    for name in IMAGE_ORDER:
        src = images[name]
        # the bootloader may re-enumerate at any point after ingesting an
        # image — including in the middle of the NEXT file's copy. UF2
        # blocks are address-tagged, so re-copying the whole file after a
        # re-mount is always safe.
        for attempt in range(1, 6):
            if badge_volume() is None:
                print("volume vanished (re-enumeration); waiting for it to return...")
                vol = wait_for_badge(args.timeout, "re-mount")
            dst = vol / name
            retry = f" (attempt {attempt})" if attempt > 1 else ""
            print(f"copying {name} ({src.stat().st_size} bytes){retry}...", flush=True)
            try:
                shutil.copyfile(src, dst)
                subprocess.run(["sync"], check=False)
                # give a re-enumerating bootloader a moment before the next copy
                time.sleep(1.0)
                break
            except OSError as e:
                print(f"{name}: device went away mid-copy ({e.strerror}); retrying after re-mount")
                time.sleep(2.0)
        else:
            fail(f"{name}: copy failed 5 times - power-cycle the badge and retry")

    flush_and_unmount()
    print(
        "\nDone. PRESS ANY BUTTON on the badge to boot (required to flush "
        "the last sector), then expect a slow first boot (PDDB init). "
        "The badge should land on the Baogram feed."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
