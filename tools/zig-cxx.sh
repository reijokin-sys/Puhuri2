#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# C++ driver backed by the ziglang Python package used by the cross build.
set -eu

ZIG=$(python3 -c "import ziglang, os; print(os.path.join(os.path.dirname(ziglang.__file__), 'zig'))")
exec "$ZIG" c++ -target "${ZIG_TARGET:-aarch64-linux-gnu.2.39}" "$@"
