#!/usr/bin/env bash
# Build the Baogram-enabled DC34 badge image: console, vault, and the
# packaged baosec-lite UF2 set. See BAOGRAM_BUILD.md for prerequisites.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
vault_dir="$(cd "$here/.." && pwd)"
ws="$(cd "$vault_dir/.." && pwd)"

fail() { echo "error: $*" >&2; exit 1; }

# --- layout checks -----------------------------------------------------
for repo in dc34-api dc34-console dc34-vault xous-core; do
    [ -d "$ws/$repo" ] || fail "expected sibling checkout $ws/$repo (see BAOGRAM_BUILD.md)"
done
[ -f "$ws/xous-core/xtask/src/main.rs" ] || fail "$ws/xous-core does not look like xous-core"

# --- toolchain checks --------------------------------------------------
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
if [ -z "${BAOGRAM_TOOLCHAIN_OK:-}" ]; then
    host_triple="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')" || fail "rustc not found"
    rustup_bin="$HOME/.rustup/toolchains/stable-$host_triple/bin"
    if [ -d "$rustup_bin" ]; then
        export PATH="$rustup_bin:$PATH"
    fi
    if rustc -vV | grep -qi 'Homebrew'; then
        fail "rustc is a Homebrew build; the prebuilt Xous std needs the official rustup toolchain (see BAOGRAM_BUILD.md). Set BAOGRAM_TOOLCHAIN_OK=1 to override."
    fi
fi
target_dir="$(rustc --print sysroot)/lib/rustlib/riscv32imac-unknown-xous-elf"
if [ ! -d "$target_dir" ]; then
    echo "Xous toolkit not installed for $(rustc --version); running cargo xtask install-toolkit..."
    (cd "$ws/xous-core" && cargo xtask install-toolkit)
fi
if ! (cd "$ws/xous-core" && git describe >/dev/null 2>&1); then
    fail "git describe fails in xous-core (image signing needs a reachable tag): run
  git -C '$ws/xous-core' fetch --depth 1 origin tag v0.10.2-beta1
  git -C '$ws/xous-core' fetch --shallow-since=2026-07-20 origin dev"
fi

# --- builds ------------------------------------------------------------
echo "===== Building Console ====="
(cd "$ws/dc34-console" && cargo build \
    --release --target riscv32imac-unknown-xous-elf \
    --features board-baosec --features oem-baosec-lite \
    --features bao1x --features utralib/bao1x)

echo "===== Building Vault (with Baogram) ====="
(cd "$ws/dc34-vault" && cargo build \
    --release --target riscv32imac-unknown-xous-elf \
    --features board-baosec)

echo "===== Packaging baosec-lite image ====="
(cd "$ws/xous-core" && cargo xtask baosec-lite \
    ../dc34-console/target/riscv32imac-unknown-xous-elf/release/dc34-console~flash \
    ../dc34-vault/target/riscv32imac-unknown-xous-elf/release/dc34-vault \
    --no-timestamp --feature usb --kernel-feature debug-proc --no-verify)

out="$ws/xous-core/target/riscv32imac-unknown-xous-elf/release"
for f in loader.uf2 swap.uf2 xous.uf2; do
    [ -f "$out/$f" ] || fail "expected image $out/$f was not produced"
done
echo
echo "Images ready in $out:"
ls -la "$out"/loader.uf2 "$out"/swap.uf2 "$out"/xous.uf2
echo
echo "NOTE: flashing is manual and irreversible on a stock badge —"
echo "read BAOGRAM_HARDWARE_TEST.md before installing."
