# Baogram — signed picture-sharing for the DC34 badge

**Baogram** turns the DC34 badge into an offline, serverless photo
network. Shoot 1-bit photos with the badge camera, sign them with the
badge's Ed25519 identity, and beam them badge-to-badge — or to a laptop —
as fountain-coded animated QR streams. Typical shots transfer in a handful
of frames. No servers, no radios, no accounts: two badges pointed at each
other is the whole network.

The badge boots straight into the photo feed. The original vault
application (FIDO2, TOTP, the conference light-gene exchange) is still
there, one "Exit Baogram" away behind the app launcher.

> [!WARNING]
> Loading your own firmware onto your badge **erases the factory light
> encryption key and permanently switches the badge to developer mode**.
> Light-gene exchange with stock badges stops working, and the factory
> state cannot be restored by you or by this project. Read
> [BAOGRAM_HARDWARE_TEST.md](BAOGRAM_HARDWARE_TEST.md) before flashing a
> badge you care about.

## What it does

* **Shoot** — the badge camera captures to 1-bit, 128x120, despeckled.
* **Sign** — every post carries an Ed25519 signature over its digest, so a
  post's author can be verified by whoever receives it.
* **Share** — animated QR, fountain-coded, so a receiver can start
  mid-stream and still recover the picture.
* **Receive** — point one badge's camera at another's screen.
* **Upload** — push a picture from your computer instead of shooting it
  (below).
* **Gallery** — up to 32 posts, stored in the badge's encrypted PDDB.

## Quickstart

Flash a badge (see [Flashing](#flashing)), then:

| you want to | do this |
|---|---|
| take a photo | 🔥 from the feed, 🔥 to capture, 🔥 to save |
| share a post | ∴ on a post → **Share** — the QR stream loops |
| receive a post | ← from the feed, then point at the sending badge |
| upload a picture | `dc34-image --port <port> --image pic.png`, then 🔥 |
| leave Baogram | ∴ → **Exit Baogram** for the vault app |

🔥 is the middle (fire) button; ∴ is the center-press/select key.

## Uploading a picture from your computer

Skip the camera: push a 128x128 black-and-white PNG over USB serial with
[`dc34-image`](https://github.com/bunnie/dc34-image) and the badge stages
it as a post — press 🔥 to sign and save it, and from there it shares over
QR like any camera shot.

```sh
dc34-image --port /dev/cu.usbmodemXXXX --image bao.png
```

Full walk-through, including what the badge does to your picture and what
to do when nothing appears on screen:
**[BAOGRAM_IMAGE_UPLOAD.md](BAOGRAM_IMAGE_UPLOAD.md)**.

## Documentation

* **[BAOGRAM_USAGE.md](BAOGRAM_USAGE.md)** — using the badge, the
  emulator, the flasher, and the laptop peer. Start here.
* **[BAOGRAM_IMAGE_UPLOAD.md](BAOGRAM_IMAGE_UPLOAD.md)** — uploading a
  picture from your computer and turning it into a post.
* [BAOGRAM_PROTOCOL.md](BAOGRAM_PROTOCOL.md) — normative wire formats.
* [BAOGRAM_ARCHITECTURE.md](BAOGRAM_ARCHITECTURE.md) — how the app is put
  together.
* [BAOGRAM_SECURITY.md](BAOGRAM_SECURITY.md) — threat model and what the
  signatures do and do not promise.
* [BAOGRAM_BUILD.md](BAOGRAM_BUILD.md) — building and testing.
* [BAOGRAM_HARDWARE_TEST.md](BAOGRAM_HARDWARE_TEST.md) — read before
  flashing any badge you care about.
* [BAOGRAM_STATUS.md](BAOGRAM_STATUS.md) — what is verified, and how.

## Flashing

**Prebuilt images.** Each
[release](https://github.com/zitterbewegung/dc34-baogram/releases) carries
a developer-signed `loader.uf2` / `swap.uf2` / `xous.uf2` set with
checksums, so you can try Baogram without a toolchain.

To install: hold any button while plugging the badge into USB — it mounts
as a FAT volume named `BAOCHIP` — then copy the files **in this order**:

1. `loader.uf2`
2. `swap.uf2`
3. `xous.uf2`

Then **press any button to boot**. That final step flushes the last
sector; powering off without it can leave part of the image missing. First
boot after an update is slow (PDDB initialization).

`python3 tools/flash_badge.py` does all of this for you, and
`tools/webflash/` does it from a browser.

To go back to stock, use the
[official images](https://ci.betrusted.io/releases/latest/baochip/dc34-badge/latest.zip)
— though the badge stays in developer mode, since the factory key is gone.

## Building

Baogram builds against sibling checkouts, with `xous-core` on the matching
`feature/baogram-camera-api` branch:

```
 .
 ├── dc34-api
 ├── dc34-console
 ├── dc34-baogram
 └── xous-core
```

```sh
scripts/build-baogram.sh          # console + vault + packaged UF2 set
python3 tools/flash_badge.py --build   # build *and* flash
```

Prerequisites — an official rustup toolchain (a distribution-built Rust
fails against the prebuilt Xous std), `cargo xtask install-toolkit` inside
`xous-core`, and a tag-reachable `xous-core` for image signing. The
details, the manual command sequence, and the test suites are in
[BAOGRAM_BUILD.md](BAOGRAM_BUILD.md).

```sh
scripts/test-baogram.sh                # every host-runnable suite
scripts/test-import-emulator.sh        # upload path, in the emulator
```

There is also a full emulator: `cargo xtask baosec-emu` from `xous-core`
runs the badge app on your desktop, no hardware needed. See
[BAOGRAM_USAGE.md](BAOGRAM_USAGE.md).

## The vault application

This repository is a fork of bunnie's DC34
[`vault`](https://github.com/bunnie/dc34-vault) application, and
everything it did still works — Baogram is an app alongside it, reachable
from the launcher.

**Conference mode.** With the core module mated to the badge carrier, the
badge shows the Defcon logo alternating with a picture you upload. Light
patterns "mix" between badges by scanning QR codes: press left or right to
show a nonce, have the color donor scan it with their middle button, scan
the QR they show back, then accept or reject the result. The patterns are
encrypted under a key shared across the whole population — extract it and
you can seed arbitrary patterns. Part of that key is planned to be leaked
so brute-forcing it becomes a contest. Details in
[defcon-scheme.md](./defcon-scheme.md).

**Token mode.** Detached from the carrier (two screws on the back; a
silicone cap is in the kit), the module is a USB FIDO2 2FA token that also
stores TOTPs and passwords scanned from QR codes, generated by
[this browser extension](https://github.com/baochip/qr-url-extension).
There is no battery, so scan a time-bearing QR after plugging in if you
want TOTP.

There are a couple of easter eggs buried in the upstream code, and maybe a
flag to capture if you look hard enough.
