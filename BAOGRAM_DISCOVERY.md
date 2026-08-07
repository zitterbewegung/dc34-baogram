# Baogram discovery notes

Source orientation for the Baogram MVP. Revisions inspected:
`xous-core` @ `5d5bbbf` (dev, 2026-08-03; branch `feature/baogram-camera-api`),
`dc34-vault` @ `3d5cbf7` (branch `feature/baogram-app`), `dc34-api` @ `617f0f3`,
`dc34-console` @ `bf64e03`.

## 1. Camera / video service (`xous-core/services/bao-video`)

bao-video is simultaneously the **Gfx server** (`SERVER_NAME_GFX = "_Graphics_"`,
`libs/ux-api/src/service/api.rs:27`) and the camera owner; QR decode is
co-located to avoid cross-process frame copies (main.rs:53-68).

* **Frame buffer**: one main-loop local `let mut frame = [0u8; 256*240]`
  (main.rs:434) — 61,440 bytes of 8-bit luminance. Filled in the
  `GfxOpcode::CamIrq` arm from the camera's UDMA IFRAM: `cam.rx_buf::<u32>()`
  yields YCbYCr u16 pairs; the Y bytes are extracted as
  `(u32 >> 8) & 0xff` and `(u32 >> 24) & 0xff` (main.rs:647-665).
* **Camera**: GC2145 (`bao1x-hal/src/gc2145`), initialized at
  `Resolution::Res320x240` with slicing `((cols-256)/2, 0)..(cols-border, 240)`
  → exactly 256x240 (main.rs:538-541).
* **AcquireQr state machine** (main.rs:452-598, board only):
  deferred-reply envelope `qr_request: Option<MessageEnvelope>`; camera
  power-up **retry loop** (RETRY_LIMIT=3): UDMA cam reset, PRST_N pulse on
  PA6, display re-init + waitscreen blit, MCLK pin AF3 + PWM timer start,
  PDWN low, `read_id`, `init`, slicing, `hal.set_preemption(false)`,
  `capture_async()`, then spin up to 2.5 s for the first IRQ
  (`cam_irq.got_irq`); reboot via susres if all retries fail.
* **Per-frame** (`CamIrq` arm): copy Y channel; if `qr_request.is_some()`
  → `capture_async()` again, else **power camera down** (PWDN high, stop
  MCLK, tri-state clock pin) and `continue`; then `qr::find_finders`
  (1:1:3:1:1 run-length scan, src/qr.rs), `blit_to_display` (2:1 decimation
  to the 128x128 OLED with adaptive mean threshold `bw_thresh`), and full
  rqrr decode only when 3 finder candidates. Success replies through the
  envelope, restores preemption, clears display.
* **Abort**: any key except accelerometer chars `🔽 🔼` takes `qr_request`
  and replies `content: None` (main.rs:599-646).
* **Preemption**: `Hal::set_preemption` (bao1x-hal-service) is disabled
  during acquisition; bao-video still blocks in `reply_and_receive_next`,
  so other processes run between frames.
* **Display**: `Oled128x128` (SH1107, 128x128), ops return `Result` with
  `display_timeout_handler` / `retry_display_op` SPI-reset recovery.
  `display.stash()/pop()` save/restore UI. `gfx::msg(...)` draws 6x12 text
  and takes `(flip, screen_size)` args at this revision.
* **Hosted mode**: `bao1x-emu::camera::Gc2145` is a **stub** — zeroed
  frame, `capture_async()` empty, no IRQ ever fires, so the whole camera
  path is dead hosted; `AcquireQr` opcode does not even exist
  (`#[cfg(feature = "board-baosec")]`), and the client
  `Gfx::acquire_qr` hosted variant returns a canned otpauth string.
  Hosted `SetPreemptionState` is `todo!()` in bao1x-emu, but bao-video's
  `hal` binding is cfg'd out hosted, so nothing calls it. **A hosted
  synthetic frame must be generated inside bao-video itself.**
  Display hosted = minifb window (`bao1x-emu/src/display.rs`).

## 2. ux-api service layer (`xous-core/libs/ux-api`)

* **GfxOpcode** (service/api.rs:30-129): implicit discriminants,
  `num_derive::FromPrimitive`, dispatched with
  `from_usize(...).unwrap_or(InvalidCall)`. Several variants are
  cfg-gated (`ditherpunk`, `bao1x`, `board-baosec`), so discriminants
  shift with features; the tail is `... DryRun, InvalidCall, Quit`.
  **New opcodes must be appended after `Quit`** so every existing value
  is untouched.
