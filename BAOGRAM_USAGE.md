# Baogram usage guide

Baogram is signed, offline, monochrome picture-sharing for the DC34
badge: shoot 1-bit photos, sign them with the badge's Ed25519 identity,
and beam them badge-to-badge (or badge-to-laptop) as animated QR codes.
No radio, no server — just cameras and screens.

This is the user-facing guide. Companion documents:

| doc | contents |
|---|---|
| BAOGRAM_PROTOCOL.md | normative wire formats (posts, codecs, QR fragments) |
| BAOGRAM_BUILD.md | building the firmware and host tools |
| BAOGRAM_HARDWARE_TEST.md | first-flash procedure and hardware test plan |
| BAOGRAM_ARCHITECTURE.md / BAOGRAM_DISCOVERY.md | how the app fits into the vault firmware |
| BAOGRAM_STATUS.md | implementation log and measured numbers |

## 1. On the badge

The badge boots straight into the **Baogram feed**. Keys:

| key | in the feed |
|---|---|
| 🔥 (middle) | open the camera |
| ↑ / ↓ | browse posts (wraps; label shows `k/n`) |
| ← | receive a post over QR |
| → | profile (handle, fingerprint, post count) |
| ∴ (home) | post menu: Share / Delete / Author / About / Back / Exit Baogram |

∴ is also SELECT inside every menu.

**Shooting:** 🔥 opens a live preview → 🔥 freezes and captures →
`fire:save left:retake` → 🔥 saves the signed post to the gallery
(32-post cap). Photos are captured at 128x120 (the display's native
resolution), despeckled, and compressed with the best of three codecs —
a typical shot serializes to a few hundred bytes.

**Sharing:** pick a post, ∴ → Share. The badge displays an endless
fountain-coded QR stream (96-byte frames, 4/second). The label reads
`<id> f<frame> k<chunks>`: `k` is how many distinct frames a receiver
needs; `f` counts forever — receivers profit from *any* `k`-ish frames,
in any order, so misses only cost time. Jog ↑/↓ cycles frame payload
size (24..96 B), ←/→ cycles the display period (250..750 ms); any other
key stops. Old posts keep the size they were signed with.

**Receiving:** ← from the feed starts the camera QR stream; aim at the
sender's screen from 5–20 cm. The overlay counts `Baogram k/n`; when
complete the post is verified (signature checked before anything is
stored) and offered for saving. Any key cancels cleanly. Frames from a
second sender in view are ignored, not fatal.

**Launcher:** "Exit Baogram" in the post menu opens the **Baochip
apps** screen (Baogram / Vault / Badge tour / Help / About; ↑↓ pick,
∴ open, ← back to the feed). The vault side's idle menu has an "Apps"
entry to get back.

## 2. Flashing a badge

> [!WARNING]
> The FIRST developer flash of a stock badge permanently erases the
> factory light-encryption key. See BAOGRAM_HARDWARE_TEST.md.

```
# build the UF2 set and flash in one go (macOS):
python3 tools/flash_badge.py --build
# or flash an existing build / snapshot:
python3 tools/flash_badge.py --images ../flash-<rev>
```

Put the badge in update mode when prompted: **hold any button while
plugging in USB** until the `BAOCHIP` drive mounts. The script
validates the UF2 block structure, copies `loader.uf2` → `swap.uf2` →
`xous.uf2` (surviving mid-copy re-enumeration), and unmounts. Then
**press any button on the badge** to boot; the first boot after an
update is slow (PDDB migration). `tools/webflash/` is a browser-based
alternative for people without a shell.

## 3. The emulator (hosted mode)

Run the whole badge app on a host, no hardware needed:

```
# one-time: hosted build of the app
cargo build --release --features hosted-baosec        # in this repo
# run (from the sibling xous-core checkout):
cargo xtask baosec-emu ../dc34-baogram/target/release/dc34-vault
```

A 128x160 window appears (owned by `bao-video`; it often opens
unfocused — click it). Host key map:

| badge key | host key |
|---|---|
| ∴ | Home (**Fn+← on Mac laptops**) |
| 🔥 | Space |
| ↑↓←→ | arrow keys |

**Esc quits the emulator instantly** — avoid it. Enter does not select
in menus; use ∴.

Environment variables (hosted only):

| var | effect |
|---|---|
| `BAO_CAMERA_IMAGE=<file>` | camera serves this raw 256x240 gray8 image instead of the synthetic test frame |
| `BAOGRAM_SEED=1` | auto-capture one photo right after boot, so a fresh feed starts with content |
| `BAOGRAM_TOUR=1` | scripted walk-through of every Baogram screen |
| `BAOGRAM_IMPORT_TEST=1` | drive the serial-upload-becomes-a-post test and log `BAOGRAM IMPORT TEST: PASS`/`FAIL` |
| `BAOGRAM_IMPORT_TEST_EXIT=1` | with the above, exit the process 0/1 on the verdict instead of idling |

Emulator state persists in `xous-core/tools/pddb-images/hosted.bin`
(gallery + identity survive restarts). Delete that file for a factory-
fresh emulator. A ready-made demo image generator:

```
python3 tools/make_test_image.py            # writes ../test-images/simphoto.{gray8,png}
BAO_CAMERA_IMAGE=$PWD/../test-images/simphoto.gray8 BAOGRAM_SEED=1 \
  cargo xtask baosec-emu ../dc34-baogram/target/release/dc34-vault   # from xous-core
```

## 4. The laptop peer (`baogram-host`)

```
cd tools/baogram-host && python3 -m venv .venv && .venv/bin/pip install -e '.[test]'
```

| command | purpose |
|---|---|
| `baogram-host receive --camera 0 --output got.bgrm` | receive a post from a badge via webcam |
| `baogram-host send post.bgrm` | display the animated QR stream (`--protocol v1` for old firmware) |
| `baogram-host make-post --image photo.png --handle alice --output p.bgrm` | build a signed post (`--size full` for 256x240) |
| `baogram-host inspect / verify / export` | examine, check, or convert `.bgrm` posts |

The peer implements the exact same wire formats as the badge (pinned by
cross-language test vectors); posts made on either side verify on the
other. Run the whole host-side test suite with `scripts/test-baogram.sh`.

## 5. Sizes and expectations

A post is `62-byte header + handle + caption + encoded image + 96-byte
digest/signature trailer`, capped at 64 KiB. Compact captures typically
encode to 100–800 bytes → **3–10 QR frames** (~1–3 s at the default
250 ms cadence). Incompressible worst case is ~22 frames; legacy
full-format posts run larger (the pre-compact era needed 42–84 frames).
Received posts keep their original size forever — a signed post is
immutable.
