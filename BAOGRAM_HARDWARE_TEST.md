# Baogram hardware test plan

> [!WARNING]
> **1. Developer-mode / factory-key warning.** Installing a
> developer-signed image on a DC34 badge **erases the factory light
> encryption key and permanently switches the badge to developer mode**.
> The conference light-gene exchange with stock badges will no longer
> interoperate, and the factory state cannot be restored by you or by
> the project. Do not proceed on a badge you are not willing to convert.
> Nothing in this repository flashes hardware automatically.

None of the tests below have been executed on physical hardware as of
this writing (no badge was available in the build environment); every
software-only equivalent has been run and is recorded in
BAOGRAM_STATUS.md. This file is the procedure for the first hardware
validation pass.

## 2. Preserve the official images first

Download and archive the official release **before** flashing anything:
<https://ci.betrusted.io/releases/latest/baochip/dc34-badge/latest.zip>
(contains `xous.uf2`, `swap.uf2`, `loader.uf2`). Store it off-device.
Re-installing these official images later still leaves the badge in
developer mode (the factory key is gone), but restores stock behavior
under the developer key.

## 3. Identify the USB device

1. Detach the core module (or use the badge whole) and connect via USB-C.
2. Put the badge into update mode per the dc34-vault README ("Updates"
   section): the badge enumerates as a mass-storage device (UF2
   bootloader).
3. On macOS it mounts as a volume (check `diskutil list` /
   `ls /Volumes`); on Linux check `lsblk` and mount the FAT volume.
   Confirm the volume contains the UF2 marker files before copying
   anything.

## 4. Install the development build

1. Build per BAOGRAM_BUILD.md; products in
   `xous-core/target/riscv32imac-unknown-xous-elf/release/`.
2. Copy, one at a time, waiting for the device to re-enumerate between
   files if it does: `loader.uf2`, then `swap.uf2`, then `xous.uf2`.
3. Power-cycle. First boot after an update takes longer (PDDB
   migration/initialization).
4. Expected: the usual vault UI; `Baogram` appears in the badge menu
   (center press on the idle screen).

## 5. Camera preview test

1. Idle screen → center press → **Baogram** → feed shows
   "No posts yet" on first run.
2. Press the middle (fire) button: "Starting camera..." then a live
   ~2 fps-or-better mono preview on the OLED.
3. Confirm the preview tracks the scene and exposure adapts (the
   threshold is adaptive, so waving a hand should modulate it).
4. Any key other than fire returns to the feed and the camera LED/power
   goes down. **PASS =** preview live, cancel works, UI redraws.

## 6. Camera capture test

1. In camera mode, press fire. The display should show the frozen
   preview screen with the "fire:save left:retake" label within ~1 s.
2. Retake (left) must restart the preview; capture again.
3. Save (fire): returns to the feed showing the new post as 1/1 with
   your `XXXXXXXX` (fingerprint-prefix) handle. **PASS =** photo visibly matches the
   scene at 128x120, save persists across a power cycle.

## 7. Frame SHA test (hosted equivalence)

On the host, `cargo test -p bao-video --features
hosted-baosec,modals/hosted-baosec` pins the synthetic frame to SHA-256
`c82bfcf7…03db`. On hardware, capture a lens-covered (all-dark) frame
and a lens-to-light frame and confirm via the gallery that the images
are all-black / all-white respectively — this validates the capture path
end-to-end (a hardware frame is scene-dependent, so no fixed SHA
applies; log output includes capture/read/quantize timings for the
metrics table).

## 8. Local save / gallery test

1. Capture and save 3+ posts. Browse with jog up/down; position label
   must show `k/n` and wrap around.
2. Power-cycle; the gallery must reload identically (order preserved).
3. Delete the middle post via center-press → Delete; confirm the count
   drops and the index heals (no "missing" placeholder).
