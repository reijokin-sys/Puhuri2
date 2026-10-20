#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Cross compile the OMEMO ratchet for the phone: libsignal-protocol-c and the
# protobuf-c runtime it needs, both as aarch64 static libraries.
#
#   tools/build-libsignal-c.sh
#
# Why this library and not something else: it is the one the deployed OMEMO
# 0.3 clients themselves use (Conversations, Gajim/QXmpp through libomemo,
# Dino, Monal), so the message format matches by construction. See
# docs/OMEMO.md and docs/DECISIONS.md.
#
# **Pinned**: libsignal-protocol-c **v2.3.3**, commit
# 3a83a4f4ed2302ff6e68ab569c88793b50c22d28 (2020-03-27) - the last release,
# and the tag every OMEMO 0.3 client is built from. The clone names the tag so
# a later master cannot slip in unnoticed; protobuf-c is pinned by version
# below. The built libraries and headers live in the repository; the upstream
# source is fetched here (see docs/OMEMO.md, "what a reviewer should look at").
#
# The result goes to build/libsignal-c/prefix (lib/ + include/) and is linked
# into the daemon by daemon/build.rs when the `omemo` feature is on.
#
# Notes that save a day next time:
#   * protobuf-c's autotools build wants the C++ protobuf headers only for its
#     protoc-c generator, which we do not need on the phone - the runtime is a
#     single C file, so it is compiled straight with zig.
#   * libsignal-protocol-c's .pb-c.c files are checked in, so neither protoc
#     nor the protobuf-c submodule is needed.
#   * zig cc is both the compiler and the linker driver; AR/RANLIB are the
#     workspace's zig wrappers.
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
WORK=${PUHURI_C_WORK:-$PROJECT/build/libsignal-c}
PREFIX=$WORK/prefix
ZIG_TARGET=${ZIG_TARGET:-aarch64-linux-gnu.2.39}
ZIG=$(python3 -c "import ziglang, os; print(os.path.join(os.path.dirname(ziglang.__file__), 'zig'))")
PROTOBUF_C_VERSION=1.5.1
LIB_SIGNAL_VERSION=v2.3.3
# The tag can be moved; the commit is what was reviewed and built.
LIB_SIGNAL_COMMIT=3a83a4f4ed2302ff6e68ab569c88793b50c22d28

mkdir -p "$WORK" "$PREFIX/include" "$PREFIX/lib" "$WORK/obj"
cd "$WORK"

echo "--- lahteet"
PROTOBUF_C_DIR=""
for candidate in "protobuf-c-$PROTOBUF_C_VERSION" protobuf-c; do
    if [ -f "$candidate/protobuf-c/protobuf-c.c" ]; then
        PROTOBUF_C_DIR=$candidate
        break
    fi
done
if [ -z "$PROTOBUF_C_DIR" ]; then
    rm -rf "protobuf-c-$PROTOBUF_C_VERSION" protobuf-c.tar.gz
    curl -sSL -o protobuf-c.tar.gz \
        "https://github.com/protobuf-c/protobuf-c/releases/download/v$PROTOBUF_C_VERSION/protobuf-c-$PROTOBUF_C_VERSION.tar.gz"
    tar xzf protobuf-c.tar.gz
    PROTOBUF_C_DIR="protobuf-c-$PROTOBUF_C_VERSION"
fi
# The source is **vendored** in the repository (`vendor/libsignal-protocol-c`),
# so the library that is built, the library the tests use and the library the
# phone runs are the same source, and a sanitizer or fuzz build is possible at
# all. The clone below stays as a fallback for a checkout that has no vendor
# directory; when it is used, the commit is checked against the recorded one.
VENDORED=$PROJECT/vendor/libsignal-protocol-c
# One source of truth: the vendored tree, or a loud failure. A second path
# (a clone) could drift from it silently, and there is no reason for one - the
# source belongs in the repository (see docs/OMEMO.md).
if [ ! -d "$VENDORED/src" ]; then
    echo "--- EI KELPAA: vendor/libsignal-protocol-c puuttuu." >&2
    echo "    Lähde kuuluu repoon (ks. VENDOR.txt); älä kloonaa tilalle." >&2
    exit 1
