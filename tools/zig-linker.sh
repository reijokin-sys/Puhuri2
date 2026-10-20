#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Linker driver for the Sähke cross build.
#
# Zig ships a complete aarch64 toolchain (compiler, lld, libc for musl and
# glibc targets) as a Python package, so the phone builds can be made on this
# computer without apt and without root. Cargo is pointed here:
#
#   CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=tools/zig-linker.sh \
#       cargo build --release --target aarch64-unknown-linux-musl
#
# ZIG_TARGET overrides the triple. `aarch64-linux-musl` gives a static binary
# that runs on the phone as it is; `aarch64-linux-gnu.2.39` would match the
# glibc of Ubuntu Touch 24.04.
#
# Rust adds `-Wl,--fix-cortex-a53-843419` for aarch64 and zig's cc refuses it
# ("unsupported linker arg"), so it is dropped here. That is safe: lld, which
# zig uses underneath, applies the Cortex-A53 erratum workaround by default.
set -eu

ZIG=$(python3 -c "import ziglang, os; print(os.path.join(os.path.dirname(ziglang.__file__), 'zig'))")

arguments=()
skip_next=0
for argument in "$@"; do
    if [ "$skip_next" = "1" ]; then
        skip_next=0
        continue
    fi
    case "$argument" in
        # cc-rs passes the Rust target triple; zig wants its own spelling,
        # which this script already puts on the command line.
        --target) skip_next=1; continue ;;
        --target=*) continue ;;
        *--fix-cortex-a53-843419*) continue ;;
    esac
    arguments+=("$argument")
done

exec "$ZIG" cc -target "${ZIG_TARGET:-aarch64-linux-musl}" "${arguments[@]}"