4. Fill to 32 posts (or temporarily lower MAX_POSTS in a test build):
   the 33rd save must be refused with "gallery full" and the pending
   photo must remain on the preview screen.

## 9. Badge → laptop transfer

1. Laptop: `baogram-host receive --camera 0 --output received.bgrm`.
2. Badge: select a post → center press → Share. An animated QR loop
   plays (default 64-byte payloads at 500 ms).
3. Hold the badge 10-20 cm from the webcam. The laptop prints fragment
   progress; on completion it verifies the signature automatically.
4. `baogram-host export received.bgrm --output received.png` and
   compare: `baogram-host inspect received.bgrm` shows the badge's
   handle and post ID. **PASS =** signature VALID and the PNG matches
   what the badge displays (pixel-exact vs the badge's stored post can
   be confirmed by re-sharing the exported PNG, see §10).

## 10. Laptop → badge transfer

1. Laptop: `baogram-host make-post --image photo.png --handle alice
   --caption hi --output post.bgrm && baogram-host send post.bgrm`.
2. Badge: feed → jog left (Receive). "Starting camera...", then the
   stream overlay counts `Baogram k/n`.
3. Aim the badge camera at the laptop screen; after all fragments the
   badge shows the verify preview. Save; confirm it appears in the feed
   with handle `alice`.
4. Negative: press a key mid-transfer — the badge must return to the
   feed cleanly and the camera must power down.

## 11. Badge → badge transfer

One badge Shares (§9 steps 2), the other Receives (§10 steps 2-3),
face to face at 5-15 cm. **PASS =** complete transfer, signature
verified, post saved and browsable on the receiver.

## 12. QR payload/rate benchmark

In Share mode the jog dial cycles diagnostics: up/down = payload size
(24/40/64/80/96 bytes), left/right = frame period (250/400/500/750 ms).
For each combination, receive a fixed post on the laptop and record the
wall-clock to completion and the receiver's "avg decode interval" line.
Fill in:

| payload (B) | period (ms) | fragments | transfer time (s) | effective B/s | notes |
|-------------|-------------|-----------|-------------------|---------------|-------|
| 24          | 250         |           |                   |               |       |
| 40          | 400         |           |                   |               |       |
| 64          | 500         |           |                   |               |       |
| 80          | 500         |           |                   |               |       |
| 96          | 750         |           |                   |               |       |

Do not quote a data rate until this table is measured on hardware.

## 13. Power-loss and cancel tests

* Cancel in every mode (camera preview, share loop, receive
  mid-transfer, preview screen): the badge must return to a sane feed
  screen with the camera off each time.
* Yank power mid-save and mid-receive: on reboot the gallery must load
  (the index self-heals; a partially written post is pruned), and
  nothing must panic.

## 14. Invalid-signature test

1. `baogram-host make-post … --output evil.bgrm`, then flip one byte of
   the image region: `python3 -c "d=bytearray(open('evil.bgrm','rb').read()); d[100]^=1; open('evil.bgrm','wb').write(bytes(d))"`.
2. `baogram-host verify evil.bgrm` must print INVALID (digest_mismatch).
3. `baogram-host send evil.bgrm` must **refuse to transmit**. To test
   the badge path, temporarily patch qr_send to skip verification (or
   craft fragments from the tampered bytes); the badge must show
   "Receive failed: invalid post" and store nothing.

## 15. Recovery instructions

* **Badge unresponsive after flashing**: re-enter the UF2 bootloader
  (per the README update procedure) and re-copy the three official UF2s
  archived in §2.
* **PDDB corrupted / gallery weird**: the index self-heals on load; if
  the PDDB itself is damaged, the platform's standard PDDB reformat via
  the console applies (this erases stored posts and identity; a new
  identity is generated on next Baogram launch).
* **Camera wedged** (no preview, "camera busy"): bao-video reboots the
  system automatically if the camera fails to start after 3 retries;
  a power cycle clears any residual state.
