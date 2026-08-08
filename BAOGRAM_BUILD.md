# Building Baogram

## Repository layout and revisions

Sibling checkouts (required by the `[patch]` sections in every
Cargo.toml):

```text
workspace/
├── dc34-api/       main @ 617f0f3   (unchanged)
├── dc34-console/   main @ bf64e03   (unchanged)
├── dc34-baogram/     branch feature/baogram-app        (base 3d5cbf7)
└── xous-core/      branch feature/baogram-camera-api (base 5d5bbbf, dev of 2026-08-03)
```

Why xous-core sits at `5d5bbbf` rather than the `616bf65` rev pinned in
the Cargo.tomls: the pin only feeds the handful of crates that are NOT
path-patched to the sibling checkout (`persistent_store`, `locales`,
`userprefs`, `getrandom`, …); everything that matters is path-patched,
and the vault at 3d5cbf7 requires post-March APIs (e.g.
`keystore/owc-inc`, `Gfx::bitmap`). See BAOGRAM_BASELINE.md for the
failed-build evidence.

## Toolchain

* Rust **1.97.1, the official rustup build** — a distribution-built
  compiler (Homebrew etc.) will fail with `E0514` against the prebuilt
  Xous std. If `rustup toolchain list` shows `stable-aarch64-apple-darwin`
  (or your host triple), prefix your PATH with its `bin/`.
* `CARGO_HOME` must be set (`export CARGO_HOME="$HOME/.cargo"`), or
  `cargo xtask install-toolkit` panics in its consistency checker.
* xous-core must be able to `git describe` (image signing embeds it). On
  a shallow/tagless clone:
  `git fetch --depth 1 origin tag v0.10.2-beta1 &&
   git fetch --shallow-since=2026-07-20 origin dev`.
* Python 3.11+ for the laptop peer.

One-time setup, from `xous-core/`:

```sh
export PATH="$HOME/.rustup/toolchains/<your-host-triple>/bin:$PATH"
export CARGO_HOME="$HOME/.cargo"
cargo xtask install-toolkit
```

## Build everything

`scripts/build-baogram.sh` in dc34-baogram performs the sequence below and
verifies the directory layout first.

```sh
# console (unchanged sources)
cd dc34-console
cargo build --release --target riscv32imac-unknown-xous-elf \
  --features board-baosec --features oem-baosec-lite \
  --features bao1x --features utralib/bao1x

# vault + Baogram
cd ../dc34-baogram
cargo build --release --target riscv32imac-unknown-xous-elf \
  --features board-baosec

# package the baosec-lite image (from xous-core)
cd ../xous-core
cargo xtask baosec-lite \
  ../dc34-console/target/riscv32imac-unknown-xous-elf/release/dc34-console~flash \
  ../dc34-baogram/target/riscv32imac-unknown-xous-elf/release/dc34-vault \
  --no-timestamp --feature usb --kernel-feature debug-proc --no-verify
```

Products: `xous-core/target/riscv32imac-unknown-xous-elf/release/`
`{loader.uf2, swap.uf2, xous.uf2}`.

## Tests

`scripts/test-baogram.sh` runs all of:

```sh
# canonical formats (host-native, includes golden vectors)
cd dc34-baogram/libraries/baogram-core && cargo test

# camera chunk math + hosted synthetic frame golden SHA
cd xous-core && cargo test -p bao-video --features hosted-baosec,modals/hosted-baosec

# Python peer + Rust/Python interop (creates .venv on first run)
cd dc34-baogram/tools/baogram-host
python3 -m venv .venv && .venv/bin/pip install -e '.[test]'
.venv/bin/pytest

# vault app units: serial-upload bitmap -> Baogram post conversion
cd dc34-baogram && cargo test --features hosted-baosec --bin dc34-vault
```

The pytest run regenerates `test-vectors/python/`; re-run the
baogram-core tests afterwards to close the Python→Rust loop.

### Emulator test: a serial upload becomes a post

`scripts/test-import-emulator.sh` runs the whole app in the baosec
emulator and exercises the `dc34-image` path end to end: it writes a known
128x128 bitmap into the `dc34:image` PDDB key exactly as the `image`
console command does, rings `VaultOp::ImageLoad`, accepts the staged post
with 🔥, then reads the post back out of the gallery and compares its
pixels to the upload. It checks:

* the upload adds exactly one post (which only happens if the app switched
  to the preview screen — 🔥 in the feed would open the camera instead)
* the post is signed and `Post::parse` verifies it
* every stored pixel matches the uploaded bitmap
* the image is neither all-dark nor all-light
* a repeat upload while one is pending does not double-stage or clobber
* `image clear` does not create a post

The emulator opens a minifb window, so this needs a graphical session; it
is deliberately not part of `test-baogram.sh`. The script refuses to start
on top of a running emulator (two emulators corrupt each other's
`hosted.bin`) and tears down the whole emulator process group afterwards.

## Flashing

Deliberately **not** automated. See BAOGRAM_HARDWARE_TEST.md — installing
a developer-signed image wipes the factory key and is irreversible.
