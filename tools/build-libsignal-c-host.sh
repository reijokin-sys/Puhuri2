#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Build libsignal-protocol-c + protobuf-c **natively** (the host architecture,
# x86-64 here) from the vendored source, into build/libsignal-c/host/prefix.
#
#   tools/build-libsignal-c-host.sh
#
# Why: the phone build links the aarch64 archives and this one links the
# vendored source too, so the tests and the phone run the same library source
# (the review's point, and what makes a sanitizer or fuzz build possible).
# Without this the host build used the distribution's own library, which is a
# different build from the phone's.
set -eu
# Build libsignal-protocol-c + protobuf-c natively from the vendored source,
# so the host (x86-64) tests link the same source as the phone.
set -eu
P=$(cd "$(dirname "$0")/.." && pwd)
SRC=$P/vendor/libsignal-protocol-c
PB=$P/vendor/protobuf-c-1.5.1/protobuf-c
OUT=$P/build/libsignal-c/host
mkdir -p "$OUT/obj" "$OUT/prefix/include" "$OUT/prefix/lib"
cd "$SRC"
cp "$PB/protobuf-c.h" "$OUT/prefix/include/"
cc -O2 -fPIC -I "$OUT/prefix/include" -c "$PB/protobuf-c.c" -o "$OUT/obj/protobuf-c.o"
ar rcs "$OUT/prefix/lib/libprotobuf-c.a" "$OUT/obj/protobuf-c.o"
rm -f "$OUT"/obj/signal-*.o
find src -name '*.c' -not -path '*/protobuf-c/*' -not -name 'internal_slow_tests.c' | sort > "$OUT/sources.txt"
while read -r source; do
    object="$OUT/obj/signal-$(printf '%s' "$source" | sed 's|^src/||; s|/|_|g; s|\.c$||').o"
    cc -O2 -fPIC -DHAVE_CONFIG_H=0 -I "$OUT/prefix/include" \
        -I src -I src/curve25519 -I src/curve25519/ed25519 \
        -I src/curve25519/ed25519/nacl_includes -I src/curve25519/ed25519/additions \
        -I src/curve25519/ed25519/additions/generalized -I src/curve25519/ed25519/tests \
        -c "$source" -o "$object"
done < "$OUT/sources.txt"
rm -f "$OUT/prefix/lib/libsignal-protocol-c.a"
ar rcs "$OUT/prefix/lib/libsignal-protocol-c.a" "$OUT"/obj/signal-*.o
cp src/*.h "$OUT/prefix/include/"
ls -l "$OUT/prefix/lib/" | tail -3
