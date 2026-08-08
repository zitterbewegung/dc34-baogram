# Baogram status

Last updated: 2026-08-07 (hosted full-app emulation added).

## Summary

All software gates pass. **No physical badge was available in the build
environment, so nothing in this project has been validated on
hardware** — the first-hardware procedure is BAOGRAM_HARDWARE_TEST.md,
and every claim below is explicit about being host/build-level evidence.

## Gate results

| gate | scope | result | evidence |
|------|-------|--------|----------|
| A | untouched console+vault build, baosec-lite packaging | PASS | BAOGRAM_BASELINE.md (exact commands, sizes) |
| B | baogram-core native tests + golden vectors | PASS | 62 tests, `libraries/baogram-core` |
| C | xous-core camera + QR-stream APIs build for riscv32imac-unknown-xous-elf; hosted synthetic frame; AcquireQr untouched; frame/chunk tests | PASS | full `cargo xtask baosec-lite` ensemble build; 7 tests in `bao-video` (incl. golden SHA `c82bfcf7…03db`); AcquireQr code not modified (its retry sequence is mirrored, not moved) |
| D | vault target build with identity/storage/UI; delete removes from index | PASS | `cargo build --release --target riscv32imac-unknown-xous-elf --features board-baosec`; storage unit logic exercised via baogram-core tests; delete path prunes + self-heals index |
| E | outbound: synthetic 256x240 post → fragments → laptop reassembles → signature verifies → PNG pixels exact | PASS (software) | pytest: Rust golden fragments reassembled out-of-order in Python, byte-identical post, signature verified, PNG export/reimport lossless. Over-the-air QR leg untested (needs hardware/webcam+display) |
| F | inbound: Python-generated post accepted by Rust; stream compiles; out-of-order; CRC + signature rejection; verified post stored | PASS (software) | `golden::python_generated_vectors` (Rust parses+verifies Python post, pixel-exact); negative vectors for CRC/signature; badge receive path builds; storage only after verify |
| G | fmt, tests, hardware-target builds, packaging, no unrelated changes | PASS | see below |

## What works (verified by builds/tests, not hardware)

* Complete capture→quantize→sign→store→browse→share→receive→verify→save
  code paths on the badge (compiled for the hardware target and packaged
  into loader/swap/xous UF2 images).
* The canonical protocol round-trips between Rust and Python
  byte-for-byte, including deterministic signatures and fragment lines.
* A synthetic 256x240 image becomes a signed post, fragments, reassembles
  out of order, verifies, decompresses, and reproduces the exact original
  Mono1 pixels (golden-vector + interop tests).
* Hosted mode serves the deterministic synthetic frame through the full
  capture/chunk-read IPC path.

## Known limitations / honest caveats

1. **Nothing hardware-tested**: camera timing, QR ranging/rates, OLED
   legibility of dense QR codes, power behavior — all unmeasured. The
   benchmark table in BAOGRAM_HARDWARE_TEST.md §12 is intentionally
   empty; no data rate is claimed.
2. **QR capacity on a 128x128 OLED**: a 64-byte payload makes a ~135-char
   Base45 QR (version ~7-8 at EC M) rendered at ~2 px/module with the
   existing `render_qr`. If real cameras can't read that density, the
   diagnostics settings (24-byte payloads) are the fallback; this is the
   single biggest hardware risk.
3. **During receive/preview, the display belongs to bao-video**; the
   vault's transfer progress is shown via the generic
   `QrStreamSetStatus` overlay ("Baogram k/n").
4. **Hosted full-app run** (fixed 2026-08-07): dc34-baogram now builds
   with `--features hosted-baosec` (the utralib build-script failure
   was caused by the unconditional `bao1x-hal` board features in
   Cargo.toml, plus missing hosted features on keystore/pddb/modals
   and a stale unpatched `bao1x-emu` git pin). Run the full app hosted
   with
   `cargo xtask baosec-emu ../dc34-baogram/target/release/dc34-vault`
   (xtask drops the stock vault2 when an app binary is given). Hosted
   stand-ins: power/LED servers are absorbed by stub threads
   (`src/hosted.rs`, reports VBUS present), battery reads 4200 mV,
   bitmap-diffusion renders instantly without the dissolve animation,
   and the camera serves the deterministic synthetic frame.