* **Chunked-transfer precedent**: `BulkRead { buf: [u8; 7936],
  from_offset: u32, len: u32 }` (api.rs:142-150) — 7,936-byte payload is
  the proven IPC page-sized envelope. 61,440 = 7 x 7,936 + 5,888 → 8 chunks.
* **IPC style**: rkyv `Archive/Serialize/Deserialize` structs sent with
  `Buffer::into_buf(x).lend_mut(conn, opcode)`, read server-side with
  `Buffer::from_memory_message_mut(...)`, replied by `.replace(resp)`
  (deferred replies park the `MessageEnvelope`; the reply happens when it
  drops). Blocking scalars use `msg.body.scalar_message_mut()` and set
  `scalar.arg1`.
* Client `Gfx` (service/gfx.rs): `acquire_qr` (board), `render_qr`,
  `bitmap`/`bitmap_diffusion` (`BaosecBitmap { bits: [u32; 512], top_left,
  bounding_box }`), `brightness`, `flip_screen`, `dry_run`,
  `register_listener` (keyboard fan-out via bao-video).

## 3. dc34-vault application structure

* **Live UI** is `src/ux.rs` (`VaultUi`); `src/ux/framework.rs`,
  `src/ux/icontray.rs`, `src/prereqs.rs` are dead legacy GAM code, not in
  the module tree. Do not imitate them.
* **VaultMode** (src/main.rs:55-73): `Idle, IdleDevMode, ShowKey{quantum},
  ResponseGene{quantum}, ConfirmGene, GeneScan, FactoryTest,
  StandAloneTest, Tour, TokenTour, DefconHelp, About, Totp, Password,
  TokenHelp` — rkyv-derived, shared as `Arc<Mutex<VaultMode>>`.
  `should_animate()` (main.rs:76-94) and
  `GlobalConfig::update_power_state` (config.rs:261-277) match
  exhaustively → adding variants forces updating them (good).
* **VaultOp** (src/vault_api.rs:25-76): implicit discriminants 0..;
  `HandleQr`, `AbortQr` are last before a hard-coded block
  `ImageLoad = 1024, Jig = 1025, SkipKey = 1026, BioActive = 1027`
  (dc34-console sends those integers raw). **Append new ops after
  `AbortQr`, keep them < 1024, never reorder.**
* **ActionOp** (src/actions.rs:46-67): menu ops, `UpdateMode`,
  `UpdateOneItem`, `ReloadDb`, `Quit`, `AcquireQr`, then
  `#[cfg(feature = "vault-testing")] GenerateTests` — append new variants
  **before** the cfg'd trailing variant. Runs on its own thread
  (`action_handler.rs`), receives blocking scalars via
  `msg_blocking_scalar_unpack!` and memory msgs via `Buffer`.
* **Buttons** (chars delivered to `VaultOp::KeyPress` via
  `gfx.register_listener(SERVER_NAME_VAULT2, ...)`, main.rs:234):
  `'↑' '↓' '←' '→'` jog dial, `'∴'` center press (raises menus),
  `'🔥'` middle "fire" button (camera/scan), `'🔼' '🔽'` orientation
  events, `'⏰'` RTC. Routing: menus first when `menu_active`, else
  `vault_ui.handle_key(k)` (per-mode filter, src/ux.rs:1489-1715), then
  main-loop `match` on the returned char.
* **Rendering**: direct `Gfx` calls in `VaultUi::redraw`
  (src/ux.rs:779-1443, per-mode match): `clear`, `bitmap`
  (`[u32; 512]` = 128x128 1bpp, bit set = dark), `draw_textview`
  (`TextBounds::CenteredTop/CenteredBot`), rectangles,
  `ScrollableList` widget, `render_qr(&Vec<bool>, width, Point)`;
  animation via `crate::totp::pumper` sending `VaultOp::Redraw`
  continuously, gated by `should_animate()`.
* **QR show pattern** (reuse for Baogram share): main-loop-owned
  `vault_ui.qr_override: Option<QrCode>` + mode variant with a `quantum`
  counter; redraw arm re-renders QR when `quantum & 7 == 0`, overlays a
  3-char label when `quantum & 7 == 6`
  (ux.rs:920-958). QR built with
  `QrCode::with_error_correction_level(base45.as_bytes(), EcLevel::M)`.
* **QR scan pattern** (reuse for Baogram receive): `'🔥'` →
  `camera_transition()` splash → blocking `ActionOp::AcquireQr` →
  `ActionManager::acquire_qr()` → `gfx.acquire_qr()`; in Gene/Idle-class
  modes the decoded string is repatriated to the main loop as
  `VaultOp::HandleQr` + `IpcString`; abort → `VaultOp::AbortQr`.
