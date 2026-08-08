# Baogram badge web-flasher logic. Runs inside Pyodide in the browser; the
# pure-Python UF2 validation also runs under CPython for tests:
#   python3 flash.py <file.uf2> [...]  prints a validation report per file.
#
# Browser flow (driven by index.html):
#   1. init() fetches images/{loader,swap,xous}.uf2, validates each, and
#      renders the image table.
#   2. The page's button click obtains a FileSystemDirectoryHandle for the
#      badge's UF2 volume (the picker must run in the JS click handler to
#      keep the user-activation token) and passes it to on_volume_selected().
#   3. on_volume_selected() confirms the volume looks like the badge
#      (named BAOCHIP; the real badge presents a plain FAT drive with NO
#      INFO_UF2.TXT marker, unlike classic soft-UF2 bootloaders), writes
#      the current image, and advances.
#
# The UF2 bootloader reboots the badge after each image, so the volume
# handle goes stale between files; the user re-picks the volume once per
# image. That is a browser-security limit, not a bug.

import hashlib
import struct

UF2_MAGIC_START0 = 0x0A324655  # "UF2\n"
UF2_MAGIC_START1 = 0x9E5D5157
UF2_MAGIC_END = 0x0AB16F30
UF2_BLOCK_SIZE = 512
UF2_FLAG_FAMILY_ID = 0x00002000

# install order matters: loader, then swap, then xous (BAOGRAM_HARDWARE_TEST.md s.4)
FLASH_ORDER = ["loader.uf2", "swap.uf2", "xous.uf2"]


class Uf2Error(ValueError):
    pass


def validate_uf2(data: bytes) -> dict:
    """Check every 512-byte block's magics and geometry; return a report."""
    if len(data) == 0 or len(data) % UF2_BLOCK_SIZE:
        raise Uf2Error(f"size {len(data)} is not a multiple of {UF2_BLOCK_SIZE}")
    n = len(data) // UF2_BLOCK_SIZE
    families = set()
    warnings = []
    total_payload = 0
    for i in range(n):
        blk = data[i * UF2_BLOCK_SIZE : (i + 1) * UF2_BLOCK_SIZE]
        m0, m1, flags, _addr, payload, seq, count = struct.unpack_from("<7I", blk, 0)
        (mend,) = struct.unpack_from("<I", blk, 508)
        if m0 != UF2_MAGIC_START0 or m1 != UF2_MAGIC_START1 or mend != UF2_MAGIC_END:
            raise Uf2Error(f"block {i}: bad UF2 magic")
        if payload > 476:
            raise Uf2Error(f"block {i}: payload length {payload} > 476")
        if (seq, count) != (i, n) and not warnings:
            warnings.append(f"block {i}: sequence {seq}/{count} (multi-part or nonstandard UF2)")
        if flags & UF2_FLAG_FAMILY_ID:
            families.add(struct.unpack_from("<I", blk, 28)[0])
        total_payload += payload
    return {
        "blocks": n,
        "payload_bytes": total_payload,
        "families": sorted(f"0x{f:08X}" for f in families),
        "warnings": warnings,
        "sha256": hashlib.sha256(data).hexdigest(),
    }


# ----------------------------------------------------------------------
# Everything below runs only inside Pyodide.
# ----------------------------------------------------------------------

_images: dict[str, bytes] = {}
_step = 0


def _el(id_):
    import js

    return js.document.getElementById(id_)


def _log(msg: str):
    import js

    pre = _el("log")
    pre.textContent = pre.textContent + msg + "\n"
    pre.scrollTop = pre.scrollHeight
    js.console.log("[flash] " + msg)


def _set_step_ui():
    if _step >= len(FLASH_ORDER):
        _el("stepname").textContent = "done"
        _el("pick").style.display = "none"
        _el("donebox").style.display = "block"
        _log("All three images written. Power-cycle the badge; the first boot "
             "takes longer than usual (PDDB initialization).")
    else:
        name = FLASH_ORDER[_step]
        _el("stepname").textContent = f"step {_step + 1} of {len(FLASH_ORDER)}: write {name}"
        _el("pick").textContent = f"Select the badge volume to write {name}"


async def init():
    from pyodide.http import pyfetch

    rows = []
    for name in FLASH_ORDER:
        resp = await pyfetch(f"images/{name}")
        if resp.status != 200:
            _log(f"ERROR: images/{name} not found (HTTP {resp.status}). "
                 "Run serve.py so the current build is staged.")
            return False
        data = await resp.bytes()
        report = validate_uf2(data)
        _images[name] = data
        for w in report["warnings"]:
            _log(f"note: {name}: {w}")
        rows.append(
            f"<tr><td>{name}</td><td>{len(data):,} B</td>"
            f"<td>{report['blocks']}</td>"
            f"<td><code>{report['sha256'][:16]}…</code></td></tr>"
        )
        _log(f"loaded {name}: {len(data):,} bytes, {report['blocks']} UF2 blocks, "
             f"sha256 {report['sha256']}")
    _el("imgrows").innerHTML = "".join(rows)
    _set_step_ui()
    return True


async def _is_uf2_volume(handle) -> bool:
    # The DC34 badge's update mode is a plain FAT volume named BAOCHIP —
    # it has no INFO_UF2.TXT marker (verified on hardware 2026-08-08).
    # Accept by name; also accept a marker file so classic UF2 drives and
    # renamed volumes still work.
    if str(handle.name).upper() == "BAOCHIP":
        _log("volume 'BAOCHIP' recognized as the badge update drive")
        return True
    try:
        info_fh = await handle.getFileHandle("INFO_UF2.TXT")
        info = await (await info_fh.getFile()).text()
        _log("volume INFO_UF2.TXT: " + info.strip().splitlines()[0])
        return True
    except Exception:
        return False


async def on_volume_selected(handle):
    """Write the current image into the picked directory handle."""
    global _step
    import js
    from pyodide.ffi import to_js

    if _step >= len(FLASH_ORDER):
        return
    name = FLASH_ORDER[_step]
    if not await _is_uf2_volume(handle):
        _log(f"'{handle.name}' does not look like the badge update drive "
             "(expected the volume named BAOCHIP). Put the badge in update "
             "mode — hold any button while plugging in USB — and pick the "
             "BAOCHIP volume.")
        return

    data = _images[name]
    _log(f"writing {name} ({len(data):,} bytes) to '{handle.name}'…")
    wrote = False
    try:
        opts = to_js({"create": True}, dict_converter=js.Object.fromEntries)
        fh = await handle.getFileHandle(name, opts)
        writable = await fh.createWritable()
        await writable.write(to_js(data))
        wrote = True
        await writable.close()
        _log(f"{name} written and closed.")
    except Exception as e:  # the badge reboots as soon as the image lands
        if wrote:
            _log(f"{name}: write completed; the badge re-enumerated before the "
                 f"file handle closed ({type(e).__name__}) — this is normal.")
        else:
            _log(f"ERROR writing {name}: {e}")
            return

    _step += 1
    if _step < len(FLASH_ORDER):
        _log("Wait for the badge volume to disappear and come back, then pick "
             "it again for the next image.")
    _set_step_ui()


if __name__ == "__main__":
    import sys

    for path in sys.argv[1:]:
        with open(path, "rb") as f:
            data = f.read()
        report = validate_uf2(data)
        print(f"{path}: {report['blocks']} blocks, {report['payload_bytes']:,} "
              f"payload bytes, families {report['families']}, "
              f"sha256 {report['sha256']}")
        for w in report["warnings"]:
            print(f"  note: {w}")