5. `cargo fmt` note: xous-core and dc34-baogram use rustfmt.toml options
   that are unstable on stable rustfmt; upstream files are formatted
   with nightly rustfmt and stable `cargo fmt --check` fails repo-wide
   even without Baogram. All Baogram-authored files are formatted; no
   unrelated file was reformatted.
6. Handle editing UI is not wired (the identity module supports it;
   default handles are `<vault username|bao>-<first 8 hex chars of the
   key fingerprint>`); captions are empty in the
   badge capture flow (the laptop peer supports both).
7. The share loop's QR is regenerated on the 250 ms UI pump, so
   effective frame periods quantize to multiples of 250 ms.

## Adversarial review

A multi-agent adversarial review (find + refutation-verify) ran over all
new code before finalization. Seven confirmed findings were fixed:

1. `AcquireQr` now respects the camera mutual-exclusion invariant
   (previously a concurrent `test qrget` from the console could reset
   the camera under a live Baogram preview and leak a parked envelope).
2. `AcquireQr` invalidates a frozen still before overwriting the frame
   buffer (previously chunk reads could return torn frames).
3. Hosted-mode key presses now abort a QR stream like the board build
   (previously a hosted receive could never be canceled).
4. `∴` on the Baogram profile screen no longer raises the credential
   manager menu over the feed.
5. Badge receive ignores fragments from *other* posts instead of
   aborting the whole transfer when another sender is in view.
6. The host receiver does the same (previously one foreign QR frame
   killed a 30/41-fragment session).
7. The new GfxOpcodes carry explicit discriminants (4096..4103) so
   hosted server and client builds — which resolve different ux-api
   feature sets — agree on opcode numbers.

Accepted (documented, not fixed) minors: the Rust `base45` crate accepts
a noncanonical 2-character tail that Python rejects (CRC+parse still
protect integrity); parsers accept any well-formed PackBits stream, so
post IDs identify bytes, not pixels (see BAOGRAM_PROTOCOL.md); a cancel
key pressed in the sub-second window before the receive worker's stream
starts is absorbed (press again); share-loop periods quantize to the
250 ms UI pump.

## Remaining risks

* Camera exposure/threshold interaction on real scenes (adaptive mean
  threshold is proven for QR scanning, not for photography).
* rqrr decode rate on laptop-screen QR codes (moire/refresh artifacts).
* PDDB write latency for ~8 KB posts inside the UI loop (sync is
  performed on save; watch for UI stalls on hardware).

## Smallest next action

Run BAOGRAM_HARDWARE_TEST.md §§4-6 (install on a sacrificial dev badge,
camera preview, capture, save) and fill in the §12 benchmark table.

## 2026-08-08: protocol v2 — fountain frames + row-delta codec

Motivated by first field use (a real photo needed 84 sequential QR codes
at 64 B / 500 ms, and every missed frame cost a full ~42 s loop):

* **BG v2 fountain frames** (BAOGRAM_PROTOCOL.md §4b): endless rateless
  stream — systematic prefix `0..k`, then deterministic XOR combinations
  (splitmix64 + integer 1/d degree distribution). Any caught frame
  helps; order and loss are irrelevant. Peeling decoder with bounded
  memory (512 frames / 64 KiB pending, oldest evicted). v1 still
  accepted; receivers lock to the first frame's version.
* **Codec 2 RowDeltaMono1** (§2): XOR each 32-byte row with the row
  above, then PackBits. Canonical pick: smaller of codec 1/2, ties to
  the lower id, raw when neither compresses.
* **New sender defaults**: 96-byte chunks @ 250 ms (was 64 B @ 500 ms).
* Measured on the golden synthetic frame: post 2,627 B -> 383 B
  (encoded image 2,439 B -> 195 B); QR count 42 (v1@64) -> 4 (v2@96).
  The synthetic frame is best-case for row-delta; photographs gain less
  from the codec, but the fountain property holds regardless.
* Rust: `cargo test` in baogram-core (77 tests, incl. shuffled/lossy
  fountain decode, cross-language PRNG pins, golden vectors regenerated
  with fountain-frames.b45). Python peer mirrored, both directions.
* Compatibility: old firmware cannot parse codec-2 posts or v2 frames
  (fleet = 1 badge; `baogram-host send --protocol v1` covers legacy).
