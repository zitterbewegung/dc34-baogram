# Baogram status

Last updated: 2026-08-06 (end of the initial implementation pass).

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
4. **Hosted full-app run**: `cargo xtask baosec-emu` boots xous-core's
   own vault2, not dc34-vault; running dc34-vault hosted end-to-end
   would need a custom service list (and `cargo check --features
   hosted-baosec` in dc34-vault fails inside utralib's build script —
   a pre-existing condition unrelated to Baogram). Hosted coverage is
   therefore at the unit/IPC-helper level, not full-app.
5. `cargo fmt` note: xous-core and dc34-vault use rustfmt.toml options
   that are unstable on stable rustfmt; upstream files are formatted
   with nightly rustfmt and stable `cargo fmt --check` fails repo-wide
   even without Baogram. All Baogram-authored files are formatted; no
   unrelated file was reformatted.
6. Handle editing UI is not wired (the identity module supports it;
   default handles are `anon-<fingerprint>`); captions are empty in the
   badge capture flow (the laptop peer supports both).
7. The share loop's QR is regenerated on the 250 ms UI pump, so
   effective frame periods quantize to multiples of 250 ms.

## Remaining risks

* Camera exposure/threshold interaction on real scenes (adaptive mean
  threshold is proven for QR scanning, not for photography).
* rqrr decode rate on laptop-screen QR codes (moire/refresh artifacts).
* PDDB write latency for ~8 KB posts inside the UI loop (sync is
  performed on save; watch for UI stalls on hardware).

## Smallest next action

Run BAOGRAM_HARDWARE_TEST.md §§4-6 (install on a sacrificial dev badge,
camera preview, capture, save) and fill in the §12 benchmark table.
