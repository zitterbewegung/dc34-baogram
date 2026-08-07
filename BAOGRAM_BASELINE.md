# Baogram baseline build record (Gate A)

Date: 2026-08-06. Host: macOS (Darwin 25.5.0, Apple Silicon).

Goal: reproduce the untouched DC34 badge build (console + vault +
baosec-lite image packaging) before any Baogram source changes.

## Repository state at baseline

Workspace layout (sibling checkouts, as required by the `[patch]` sections
in every Cargo.toml):

```
baostagram/
├── dc34-api/       main    @ 617f0f3dff3cea1e9421d766b19664f5bec9a54b (clean)
├── dc34-console/   main    @ bf64e03f019532cca5055fcdbe51977d572e3630 (clean)
├── dc34-vault/     feature/baogram-app branched from
│                   main    @ 3d5cbf707a715fca508074e4a377f0d7497e0cba (clean)
└── xous-core/      feature/baogram-camera-api at
                    dev     @ 5d5bbbfa95c0dcef26fe1fe9b496b7f6f31d191b (clean)
```

### Which xous-core revision, and why

`dc34-vault/Cargo.toml`, `dc34-api/Cargo.toml`, and `dc34-console/Cargo.toml`
all pin git dependencies on xous-core at rev
`616bf65f6e379165464f50b1e79ec42aff77a683` (2026-03-22). That pin is
**vestigial for most crates**: the `[patch."https://github.com/betrusted-io/xous-core"]`
sections redirect every patched crate (`ux-api`, `bao1x-hal`, `pddb`,
`keystore`, `modals`, `blitstr2`, …) to the **sibling `../xous-core`
checkout**, and only a handful of crates (`persistent_store`, `locales`,
`userprefs`, `getrandom`, and for the vault `usb-bao1x`/`bao1x-emu`) come
from the git pin.

Building with the sibling checkout at 616bf65 **fails**:

```
error: failed to select a version for `keystore`.
package `dc34-console` depends on `keystore` with feature `owc-inc`
but `keystore` does not have that feature.
```

`services/keystore` gained `owc-inc` after March; it is present at dev
(`5d5bbbf`, 2026-08-03), the same revision the author's sibling checkout
used. The checked-in `Cargo.lock` files also resolve cleanly against dev.
The sibling checkout therefore uses the *deliberate compatible update*
`5d5bbbfa95c0dcef26fe1fe9b496b7f6f31d191b` (tip of `dev` on 2026-08-03);
the unpatched git-pinned crates continue to come from `616bf65` exactly as
the lockfiles specify.

## Toolchain

* rustc/cargo **1.97.1 (8bab26f4f 2026-07-14)** — the **official rustup
  build** at `~/.rustup/toolchains/stable-aarch64-apple-darwin`.
* Xous target std installed by `cargo xtask install-toolkit` into that
  toolchain's sysroot (`lib/rustlib/riscv32imac-unknown-xous-elf`).

### Environmental blockers hit and their remediations

1. **`cargo xtask install-toolkit` panics with `NotPresent`**
   (`xtask/src/verifier.rs:40` does `env::var("CARGO_HOME").unwrap()`).
   Remediation: run with `CARGO_HOME="$HOME/.cargo"` exported.

2. **Homebrew rustc is incompatible with the prebuilt toolkit.**
   With Homebrew's self-compiled rust (`/opt/homebrew/bin/cargo`, version
   string also "1.97.1 (8bab26f4f)"), every target build fails with
   `error[E0514]: found crate 'core' compiled by an incompatible version of
   rustc` (cascading into thousands of prelude errors inside `rend`): the
   prebuilt xous std is built by the *official* 1.97.1, whose crate
   metadata does not match Homebrew's self-built compiler.
   Remediation: prefix `PATH` with
   `$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin` for every
   build and for `install-toolkit` (the scripts in `scripts/` do this).

3. **Image signing fails with `Can't sign swap image`.**
   `xous-create-image` embeds `git describe` output
   (`tools/src/sign_image.rs::semver_for_sign_embed`); the xous-core clone
   was **shallow with no tags**, so `git describe` failed. Remediation:
   `git fetch --depth 1 origin tag v0.10.2-beta1` followed by
   `git fetch --shallow-since=2026-07-20 origin dev`, after which
   `git describe` → `v0.10.2-beta1-76-g5d5bbbfa9`.

No source file was modified to make the baseline build.

## Exact commands and results

All commands run with:

```sh
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
export CARGO_HOME="$HOME/.cargo"
```

1. Toolkit (from `xous-core/`):

   ```sh
   cargo xtask install-toolkit        # exit 0
   ```

2. Console (from `dc34-console/`):

   ```sh
   cargo build --release --target riscv32imac-unknown-xous-elf \
     --features board-baosec --features oem-baosec-lite \
     --features bao1x --features utralib/bao1x
   # exit 0 → target/riscv32imac-unknown-xous-elf/release/dc34-console (631,032 bytes)
   ```

3. Vault (from `dc34-vault/`):

   ```sh
   cargo build --release --target riscv32imac-unknown-xous-elf \
     --features board-baosec
   # exit 0 → target/riscv32imac-unknown-xous-elf/release/dc34-vault (1,753,680 bytes)
   ```

4. Image packaging (from `xous-core/`, per the dc34-vault README):

   ```sh
   cargo xtask baosec-lite \
     ../dc34-console/target/riscv32imac-unknown-xous-elf/release/dc34-console~flash \
     ../dc34-vault/target/riscv32imac-unknown-xous-elf/release/dc34-vault \
     --no-timestamp --feature usb --kernel-feature debug-proc --no-verify
   # exit 0
   ```

   Products in `xous-core/target/riscv32imac-unknown-xous-elf/release/`:

   | file       | size (bytes) |
   |------------|--------------|
   | loader.uf2 | 353,280      |
   | swap.uf2   | 2,343,424    |
   | xous.uf2   | 6,366,720    |

   Image process table: kernel, xous-swapper, keystore, xous-ticktimer,
   xous-log, xous-names, usb-bao1x, bao1x-hal-service, modals, pddb,
   bao-video, dc34-console, dc34-vault.

Gate A: **PASS**.
