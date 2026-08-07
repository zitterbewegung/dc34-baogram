# Baogram architecture

## Component map

```text
┌──────────────────────────── badge (Xous) ────────────────────────────┐
│                                                                      │
│  bao-video (xous-core, PID "Gfx" server)                             │
│  ├─ GC2145 camera driver, 256x240 Y-channel frame buffer (61,440 B)  │
│  ├─ AcquireQr        (pre-existing one-shot QR, unchanged)           │
│  ├─ CameraPreview*/CameraStill* (new generic still-capture API)      │
│  └─ QrStream*        (new generic continuous QR decode API)          │
│           ▲  rkyv IPC (GfxOpcode, appended after Quit)               │
│           │                                                          │
│  dc34-vault ("_Vault2_")                                             │
│  ├─ src/baogram/     identity · storage · camera · feed ·           │
│  │                   transfer · render · controller                  │
│  ├─ VaultMode::Baogram{Feed,Camera,Preview,PostMenu,Share,Receive,   │
│  │                     Profile} + baogrammenu (MenuMatic)            │
│  └─ libraries/baogram-core  ◄── canonical formats (no Xous deps)     │
│           │                                                          │
│  pddb: dict "baogram" → identity · index · post.<hex-id>             │
└──────────────────────────────────────────────────────────────────────┘
            ▲ animated QR loop (Base45 fragments, EC level M)
            ▼
┌─────────────────────────── laptop peer ──────────────────────────────┐
│  tools/baogram-host (Python 3.11+)                                   │
│  format/codec/crypto/fragments = byte-exact mirror of baogram-core   │
│  cli: make-post · inspect · verify · export · send · receive         │
└──────────────────────────────────────────────────────────────────────┘
```

## Layering decisions

* **xous-core changes are generic.** bao-video gained an
  application-facing still-camera API and a continuous QR stream; it
  knows nothing about Baogram (no fragment parsing, no post formats —
  QrStreamSetStatus takes an arbitrary short string). Suitable for
  upstream contribution.
* **baogram-core is pure.** Formats, codec, crypto, reassembly: no Xous,
  camera, display, or PDDB dependencies; tests run host-native and pin
  golden vectors shared with the Python peer.
* **dc34-vault owns all Baogram semantics**: rendering (2x downsample to
  the 128x128 OLED), storage conventions, UI flow.

## Camera data path (capture)

1. `CameraPreviewStart` powers the camera with the retry/verify sequence
   proven by AcquireQr; bao-video blits each frame to the OLED
   (2:1 decimation, adaptive threshold).
2. `CameraStillCapture` parks the request (deferred reply); the next
   complete frame freezes **in place** in bao-video's single 61,440-byte
   frame buffer (no second copy), the camera powers down, preemption is
   restored, and the reply carries `{256, 240, GRAY8, len, generation}`.
3. The vault reads the frame in eight 7,936-byte chunks
   (`CameraStillReadChunk`; stale generations and invalid offsets are
   refused), quantizes to Mono1 (7,680 bytes), and drops the grayscale
   buffer.
4. Save: sequence++, `Post::create` (sign), serialize, PDDB write, sync.

Memory at peak during capture: one 61,440 B grayscale + one 7,680 B
Mono1; during share: one serialized post + one fragment + one QrCode;
during receive: one bounded (<= 65,536 B) reassembly buffer + fixed
512 B bitmap.

## Transfer path

* **Share** runs on the vault's 250 ms animation pump: every
  `period_ms/250` ticks the next fragment is materialized (FragmentIter
  → Base45 → QrCode) and drawn via the existing `render_qr` path; the
  loop wraps until canceled. Payload size and period are runtime
  diagnostics settings (jog dial).
* **Receive** runs in a worker thread: `QrStreamStart` → blocking
  `QrStreamRead` loop → Base45/fragment parse → reassembler feed →
  progress via `QrStreamSetStatus` ("Baogram k/n") → on completion
  `QrStreamStop`, then full parse+verify; only a verified post reaches
  the Save/Reject preview. Any keypress aborts the stream inside
  bao-video; the worker sees `None` and reports the cancel. Conflicting
  fragments abort the transfer with an error notification.

## Concurrency model

* The vault main loop is single-threaded and may block briefly on
  capture (~1 frame). Camera modes are mutually exclusive inside
  bao-video (`AcquireQr` xor preview/still xor stream); a second
  claimant gets a busy error.
* The receive worker communicates via `Arc<Mutex<BaogramShared>>` and
  wakes the main loop with `VaultOp::BaogramRxDone`. Locks are never
  held across blocking IPC in the main loop; the worker locks only for
  short state updates.
* Keys reach the vault through bao-video's filtered listener exactly as
  before; during camera preview bao-video does **not** consume keys
  (the vault decides capture/cancel), while QR modes keep their
  key-abort semantics.

## What did NOT change

`dc34-api` and `dc34-console` are untouched. Existing vault features
(FIDO2, TOTP, passwords, gene exchange, tours, menus) and the existing
`AcquireQr` server path are behaviorally unchanged; existing enum
discriminants (VaultMode archives, VaultOp incl. the hard-coded
1024-1027 block, GfxOpcode) are all preserved by append-only extension.