fi
ln -sfn "$VENDORED" libsignal-protocol-c
echo "--- lähde: repossa oleva vendor/libsignal-protocol-c (VENDOR.txt)"

echo "--- protobuf-c (ajonaikainen, yksi C-tiedosto)"
cp "$PROTOBUF_C_DIR/protobuf-c/protobuf-c.h" "$PREFIX/include/"
"$ZIG" cc -target "$ZIG_TARGET" -O2 -fPIC -I "$PREFIX/include" \
    -c "$PROTOBUF_C_DIR/protobuf-c/protobuf-c.c" -o obj/protobuf-c.o
"$ZIG" ar rcs "$PREFIX/lib/libprotobuf-c.a" obj/protobuf-c.o

echo "--- libsignal-protocol-c"
# The library is bigger than its top level: curve.c calls into the whole
# ed25519 stack under curve25519/ed25519, and the link fails with
# "undefined symbol: all_fast_tests" without it. Compile everything upstream's
# CMake compiles - all of src/ except the protobuf-c runtime (its own library
# above) and internal_slow_tests.c, which upstream never builds.
rm -f obj/signal-*.o
SOURCES=$(find libsignal-protocol-c/src -name '*.c' \
    -not -path '*/protobuf-c/*' \
    -not -name 'internal_slow_tests.c' | sort)
for source in $SOURCES; do
    object="obj/signal-$(echo "$source" | sed 's|^libsignal-protocol-c/src/||; s|/|_|g; s|\.c$||').o"
    "$ZIG" cc -target "$ZIG_TARGET" -O2 -fPIC -DHAVE_CONFIG_H=0 \
        -I "$PREFIX/include" \
        -I libsignal-protocol-c/src \
        -I libsignal-protocol-c/src/curve25519 \
        -I libsignal-protocol-c/src/curve25519/ed25519 \
        -I libsignal-protocol-c/src/curve25519/ed25519/nacl_includes \
        -I libsignal-protocol-c/src/curve25519/ed25519/additions \
        -I libsignal-protocol-c/src/curve25519/ed25519/additions/generalized \
        -I libsignal-protocol-c/src/curve25519/ed25519/tests \
        -c "$source" -o "$object"
done
rm -f "$PREFIX/lib/libsignal-protocol-c.a"
"$ZIG" ar rcs "$PREFIX/lib/libsignal-protocol-c.a" obj/signal-*.o

mkdir -p "$PREFIX/include/signal"
cp libsignal-protocol-c/src/*.h "$PREFIX/include/signal/"

echo "--- valmis:"
ls -l "$PREFIX/lib/"

# The checksums are **checked**, not just written: the committed file is what
# the repository's archives are supposed to be, so a rebuild that differs is
# said out loud instead of quietly replacing it. (A difference is not by itself
# a fault - the compiler version is part of the result - which is why the
# message says which of the two it might be.)
SUMS=$PREFIX/lib/SHA256SUMS
NEW=$(sha256sum "$PREFIX/lib/"*.a)
if [ -f "$SUMS" ] && ! printf '%s\n' "$NEW" | cmp -s - "$SUMS"; then
    FRESH=$WORK/SHA256SUMS.new
    printf '%s\n' "$NEW" > "$FRESH"
    echo "--- HUOM: uudelleenrakennettu kirjasto eroaa repossa olevasta:" >&2
    diff -u "$SUMS" "$FRESH" >&2 || true
    echo "    Sama lähde eri kääntäjällä antaa eri binäärin - tarkista kumpi on kyseessä" >&2
    echo "    ennen kuin korvaat arkistot ja tämän tiedoston." >&2
    exit 1
fi
printf '%s\n' "$NEW" > "$SUMS"
echo "--- tarkistussummat:"
cat "$SUMS"
