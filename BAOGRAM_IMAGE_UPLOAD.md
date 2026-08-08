# Uploading a picture from your computer

The badge camera is not the only way to make a post. Any 128x128
black-and-white PNG can be pushed over USB serial with
[`dc34-image`](https://github.com/bunnie/dc34-image) — bunnie's badge
image uploader — and Baogram turns it into a signed post, identical in
every way to one you shot with the camera: same format, same signature,
shareable over QR to any other badge.

Verified end to end on hardware, 2026-08-08.

## 1. Install the uploader

```sh
pipx install git+https://github.com/bunnie/dc34-image.git
```

## 2. Find the badge's serial port

Plug the badge in **normally** (no button held — that is update mode,
which is a different thing).

```sh
ls /dev/cu.usbmodem*          # macOS
ls /dev/ttyACM*               # Linux
```

The port name changes every time the badge re-enumerates, so check it
each session rather than hard-coding it.

## 3. Get to a screen that can accept the upload

An upload only becomes a post when the badge is inside Baogram and idle:
the **feed**, the **profile** screen, or the **launcher**.

Everywhere else the upload still lands — it sets your badge's avatar
bitmap, the picture that alternates with the DC logo on the conference
screen, exactly as it always has — but the badge stays where it is. That
is deliberate:

* **On the idle / conference screens** the upload *is* the avatar, and
  someone using that workflow did not ask to be dropped into a post
  preview.
* **Mid-camera, mid-share, mid-receive** an upload must never yank the
  display out from under work in progress.
* **With a post already awaiting your decision** it will not clobber it.

So if you want the picture as a post, get to the Baogram feed first.

## 4. Send the picture

```sh
dc34-image --port /dev/cu.usbmodemXXXX --image bao.png
```

Add `--force` to let the uploader resize and Floyd-Steinberg dither any
image down to 128x128 black-and-white first. It saves the converted file
alongside the original, so you can see what the badge is actually getting.

You should see 32 chunks acknowledged, then `SUCCESS`:

```
[INFO] Sending 32 chunks via /dev/cu.usbmodemR9DNH53 @ 1000000 baud
[OK]  Chunk  1/32
...
[OK]  Chunk 32/32 -> SUCCESS — transfer complete
```

## 5. Save it on the badge

The badge jumps straight to the post preview showing your picture, with
the label `uploaded  🔥 save   ← discard`.

| key | what it does |
|---|---|
| **🔥** (middle / fire button) | sign the post and save it to the gallery |
| **←** (left) | discard the upload |

**Press 🔥.** You land back in the feed with your picture as the newest
post, ready to share over QR like any other.

If the save fails — the gallery is capped at 32 posts — the badge shows
you why and *keeps* the pending post, so nothing is lost; delete a post
and press 🔥 again.

## What the badge does to your picture

| | |
|---|---|
| upload | 128x128, 1 bit |
| post | 128x120, 1 bit (`PIXEL_FORMAT_MONO1_SMALL`) — the same size every other Baogram post uses |
| columns | 1:1, every one survives |
| rows | nearest-neighbour resample, 128 -> 120: the whole picture stays visible and 8 of the 128 rows are dropped |

So expect a slight vertical squeeze. Straight horizontal lines near the
top and bottom are where you will notice it first.

The picture also becomes your badge's avatar bitmap, which is what the
upload did before Baogram existed — that behaviour is unchanged.

## Removing it

```sh
dc34-image --port /dev/cu.usbmodemXXXX --clear
```

This clears the avatar bitmap. It does **not** delete posts you already
saved — remove those from the post menu on the badge.

## If nothing appears on screen

* **The badge was not in Baogram.** From the idle/conference screen the
  upload only sets your avatar. Open Baogram, then send again.
* **The badge was busy.** Back out to the feed and send again.
* **A post was already waiting for review.** Press 🔥 or ← to deal with
  it, then send again.
* **Chunks time out or `ERR`.** Something else is holding the serial port
  (another `dc34-image`, a terminal, a monitor). Close it and retry;
  `--delay 0.4` slows the chunk rate if the link is marginal.

## How it works

```
your.png
  -> dc34-image: flip, 1-bit, pack to 512 u32s, 32 base64 chunks (CRC32 each)
  -> serial "image <base64>" lines at 1 Mbaud
  -> dc34-console `image` command: verify CRC, reassemble 2,048 bytes
  -> PDDB key dc34:image  +  VaultOp::ImageLoad
  -> Baogram: convert to Mono1Small, stage as a pending post
  -> 🔥: sign with the badge identity, save to the gallery
```

The conversion and the staging rules are tested two ways: unit tests in
`src/baogram/render.rs` (`cargo test --features hosted-baosec --bin
dc34-vault`) and an emulator test that drives the whole app,
`scripts/test-import-emulator.sh`. See BAOGRAM_BUILD.md.
