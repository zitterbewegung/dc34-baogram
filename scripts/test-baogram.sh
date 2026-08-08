#!/usr/bin/env bash
# Run every host-runnable Baogram test suite:
#   1. baogram-core (canonical formats + golden vectors)
#   2. bao-video still-camera helpers (chunk math + synthetic-frame SHA)
#   3. Python peer + Rust/Python interop (pytest, regenerates the
#      python vectors)
#   4. baogram-core again, on those vectors, closing the Python->Rust loop
#   5. vault app unit tests (upload conversion + which screens accept one)
#
# The emulator-driven import test needs a window; run it separately with
# scripts/test-import-emulator.sh.
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

echo "===== [1/5] baogram-core ====="
(cd "$vault_dir/libraries/baogram-core" && cargo test)

echo "===== [2/5] bao-video still-camera helpers (hosted) ====="
(cd "$ws/xous-core" && cargo test -p bao-video --features hosted-baosec,modals/hosted-baosec)

echo "===== [3/5] baogram-host (Python peer + interop) ====="
host_dir="$vault_dir/tools/baogram-host"
if [ ! -x "$host_dir/.venv/bin/pytest" ]; then
    echo "creating venv and installing baogram-host..."
    python3 -m venv "$host_dir/.venv"
    "$host_dir/.venv/bin/pip" -q install -e "$host_dir[test]"
fi
(cd "$host_dir" && .venv/bin/pytest -q)

echo "===== [4/5] baogram-core again (consumes Python-generated vectors) ====="
(cd "$vault_dir/libraries/baogram-core" && cargo test golden)

echo "===== [5/5] vault app unit tests (upload -> post conversion) ====="
(cd "$vault_dir" && cargo test --features hosted-baosec --bin dc34-vault)

echo
echo "All Baogram host-side test suites passed."
echo
echo "Not included (needs a graphical session for the emulator window):"
echo "  scripts/test-import-emulator.sh - drives a serial upload through the"
echo "  live app and checks it lands in the gallery as a signed post."