* **Menus**: `MenuMatic` instances each on an own server
  (idlemenu.rs, genemenu.rs 33 lines — clone for Baogram menus);
  modals (`modals::Modals`) for text entry
  (`alert_builder(...).field(...)`), radio (`add_list`+`get_radiobutton`),
  notifications, dynamic notifications, progress.
* **Custom image precedent**: PDDB `dc34/image` = 2,048-byte
  `[u32; 512]` bitmap (bytemuck-cast), uploaded in 32 chunks of
  `u16 BE index | 64B data | CRC-32` over USB from dc34-console
  (cmds/image.rs), displayed with `gfx.bitmap_diffusion`.

## 4. Storage & crypto in dc34-vault

* **PDDB idioms**: `pddb.get(dict, key, None, create_dict, create_key,
  alloc_hint, cb)` → `PddbKey: Read+Write`; missing key with
  `create=false` → `ErrorKind::NotFound`; update = `delete_key` then
  recreate; enumerate with `list_keys(dict, None)`; bulk with
  `read_dict(dict, None, Some(limit))`; `pddb.sync()` after writes
  (errors swallowed). Existing dicts: `vault.passwords`, `vault.totp`,
  `vault.config`, `fido.u2fapps`, `opensk`, `dc34`. A new `baogram`
  dict with fixed keys follows the `dc34` pattern.
* **Ed25519**: `ed25519-compact` v1 (already a default-on dep, feature
  `ed25519`): `KeyPair::from_seed(Seed::new(seed))`, `sk.sign(msg, None)`,
  `pk.verify` available. Same crate used by baogram-core → identical
  on-badge and host semantics.
* **RNG**: `rand::thread_rng()` is TRNG-backed on target via the
  `[patch.crates-io.getrandom]` → xous-core getrandom implementation
  (documented at actions.rs:415-450); hosted = OS rng. Use it for the
  identity seed.
* **SHA-256**: `sha2` 0.10.8 already a dep (config.rs `k0_hash`).

## 5. Hosted mode & xtask

* `cargo xtask baosec-emu` builds+runs a hosted baosec image but with
  xous-core's **own** `vault2` app, not dc34-vault; requires
  `UUID` env (64 hex chars). `cargo xtask hosted-bao1x-ci` is the
  build-only variant. PDDB hosted is backed by
  `tools/pddb-images/hosted.bin`.
* Hosted testability for Baogram: PDDB/identity/UI logic runs hosted;
  camera and QR streaming need the synthetic-frame path implemented
  server-side in bao-video (this branch adds it).

## 6. Selected integration points

xous-core (branch `feature/baogram-camera-api`):
1. `libs/ux-api/src/service/api.rs` — append camera/QR-stream opcodes
   after `Quit`; add `CameraFrameInfo`, `CameraFrameChunk`,
   `QrStreamItem` types + constants (7,936-byte chunk like `BulkRead`).
2. `libs/ux-api/src/service/gfx.rs` — client methods incl. a safe
   full-frame read loop.
3. `services/bao-video/src/main.rs` — new opcode arms; camera power
   up/down as local macros duplicating the proven AcquireQr sequence
   (AcquireQr arm itself untouched); mode state
   (`preview_active`, `still_request`, `qr_stream_*`); CamIrq arm gains
   preview/stream/still branches; keypress aborts the QR stream but not
   preview (the vault owns preview lifecycle — a keypress abort would
   race the vault's own CameraStillCapture message).
4. `services/bao-video/src/still.rs` (new) — pure helpers (synthetic
   frame, chunk math) with host-runnable unit tests.

dc34-vault (branch `feature/baogram-app`):
1. `Cargo.toml` — add `baogram-core = { path = "libraries/baogram-core" }`.
2. `src/main.rs` — `VaultMode::Baogram*` variants (end of enum),
   `should_animate` arms, main-loop `VaultOp::Baogram*` handlers,
   baogram module init.
3. `src/vault_api.rs` — `VaultOp` additions after `AbortQr`.
4. `src/ux.rs` — redraw + handle_key arms for Baogram modes; reuse
   `qr_override`/quantum pattern for Share; `user_bitmap`-style preview
   rendering for the feed.
5. `src/config.rs` — `update_power_state` arms.
6. `src/idlemenu.rs` — "Baogram" entry.
7. `src/baogram/` (new) — identity, storage, camera, feed, transfer,
   render, controller.
8. `src/actions.rs`/`action_handler.rs` — no changes needed if Baogram
   drives the camera APIs from its own controller thread (chosen
   approach: keep Baogram self-contained rather than threading through
   ActionManager).

dc34-api / dc34-console: **no changes required** — Baogram introduces no
new cross-crate constants; `SERVER_NAME_VAULT2` and PDDB dict names it
uses are private to dc34-vault or new.
