#!/usr/bin/env bash
# End-to-end test of the serial-upload-becomes-a-post path, run against the
# whole app in the baosec emulator (hosted mode).
#
# The app-side driver lives in src/hosted.rs (BAOGRAM_IMPORT_TEST=1): it
# writes a known 128x128 bitmap into the dc34:image PDDB key exactly as the
# `image` console command does, rings VaultOp::ImageLoad, accepts the staged
# post, then reads the post back and compares its pixels to the upload.
#
# Note: the emulator opens a minifb window, so this needs a graphical
# session (it will not run over a plain SSH connection).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
vault_dir="$(cd "$here/.." && pwd)"
ws="$(cd "$vault_dir/.." && pwd)"

fail() { echo "error: $*" >&2; exit 1; }

[ -d "$ws/xous-core" ] || fail "expected sibling checkout $ws/xous-core"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
host_triple="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')" || fail "rustc not found"
rustup_bin="$HOME/.rustup/toolchains/stable-$host_triple/bin"
[ -d "$rustup_bin" ] && export PATH="$rustup_bin:$PATH"

timeout_s="${BAOGRAM_IMPORT_TEST_TIMEOUT:-120}"
pddb_img="$ws/xous-core/tools/pddb-images/hosted.bin"
log="$(mktemp -t baogram-import-test)"

echo "===== building the hosted app ====="
(cd "$vault_dir" && cargo build --release --features hosted-baosec)

# Run against a factory-fresh PDDB so the post count starts from a known
# state; the developer's emulator state is put back afterwards.
saved=""
if [ -f "$pddb_img" ]; then
    saved="$pddb_img.import-test-backup"
    mv "$pddb_img" "$saved"
fi
restore() {
    if [ -n "$saved" ] && [ -f "$saved" ]; then
        mv -f "$saved" "$pddb_img"
    fi
}
trap restore EXIT

echo "===== running the emulator (BAOGRAM_IMPORT_TEST=1) ====="
# A second emulator sharing hosted.bin corrupts the run (both fight over the
# PDDB and the keyboard), so refuse to start on top of one.
if pgrep -f "baosec-emu" >/dev/null 2>&1 || pgrep -f "xous-kernel .*dc34-vault" >/dev/null 2>&1; then
    fail "an emulator is already running - stop it first (pgrep -fl baosec-emu)"
fi
set +e
# job control, so the emulator gets its own process group: `cargo xtask` execs
# a kernel child that survives a kill aimed at the wrapper alone.
set -m
(
    cd "$ws/xous-core" && \
    BAOGRAM_IMPORT_TEST=1 BAOGRAM_IMPORT_TEST_EXIT=1 \
    cargo xtask baosec-emu ../dc34-baogram/target/release/dc34-vault
) >"$log" 2>&1 &
emu_pid=$!
set +m
# poll for a verdict rather than waiting on the emulator, which may keep
# other threads alive after the test thread calls exit()
verdict=""
for _ in $(seq "$timeout_s"); do
    if grep -q "BAOGRAM IMPORT TEST: PASS" "$log"; then verdict=pass; break; fi
    if grep -q "BAOGRAM IMPORT TEST: FAIL" "$log"; then verdict=fail; break; fi
    if ! kill -0 "$emu_pid" 2>/dev/null; then break; fi
    sleep 1
done
# kill the whole process group, then confirm nothing survived
kill -TERM -"$emu_pid" 2>/dev/null
wait "$emu_pid" 2>/dev/null
for _ in $(seq 10); do
    pgrep -f "xous-kernel .*dc34-vault" >/dev/null 2>&1 || break
    sleep 1
done
if pgrep -f "xous-kernel .*dc34-vault" >/dev/null 2>&1; then
    kill -KILL -"$emu_pid" 2>/dev/null
    echo "warning: had to SIGKILL the emulator process group" >&2
fi
set -e

echo
grep "BAOGRAM IMPORT TEST" "$log" || true
echo

case "$verdict" in
    pass) echo "Import-to-post emulator test PASSED"; rm -f "$log" ;;
    fail) echo "Import-to-post emulator test FAILED (full log: $log)"; exit 1 ;;
    *)    echo "Import-to-post emulator test produced no verdict within ${timeout_s}s"
          echo "(full log: $log) - if this is a headless session, the emulator"
          echo "window cannot open; run it from a graphical session."
          exit 1 ;;
esac
