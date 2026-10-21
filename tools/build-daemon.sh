#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Build the Puhuri 2 background service for the phone (aarch64).
#
#   tools/build-daemon.sh
#   PUHURI_FEATURES=omemo tools/build-daemon.sh   # with the OMEMO ratchet
#
# The service is Rust. It is cross compiled here; the resulting binaries go to
# build/ and are installed on the phone by install.sh.
#
# PUHURI_FEATURES adds cargo features (currently only `omemo`, which links the
# static libsignal-protocol-c that tools/build-libsignal-c.sh makes).
#
# Note: a cargo build touches ~/.cargo outside the Codex sandbox, so run this
# with the app's approval when the sandbox complains.
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
TARGET=${PUHURI_TARGET:-aarch64-unknown-linux-gnu}
FEATURES=${PUHURI_FEATURES:-}

. "$HOME/.cargo/env" 2>/dev/null || true

cd "$PROJECT/daemon"
echo "--- kohde: $TARGET"

# Panic locations and dependency debug information must not reveal the
# builder's home directory in a distributed binary.
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$HOME=/build/home"
export CFLAGS_aarch64_unknown_linux_gnu="${CFLAGS_aarch64_unknown_linux_gnu:+$CFLAGS_aarch64_unknown_linux_gnu }-ffile-prefix-map=$HOME=/build/home -fdebug-prefix-map=$HOME=/build/home"
export CXXFLAGS_aarch64_unknown_linux_gnu="${CXXFLAGS_aarch64_unknown_linux_gnu:+$CXXFLAGS_aarch64_unknown_linux_gnu }-ffile-prefix-map=$HOME=/build/home -fdebug-prefix-map=$HOME=/build/home"

# Zig is the linker driver: it brings its own aarch64 toolchain and glibc
# (2.39 matches Ubuntu Touch 24.04), so no apt and no root are needed. The
# XMPP stack is pure Rust apart from its TLS backend (aws-lc-rs) and the
# bundled SQLite, and both are C: the same zig driver is handed to cc-rs.
export ZIG_TARGET=${ZIG_TARGET:-aarch64-linux-gnu.2.39}
if [ -z "${PUHURI_CROSS_LINKER:-}" ]; then
    PUHURI_CROSS_LINKER="$PROJECT/tools/zig-linker.sh"
fi
case "$TARGET" in
    aarch64-unknown-linux-gnu)
        export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$PUHURI_CROSS_LINKER"
        export CC_aarch64_unknown_linux_gnu="$PUHURI_CROSS_LINKER"
        export AR_aarch64_unknown_linux_gnu="$PROJECT/tools/zig-ar.sh"
        export RANLIB_aarch64_unknown_linux_gnu="$PROJECT/tools/zig-ranlib.sh"
        ;;
    aarch64-unknown-linux-musl)
        export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$PUHURI_CROSS_LINKER"
        ;;
esac

if [ -n "$FEATURES" ]; then
    echo "--- ominaisuudet: $FEATURES"
    cargo build --release --target "$TARGET" --features "$FEATURES"
else
    cargo build --release --target "$TARGET"
fi

mkdir -p "$PROJECT/build"
cp "target/$TARGET/release/puhurid" "$PROJECT/build/puhurid"
cp "target/$TARGET/release/puhuri-ctl" "$PROJECT/build/puhuri-ctl"
ls -l "$PROJECT/build/"
